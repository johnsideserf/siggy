//! SQLite persistence layer (WAL mode).
//!
//! Three core tables: `conversations`, `messages`, `read_markers`. Schema
//! migrations are version-based (see private `migrate`). Read paths
//! propagate errors via `?`; write paths are logged via
//! [`crate::conversation_store::db_warn`] (silent) or
//! `App::db_warn_visible` (surfaces in the status bar) so transient
//! persistence failures don't break the UI.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::app::{Conversation, DisplayMessage};
use crate::mute::MuteState;
use crate::signal::types::{LinkPreview, Mention, MessageStatus, PollData, PollVote, Reaction};

/// (sender, body, timestamp_ms, conversation_id, conversation_name)
pub type SearchRow = (String, String, i64, String, String);

/// A schema migration: the target version it brings the database up to, and
/// the SQL batch that performs the change. Each batch is responsible for its
/// own `BEGIN; ...; UPDATE/INSERT schema_version; COMMIT;` so we never have to
/// template SQL strings at runtime.
struct Migration {
    version: i32,
    sql: &'static str,
}

/// Ordered list of schema migrations. To add a new version, append a new
/// `Migration` with the next version number and the SQL batch that lifts the
/// schema from `version - 1` to `version`. The batch must end with
/// `UPDATE schema_version SET version = N;` (or, for version 1, the initial
/// `INSERT`). Never edit an existing migration -- write a new one.
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: "
            BEGIN;

            CREATE TABLE conversations (
                id         TEXT PRIMARY KEY,
                name       TEXT NOT NULL,
                is_group   INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE messages (
                rowid           INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id TEXT NOT NULL REFERENCES conversations(id),
                sender          TEXT NOT NULL,
                timestamp       TEXT NOT NULL,
                body            TEXT NOT NULL,
                is_system       INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX idx_messages_conv_ts ON messages(conversation_id, timestamp);

            CREATE TABLE read_markers (
                conversation_id TEXT PRIMARY KEY REFERENCES conversations(id),
                last_read_rowid INTEGER NOT NULL DEFAULT 0
            );

            INSERT INTO schema_version (version) VALUES (1);

            COMMIT;
        ",
    },
    Migration {
        version: 2,
        sql: "
            BEGIN;
            ALTER TABLE conversations ADD COLUMN muted INTEGER NOT NULL DEFAULT 0;
            UPDATE schema_version SET version = 2;
            COMMIT;
        ",
    },
    Migration {
        version: 3,
        sql: "
            BEGIN;
            ALTER TABLE messages ADD COLUMN status INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE messages ADD COLUMN timestamp_ms INTEGER NOT NULL DEFAULT 0;
            UPDATE schema_version SET version = 3;
            COMMIT;
        ",
    },
    Migration {
        version: 4,
        sql: "
            BEGIN;
            CREATE TABLE reactions (
                rowid           INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id TEXT NOT NULL,
                target_ts_ms    INTEGER NOT NULL,
                target_author   TEXT NOT NULL,
                emoji           TEXT NOT NULL,
                sender          TEXT NOT NULL,
                UNIQUE(conversation_id, target_ts_ms, target_author, sender)
            );
            CREATE INDEX idx_reactions_target ON reactions(conversation_id, target_ts_ms);
            UPDATE schema_version SET version = 4;
            COMMIT;
        ",
    },
    Migration {
        version: 5,
        sql: "
            BEGIN;
            CREATE INDEX IF NOT EXISTS idx_messages_conv_ts_ms ON messages(conversation_id, timestamp_ms);
            UPDATE schema_version SET version = 5;
            COMMIT;
        ",
    },
    Migration {
        version: 6,
        sql: "
            BEGIN;
            ALTER TABLE messages ADD COLUMN is_edited INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE messages ADD COLUMN is_deleted INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE messages ADD COLUMN quote_author TEXT;
            ALTER TABLE messages ADD COLUMN quote_body TEXT;
            ALTER TABLE messages ADD COLUMN quote_ts_ms INTEGER;
            ALTER TABLE messages ADD COLUMN sender_id TEXT NOT NULL DEFAULT '';
            UPDATE schema_version SET version = 6;
            COMMIT;
        ",
    },
    Migration {
        version: 7,
        sql: "
            BEGIN;
            ALTER TABLE conversations ADD COLUMN expiration_timer INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE messages ADD COLUMN expires_in_seconds INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE messages ADD COLUMN expiration_start_ms INTEGER NOT NULL DEFAULT 0;
            UPDATE schema_version SET version = 7;
            COMMIT;
        ",
    },
    Migration {
        version: 8,
        sql: "
            BEGIN;
            ALTER TABLE conversations ADD COLUMN accepted INTEGER NOT NULL DEFAULT 1;
            UPDATE schema_version SET version = 8;
            COMMIT;
        ",
    },
    Migration {
        version: 9,
        sql: "
            BEGIN;
            ALTER TABLE conversations ADD COLUMN blocked INTEGER NOT NULL DEFAULT 0;
            UPDATE schema_version SET version = 9;
            COMMIT;
        ",
    },
    Migration {
        version: 10,
        sql: "
            BEGIN;
            ALTER TABLE messages ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
            UPDATE schema_version SET version = 10;
            COMMIT;
        ",
    },
    Migration {
        version: 11,
        sql: "
            BEGIN;
            ALTER TABLE messages ADD COLUMN poll_data TEXT;
            CREATE TABLE IF NOT EXISTS poll_votes (
                conv_id TEXT NOT NULL,
                poll_timestamp INTEGER NOT NULL,
                voter TEXT NOT NULL,
                voter_name TEXT,
                option_indexes TEXT NOT NULL,
                vote_count INTEGER NOT NULL DEFAULT 1,
                UNIQUE(conv_id, poll_timestamp, voter)
            );
            UPDATE schema_version SET version = 11;
            COMMIT;
        ",
    },
    Migration {
        version: 12,
        sql: "
            BEGIN;
            ALTER TABLE messages ADD COLUMN link_preview TEXT;
            UPDATE schema_version SET version = 12;
            COMMIT;
        ",
    },
    Migration {
        version: 13,
        sql: "
            BEGIN;
            ALTER TABLE messages ADD COLUMN body_raw TEXT;
            ALTER TABLE messages ADD COLUMN mentions_json TEXT;
            UPDATE schema_version SET version = 13;
            COMMIT;
        ",
    },
    Migration {
        version: 14,
        sql: "
            BEGIN;
            ALTER TABLE conversations ADD COLUMN mute_expires_at TEXT;
            UPDATE schema_version SET version = 14;
            COMMIT;
        ",
    },
    Migration {
        version: 15,
        sql: "
            BEGIN;
            ALTER TABLE conversations ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;
            UPDATE schema_version SET version = 15;
            COMMIT;
        ",
    },
    // Incoming-message idempotency (#640 U6, KTD-2): at-least-once delivery
    // from the backend must be safe, so replayed envelopes may not duplicate
    // rows or re-fire side effects.
    //
    // Key design: one incoming message persists as SEVERAL rows sharing
    // (conversation_id, sender_id, timestamp_ms) -- the body row plus one row
    // per attachment -- so that triple alone cannot be unique. `entry_seq`
    // numbers the rows of one message in insertion order (0 = body/first
    // entry), which is deterministic across a replayed parse, so the 4-column
    // key dedups exact replays while multi-entry messages survive.
    //
    // The backfill numbers existing rows per key by rowid, which makes the
    // index build on any populated table without deleting anything: rows that
    // LOOK like duplicates under the 3-column key are indistinguishable from
    // legitimate multi-entry messages, and a wrong dedup is unrecoverable
    // while a missed one is cosmetic. Historical duplicates therefore stay;
    // only post-migration replays are suppressed.
    //
    // Outgoing rows (sender = 'you') are deliberately outside the partial
    // index: the #480 send-confirm dance rewrites their timestamp_ms, which
    // a covering unique index would corrupt or reject. They get an app-level
    // exists-check in on_message_added instead.
    Migration {
        version: 16,
        sql: "
            BEGIN;
            ALTER TABLE messages ADD COLUMN entry_seq INTEGER NOT NULL DEFAULT 0;
            UPDATE messages SET entry_seq = sub.rn - 1
              FROM (SELECT rowid AS r,
                           ROW_NUMBER() OVER (
                               PARTITION BY conversation_id, sender_id, timestamp_ms
                               ORDER BY rowid
                           ) AS rn
                      FROM messages
                     WHERE sender <> 'you' AND sender_id <> '') AS sub
             WHERE messages.rowid = sub.r;
            CREATE UNIQUE INDEX idx_messages_incoming_dedup
                ON messages(conversation_id, sender_id, timestamp_ms, entry_seq)
                WHERE sender <> 'you' AND sender_id <> '';
            UPDATE schema_version SET version = 16;
            COMMIT;
        ",
    },
    // Native receive journal (#642 U11, plan KTD-2). The native adapter's
    // engine thread appends each mapped incoming event here (own
    // connection) BEFORE pulling the next stream item, because
    // libsignal-service acks envelopes before yielding them: this table is
    // what makes an acked-but-unprocessed envelope survive a crash. The
    // main thread deletes a row after committing the event's effects;
    // startup replays whatever is left and the entry_seq dedup (v16) makes
    // replays silent. Empty and unused under the signal-cli backend.
    Migration {
        version: 17,
        sql: "
            BEGIN;
            CREATE TABLE native_journal (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                payload TEXT NOT NULL
            );
            UPDATE schema_version SET version = 17;
            COMMIT;
        ",
    },
];

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        // The standard WAL pairing: commits stop fsyncing the WAL (the sync
        // happens at checkpoint instead). Without this SQLite defaults to
        // FULL, which paid one fsync per inserted message during sync bursts
        // (#489). Durability loss is limited to power failure, not app crash.
        conn.execute_batch("PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        conn.execute_batch("PRAGMA secure_delete=ON;")?;
        // Wait for a competing writer instead of failing with SQLITE_BUSY.
        // WAL allows exactly one writer at a time; the native backend's
        // adapter connection (#640 U6/U11) inserts from another thread while
        // the main loop may hold a drain batch, so both sides must queue
        // rather than error. Drain batches are short; 5s is generous.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    /// Open a transaction for batching a burst of writes (e.g. the signal
    /// event drain loop, #489). Pairs with [`Self::commit_batch`]; both are
    /// best-effort because the writes inside carry their own error handling
    /// (`db_warn_visible`) and must still execute if batching fails.
    pub fn begin_batch(&self) {
        if let Err(e) = self.conn.execute_batch("BEGIN IMMEDIATE;") {
            crate::debug_log::logf(format_args!("begin_batch failed: {e}"));
        }
    }

    /// Commit a batch opened by [`Self::begin_batch`].
    pub fn commit_batch(&self) {
        if let Err(e) = self.conn.execute_batch("COMMIT;") {
            crate::debug_log::logf(format_args!("commit_batch failed: {e}"));
        }
    }

    /// Filesystem path of the open database, `None` for in-memory DBs.
    /// The native adapter's engine thread opens its own journal connection
    /// to the same file (#642 U11, KTD-2).
    pub fn path(&self) -> Option<std::path::PathBuf> {
        self.conn
            .path()
            .filter(|p| !p.is_empty())
            .map(std::path::PathBuf::from)
    }

    // --- Native receive journal (#642 U11, KTD-2) ---

    /// Journal rows the native adapter persisted but the main thread has not
    /// yet confirmed processed: `(id, payload)` in insertion order. Appends
    /// happen on the adapter's own connection (`backend::native::journal`);
    /// this side only replays and deletes.
    pub fn journal_pending(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, payload FROM native_journal ORDER BY id")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Drop one journal row after its event's effects are committed.
    pub fn journal_delete(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM native_journal WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Atomically fold every row of conversation `from` into `to` (#642 U11
    /// stage 3, the KTD-6 late-sync re-key): a message that arrived before
    /// contact sync created a uuid-keyed conversation, and the peer's E.164
    /// is now known. Runs inside a SAVEPOINT rather than BEGIN because the
    /// caller may already hold a drain batch transaction.
    ///
    /// `UPDATE OR IGNORE`: a moved row that collides with an identical twin
    /// already under `to` (same dedup key - the envelope was delivered on
    /// both sides of the re-key boundary) stays behind and is dropped with
    /// the rest of the source conversation, which is exactly dedup.
    pub fn merge_conversation(&self, from: &str, to: &str) -> Result<()> {
        self.conn.execute_batch("SAVEPOINT rekey;")?;
        let moved = (|| -> Result<()> {
            self.conn.execute(
                "UPDATE OR IGNORE messages SET conversation_id = ?2 WHERE conversation_id = ?1",
                params![from, to],
            )?;
            self.conn.execute(
                "UPDATE OR IGNORE reactions SET conversation_id = ?2 WHERE conversation_id = ?1",
                params![from, to],
            )?;
            self.conn.execute(
                "UPDATE OR IGNORE read_markers SET conversation_id = ?2 WHERE conversation_id = ?1",
                params![from, to],
            )?;
            self.conn.execute(
                "DELETE FROM messages WHERE conversation_id = ?1",
                params![from],
            )?;
            self.conn.execute(
                "DELETE FROM reactions WHERE conversation_id = ?1",
                params![from],
            )?;
            self.conn.execute(
                "DELETE FROM read_markers WHERE conversation_id = ?1",
                params![from],
            )?;
            self.conn
                .execute("DELETE FROM conversations WHERE id = ?1", params![from])?;
            Ok(())
        })();
        match moved {
            Ok(()) => {
                self.conn.execute_batch("RELEASE rekey;")?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK TO rekey; RELEASE rekey;");
                Err(e)
            }
        }
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);",
        )?;

        let current: i32 = self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )?;

        for migration in MIGRATIONS {
            if current < migration.version {
                self.conn.execute_batch(migration.sql)?;
            }
        }
        Ok(())
    }

    // --- Conversations ---

    pub fn upsert_conversation(&self, id: &str, name: &str, is_group: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO conversations (id, name, is_group)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name",
            params![id, name, is_group as i32],
        )?;
        Ok(())
    }

    pub fn update_accepted(&self, id: &str, accepted: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE conversations SET accepted = ?2 WHERE id = ?1",
            params![id, accepted as i32],
        )?;
        Ok(())
    }

    pub fn delete_conversation(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM reactions WHERE conversation_id = ?1",
            params![id],
        )?;
        self.conn.execute(
            "DELETE FROM messages WHERE conversation_id = ?1",
            params![id],
        )?;
        self.conn.execute(
            "DELETE FROM read_markers WHERE conversation_id = ?1",
            params![id],
        )?;
        self.conn
            .execute("DELETE FROM conversations WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Load a page of messages for a conversation, ordered chronologically (oldest first).
    /// `offset` skips the N most recent messages (for pagination).
    pub fn load_messages_page(
        &self,
        conv_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<DisplayMessage>> {
        let mut msg_stmt = self.conn.prepare(
            "SELECT sender, timestamp, body, is_system, status, timestamp_ms, is_edited, is_deleted, quote_author, quote_body, quote_ts_ms, sender_id, expires_in_seconds, expiration_start_ms, pinned, poll_data, link_preview, body_raw, mentions_json FROM messages
             WHERE conversation_id = ?1
             ORDER BY timestamp_ms DESC, rowid DESC LIMIT ?2 OFFSET ?3",
        )?;

        let mut messages: Vec<DisplayMessage> = msg_stmt
            .query_map(params![conv_id, limit as i64, offset as i64], |row| {
                let sender: String = row.get(0)?;
                let ts_str: String = row.get(1)?;
                let body: String = row.get(2)?;
                let is_system: bool = row.get::<_, i32>(3)? != 0;
                let status_i32: i32 = row.get(4)?;
                let timestamp_ms: i64 = row.get(5)?;
                let is_edited: bool = row.get::<_, i32>(6)? != 0;
                let is_deleted: bool = row.get::<_, i32>(7)? != 0;
                let quote_author: Option<String> = row.get(8)?;
                let quote_body: Option<String> = row.get(9)?;
                let quote_ts_ms: Option<i64> = row.get(10)?;
                let sender_id: String = row.get(11)?;
                let expires_in_seconds: i64 = row.get(12)?;
                let expiration_start_ms: i64 = row.get(13)?;
                let is_pinned: bool = row.get::<_, i32>(14)? != 0;
                let poll_data_json: Option<String> = row.get(15)?;
                let link_preview_json: Option<String> = row.get(16)?;
                let body_raw: Option<String> = row.get(17)?;
                let mentions_json: Option<String> = row.get(18)?;
                Ok((
                    sender,
                    ts_str,
                    body,
                    is_system,
                    status_i32,
                    timestamp_ms,
                    is_edited,
                    is_deleted,
                    quote_author,
                    quote_body,
                    quote_ts_ms,
                    sender_id,
                    expires_in_seconds,
                    expiration_start_ms,
                    is_pinned,
                    poll_data_json,
                    link_preview_json,
                    body_raw,
                    mentions_json,
                ))
            })?
            .filter_map(|r| r.ok())
            .filter_map(
                |(
                    sender,
                    ts_str,
                    body,
                    is_system,
                    status_i32,
                    timestamp_ms,
                    is_edited,
                    is_deleted,
                    quote_author,
                    quote_body,
                    quote_ts_ms,
                    sender_id,
                    expires_in_seconds,
                    expiration_start_ms,
                    is_pinned,
                    poll_data_json,
                    link_preview_json,
                    body_raw,
                    mentions_json,
                )| {
                    let timestamp = chrono::DateTime::parse_from_rfc3339(&ts_str)
                        .ok()?
                        .with_timezone(&chrono::Utc);
                    let quote = match (quote_author, quote_body, quote_ts_ms) {
                        (Some(author), Some(body), Some(ts)) => Some(crate::app::Quote {
                            author_id: author.clone(),
                            author,
                            body: body.replace('\u{FFFC}', ""),
                            timestamp_ms: ts,
                        }),
                        _ => None,
                    };
                    let poll_data =
                        poll_data_json.and_then(|j| serde_json::from_str::<PollData>(&j).ok());
                    let preview = link_preview_json
                        .and_then(|j| serde_json::from_str::<LinkPreview>(&j).ok());
                    let mentions: Vec<Mention> = mentions_json
                        .as_deref()
                        .and_then(|j| serde_json::from_str(j).ok())
                        .unwrap_or_default();
                    Some(DisplayMessage {
                        sender,
                        timestamp,
                        body,
                        is_system,
                        image_lines: None,
                        image_path: None,
                        status: MessageStatus::from_i32(status_i32),
                        timestamp_ms,
                        reactions: Vec::new(),
                        mention_ranges: Vec::new(),
                        style_ranges: Vec::new(),
                        body_raw,
                        mentions,
                        quote,
                        is_edited,
                        is_deleted,
                        is_pinned,
                        sender_id,
                        expires_in_seconds,
                        expiration_start_ms,
                        poll_data,
                        poll_votes: Vec::new(),
                        preview,
                        preview_image_lines: None,
                        preview_image_path: None,
                    })
                },
            )
            .collect();

        // Reverse so oldest first
        messages.reverse();

        // Attach reactions
        let mut ts_to_idx: HashMap<i64, Vec<usize>> = HashMap::new();
        for (i, m) in messages.iter().enumerate() {
            ts_to_idx.entry(m.timestamp_ms).or_default().push(i);
        }
        let page_range = messages
            .first()
            .map(|f| (f.timestamp_ms, messages[messages.len() - 1].timestamp_ms));
        if let Some((min_ts, max_ts)) = page_range
            && let Ok(reactions) = self.load_reactions_in_range(conv_id, min_ts, max_ts)
        {
            for (target_ts, target_author, emoji, sender) in reactions {
                let idx = ts_to_idx.get(&target_ts).and_then(|idxs| {
                    idxs.iter()
                        .find(|&&i| {
                            messages[i].sender == target_author || messages[i].is_outgoing()
                        })
                        .or_else(|| idxs.first())
                        .copied()
                });
                if let Some(msg) = idx.and_then(|i| messages.get_mut(i)) {
                    if let Some(existing) = msg.reactions.iter_mut().find(|r| r.sender == sender) {
                        existing.emoji = emoji;
                    } else {
                        msg.reactions.push(Reaction { emoji, sender });
                    }
                }
            }
        }

        // Attach poll votes
        for msg in &mut messages {
            if msg.poll_data.is_some()
                && let Ok(votes) = self.load_poll_votes(conv_id, msg.timestamp_ms)
            {
                msg.poll_votes = votes;
            }
        }

        Ok(messages)
    }

    /// Load all conversations with their most recent messages (up to `msg_limit`).
    pub fn load_conversations(&self, msg_limit: usize) -> Result<Vec<Conversation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, is_group, expiration_timer, accepted, archived FROM conversations",
        )?;

        let convs: Vec<(String, String, bool, i64, bool, bool)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)? != 0,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i32>(4)? != 0,
                    row.get::<_, i32>(5)? != 0,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut result = Vec::with_capacity(convs.len());

        for (id, name, is_group, expiration_timer, accepted, archived) in convs {
            let messages = self.load_messages_page(&id, msg_limit, 0)?;
            let unread = self.unread_count(&id).unwrap_or(0);

            result.push(Conversation {
                name,
                id: id.clone(),
                messages,
                unread,
                is_group,
                expiration_timer,
                accepted,
                archived,
            });
        }

        Ok(result)
    }

    /// Load conversation IDs ordered by most recent message.
    pub fn load_conversation_order(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id FROM conversations c
             LEFT JOIN messages m ON m.conversation_id = c.id
             GROUP BY c.id
             ORDER BY COALESCE(MAX(m.rowid), 0) DESC",
        )?;

        let ids: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(ids)
    }

    // --- Messages ---

    #[allow(clippy::too_many_arguments, dead_code)]
    pub fn insert_message(
        &self,
        conv_id: &str,
        sender: &str,
        timestamp: &str,
        body: &str,
        is_system: bool,
        status: Option<MessageStatus>,
        timestamp_ms: i64,
    ) -> Result<i64> {
        // sender_id is '' here, which keeps the row outside the dedup index,
        // so the insert can never be conflict-skipped (unwrap_or unreachable).
        Ok(self
            .insert_message_full(
                conv_id,
                sender,
                timestamp,
                body,
                is_system,
                status,
                timestamp_ms,
                "",
                None,
                None,
                None,
                0,
                0,
                0,
            )?
            .unwrap_or(-1))
    }

    /// Insert one message row. Returns `Ok(Some(rowid))` for a new row and
    /// `Ok(None)` when the row was a replay of an already-persisted incoming
    /// entry (#640 U6): the insert is conflict-targeted at the partial unique
    /// index on (conversation_id, sender_id, timestamp_ms, entry_seq), so
    /// only genuine key collisions are skipped -- any other constraint
    /// violation still surfaces as an error rather than being swallowed.
    /// Callers must skip the full side-effect chain (in-memory append,
    /// notification, unarchive, sidebar reorder) on `None`.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_message_full(
        &self,
        conv_id: &str,
        sender: &str,
        timestamp: &str,
        body: &str,
        is_system: bool,
        status: Option<MessageStatus>,
        timestamp_ms: i64,
        sender_id: &str,
        quote_author: Option<&str>,
        quote_body: Option<&str>,
        quote_ts_ms: Option<i64>,
        expires_in_seconds: i64,
        expiration_start_ms: i64,
        entry_seq: i64,
    ) -> Result<Option<i64>> {
        let status_i32 = status.map(|s| s.to_i32()).unwrap_or(0);
        let changed = self.conn.execute(
            "INSERT INTO messages (conversation_id, sender, timestamp, body, is_system, status, timestamp_ms, sender_id, quote_author, quote_body, quote_ts_ms, expires_in_seconds, expiration_start_ms, entry_seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT (conversation_id, sender_id, timestamp_ms, entry_seq)
                 WHERE sender <> 'you' AND sender_id <> ''
                 DO NOTHING",
            params![conv_id, sender, timestamp, body, is_system as i32, status_i32, timestamp_ms, sender_id, quote_author, quote_body, quote_ts_ms, expires_in_seconds, expiration_start_ms, entry_seq],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        Ok(Some(self.conn.last_insert_rowid()))
    }

    /// App-level duplicate check for outgoing rows, which live outside the
    /// dedup index because the #480 send-confirm dance rewrites their
    /// timestamp_ms (#640 U6). Matches the sync-transcript replay shape: an
    /// outgoing row in this conversation with this (server) timestamp and
    /// this body already exists. Body is included so multi-entry outgoing
    /// messages (text + attachments, same timestamp) are not misjudged.
    pub fn outgoing_message_exists(&self, conv_id: &str, timestamp_ms: i64, body: &str) -> bool {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row(
                "SELECT 1 FROM messages
                  WHERE conversation_id = ?1 AND timestamp_ms = ?2
                    AND sender = 'you' AND body = ?3
                  LIMIT 1",
                params![conv_id, timestamp_ms, body],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// Update delivery status for an outgoing message by its ms epoch timestamp.
    pub fn update_message_status(
        &self,
        conv_id: &str,
        timestamp_ms: i64,
        status: i32,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET status = ?3
             WHERE conversation_id = ?1 AND timestamp_ms = ?2 AND sender = 'you' AND status < ?3",
            params![conv_id, timestamp_ms, status],
        )?;
        Ok(())
    }

    /// Author fields of the message row at `timestamp_ms`:
    /// (sender, sender_id, is_outgoing). Used by the edit / remote-delete
    /// author check (#482) when the target message is outside the loaded
    /// page. Outgoing detection mirrors `DisplayMessage::is_outgoing`:
    /// a stored status (non-zero) or the literal "you" sender.
    pub fn message_author(
        &self,
        conv_id: &str,
        timestamp_ms: i64,
    ) -> Result<Option<(String, String, bool)>> {
        use rusqlite::OptionalExtension;
        let row = self
            .conn
            .query_row(
                "SELECT sender, sender_id, status FROM messages
                 WHERE conversation_id = ?1 AND timestamp_ms = ?2 LIMIT 1",
                params![conv_id, timestamp_ms],
                |row| {
                    let sender: String = row.get(0)?;
                    let sender_id: String = row.get(1)?;
                    let status: i32 = row.get(2)?;
                    let outgoing = status != 0 || sender == "you";
                    Ok((sender, sender_id, outgoing))
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Upgrade an outgoing message's status by timestamp alone, across all
    /// conversations. Fallback for buffered receipts whose target is not in
    /// the loaded message page (#484): receipts carry no conversation id,
    /// and outgoing timestamps are server-assigned and unique in practice.
    /// `status > 0` restricts to outgoing rows; `status < ?2` keeps the
    /// write upgrade-only, so a stray collision cannot downgrade anything.
    pub fn update_outgoing_status_any_conv(&self, timestamp_ms: i64, status: i32) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET status = ?2
             WHERE timestamp_ms = ?1 AND sender = 'you' AND status > 0 AND status < ?2",
            params![timestamp_ms, status],
        )?;
        Ok(())
    }

    /// Mark an outgoing message Failed. Needs its own method because
    /// `update_message_status` only allows upward transitions and Failed (1)
    /// sits below Sending (2), so a failure write through it was silently
    /// dropped (#486). A failure is only valid from the Sending state, so
    /// that is the guard here.
    pub fn mark_message_failed(&self, conv_id: &str, timestamp_ms: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET status = ?3
             WHERE conversation_id = ?1 AND timestamp_ms = ?2 AND sender = 'you' AND status = ?4",
            params![
                conv_id,
                timestamp_ms,
                MessageStatus::Failed.to_i32(),
                MessageStatus::Sending.to_i32()
            ],
        )?;
        Ok(())
    }

    /// Update timestamp_ms and status for an outgoing message when the server assigns
    /// a canonical timestamp (replacing the local one).
    pub fn update_message_timestamp_ms(
        &self,
        conv_id: &str,
        old_ts: i64,
        new_ts: i64,
        status: i32,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET timestamp_ms = ?3, status = ?4
             WHERE conversation_id = ?1 AND timestamp_ms = ?2 AND sender = 'you'",
            params![conv_id, old_ts, new_ts, status],
        )?;
        Ok(())
    }

    // --- Read markers ---

    pub fn save_read_marker(&self, conv_id: &str, last_rowid: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO read_markers (conversation_id, last_read_rowid)
             VALUES (?1, ?2)
             ON CONFLICT(conversation_id) DO UPDATE SET last_read_rowid = excluded.last_read_rowid",
            params![conv_id, last_rowid],
        )?;
        Ok(())
    }

    pub fn last_message_rowid(&self, conv_id: &str) -> Result<Option<i64>> {
        let result = self.conn.query_row(
            "SELECT MAX(rowid) FROM messages WHERE conversation_id = ?1",
            params![conv_id],
            |row| row.get::<_, Option<i64>>(0),
        )?;
        Ok(result)
    }

    pub fn unread_count(&self, conv_id: &str) -> Result<usize> {
        let last_read: i64 = self.conn.query_row(
            "SELECT COALESCE(
                    (SELECT last_read_rowid FROM read_markers WHERE conversation_id = ?1),
                    0
                 )",
            params![conv_id],
            |row| row.get(0),
        )?;

        // Incoming only (status = 0, sender filter for legacy/demo rows):
        // your own sent messages are never "unread". Mirrors the in-memory
        // recompute in handle_read_sync, which filters status.is_none();
        // without this, sending N messages and restarting showed an unread
        // badge of N with the divider above your own messages (#484).
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM messages
             WHERE conversation_id = ?1 AND rowid > ?2 AND is_system = 0
               AND status = 0 AND sender != 'you'",
            params![conv_id, last_read],
            |row| row.get(0),
        )?;

        Ok(count as usize)
    }

    // --- Reactions ---

    pub fn upsert_reaction(
        &self,
        conv_id: &str,
        target_ts_ms: i64,
        target_author: &str,
        sender: &str,
        emoji: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO reactions (conversation_id, target_ts_ms, target_author, sender, emoji)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(conversation_id, target_ts_ms, target_author, sender)
             DO UPDATE SET emoji = excluded.emoji",
            params![conv_id, target_ts_ms, target_author, sender, emoji],
        )?;
        Ok(())
    }

    pub fn remove_reaction(
        &self,
        conv_id: &str,
        target_ts_ms: i64,
        target_author: &str,
        sender: &str,
    ) -> Result<()> {
        self.conn.execute(
            "DELETE FROM reactions
             WHERE conversation_id = ?1 AND target_ts_ms = ?2
               AND target_author = ?3 AND sender = ?4",
            params![conv_id, target_ts_ms, target_author, sender],
        )?;
        Ok(())
    }

    /// Load reactions targeting messages within a timestamp range. Used by
    /// the paged message loader so each page reads only its own reactions
    /// instead of the conversation's full reactions table (#488); the range
    /// query is served by idx_reactions_target.
    pub fn load_reactions_in_range(
        &self,
        conv_id: &str,
        min_ts: i64,
        max_ts: i64,
    ) -> Result<Vec<(i64, String, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT target_ts_ms, target_author, emoji, sender FROM reactions
             WHERE conversation_id = ?1 AND target_ts_ms BETWEEN ?2 AND ?3",
        )?;
        let rows: Vec<(i64, String, String, String)> = stmt
            .query_map(params![conv_id, min_ts, max_ts], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Load all reactions for a conversation.
    /// Returns (target_ts_ms, target_author, emoji, sender) tuples.
    ///
    /// Test-only: production paging uses `load_reactions_in_range` (#488), so
    /// the unbounded variant now exists purely as a convenience for tests that
    /// assert reaction storage across a whole conversation.
    #[cfg(test)]
    pub fn load_reactions(&self, conv_id: &str) -> Result<Vec<(i64, String, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT target_ts_ms, target_author, emoji, sender FROM reactions
             WHERE conversation_id = ?1",
        )?;
        let rows: Vec<(i64, String, String, String)> = stmt
            .query_map(params![conv_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Update the body and mark a message as edited.
    pub fn update_message_body(&self, conv_id: &str, timestamp_ms: i64, body: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET body = ?3, is_edited = 1
             WHERE conversation_id = ?1 AND timestamp_ms = ?2",
            params![conv_id, timestamp_ms, body],
        )?;
        Ok(())
    }

    /// Mark a message as locally deleted.
    pub fn mark_message_deleted(&self, conv_id: &str, timestamp_ms: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET is_deleted = 1, body = '[deleted]'
             WHERE conversation_id = ?1 AND timestamp_ms = ?2",
            params![conv_id, timestamp_ms],
        )?;
        Ok(())
    }

    /// Set the pinned state of a message.
    pub fn set_message_pinned(&self, conv_id: &str, timestamp_ms: i64, pinned: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET pinned = ?3
             WHERE conversation_id = ?1 AND timestamp_ms = ?2",
            params![conv_id, timestamp_ms, pinned as i32],
        )?;
        Ok(())
    }

    // --- Search ---

    /// Search messages in a specific conversation using case-insensitive LIKE.
    /// Returns (sender, body, timestamp_ms, conversation_id, conversation_name) tuples,
    /// most recent first, limited to `limit` results.
    pub fn search_messages(
        &self,
        conv_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchRow>> {
        let escaped = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        let mut stmt = self.conn.prepare(
            "SELECT m.sender, m.body, m.timestamp_ms, c.id, c.name
             FROM messages m
             JOIN conversations c ON c.id = m.conversation_id
             WHERE m.conversation_id = ?1
               AND m.body LIKE ?2 ESCAPE '\\' COLLATE NOCASE
               AND m.is_system = 0
               AND m.is_deleted = 0
             ORDER BY m.timestamp_ms DESC
             LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(params![conv_id, pattern, limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Search messages across all conversations using case-insensitive LIKE.
    /// Returns (sender, body, timestamp_ms, conversation_id, conversation_name) tuples,
    /// most recent first, limited to `limit` results.
    pub fn search_all_messages(&self, query: &str, limit: usize) -> Result<Vec<SearchRow>> {
        let escaped = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        let mut stmt = self.conn.prepare(
            "SELECT m.sender, m.body, m.timestamp_ms, c.id, c.name
             FROM messages m
             JOIN conversations c ON c.id = m.conversation_id
             WHERE m.body LIKE ?1 ESCAPE '\\' COLLATE NOCASE
               AND m.is_system = 0
               AND m.is_deleted = 0
             ORDER BY m.timestamp_ms DESC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![pattern, limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Find the max rowid for messages up to (and including) a given timestamp.
    /// Uses the idx_messages_conv_ts_ms index for efficient lookup.
    pub fn max_rowid_up_to_timestamp(
        &self,
        conv_id: &str,
        timestamp_ms: i64,
    ) -> Result<Option<i64>> {
        let result = self.conn.query_row(
            "SELECT MAX(rowid) FROM messages WHERE conversation_id = ?1 AND timestamp_ms <= ?2",
            params![conv_id, timestamp_ms],
            |row| row.get::<_, Option<i64>>(0),
        )?;
        Ok(result)
    }

    // --- Muted conversations ---

    /// Persist a mute state. `None` unmutes the conversation.
    pub fn set_mute(&self, conv_id: &str, state: Option<MuteState>) -> Result<()> {
        let (muted, expires_str) = match state {
            None => (0, None),
            Some(MuteState::Permanent) => (1, None),
            Some(MuteState::Until(t)) => (1, Some(t.to_rfc3339())),
        };
        self.conn.execute(
            "UPDATE conversations SET muted = ?2, mute_expires_at = ?3 WHERE id = ?1",
            params![conv_id, muted, expires_str],
        )?;
        Ok(())
    }

    pub fn load_mutes(&self) -> Result<HashMap<String, MuteState>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, mute_expires_at FROM conversations WHERE muted = 1")?;
        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let expires: Option<String> = row.get(1)?;
                Ok((id, expires))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut map = HashMap::new();
        for (id, expires_str) in rows {
            let state = match expires_str.and_then(|s| DateTime::parse_from_rfc3339(&s).ok()) {
                Some(dt) => MuteState::Until(dt.with_timezone(&Utc)),
                None => MuteState::Permanent,
            };
            map.insert(id, state);
        }
        Ok(map)
    }

    /// Clear mutes whose expiry has passed. Returns the conversation IDs that were unmuted.
    pub fn clear_expired_mutes(&self, now: DateTime<Utc>) -> Result<Vec<String>> {
        let now_str = now.to_rfc3339();
        let mut stmt = self.conn.prepare(
            "UPDATE conversations SET muted = 0, mute_expires_at = NULL
             WHERE muted = 1 AND mute_expires_at IS NOT NULL AND mute_expires_at <= ?1
             RETURNING id",
        )?;
        let ids = stmt
            .query_map(params![now_str], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        Ok(ids)
    }

    // --- Blocked conversations ---

    /// Set or clear the archived flag on a conversation.
    pub fn set_archived(&self, conv_id: &str, archived: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE conversations SET archived = ?2 WHERE id = ?1",
            params![conv_id, archived as i32],
        )?;
        Ok(())
    }

    /// Move the read marker back so exactly the newest incoming message counts
    /// as unread again (#611). Returns false (and changes nothing) when the
    /// conversation has no incoming messages to mark.
    pub fn mark_unread(&self, conv_id: &str) -> Result<bool> {
        let last_incoming: Option<i64> = self.conn.query_row(
            "SELECT MAX(rowid) FROM messages
             WHERE conversation_id = ?1 AND is_system = 0
               AND status = 0 AND sender != 'you'",
            params![conv_id],
            |row| row.get(0),
        )?;
        let Some(last) = last_incoming else {
            return Ok(false);
        };
        let prev: Option<i64> = self.conn.query_row(
            "SELECT MAX(rowid) FROM messages
             WHERE conversation_id = ?1 AND is_system = 0
               AND status = 0 AND sender != 'you' AND rowid < ?2",
            params![conv_id, last],
            |row| row.get(0),
        )?;
        self.save_read_marker(conv_id, prev.unwrap_or(0))?;
        Ok(true)
    }

    pub fn set_blocked(&self, conv_id: &str, blocked: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE conversations SET blocked = ?2 WHERE id = ?1",
            params![conv_id, blocked as i32],
        )?;
        Ok(())
    }

    pub fn load_blocked(&self) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM conversations WHERE blocked = 1")?;
        let ids: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(ids.into_iter().collect())
    }

    // --- Disappearing messages ---

    pub fn update_expiration_timer(&self, conv_id: &str, seconds: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE conversations SET expiration_timer = ?2 WHERE id = ?1",
            params![conv_id, seconds],
        )?;
        Ok(())
    }

    pub fn delete_expired_messages(&self, now_ms: i64) -> Result<usize> {
        let deleted = self.conn.execute(
            "DELETE FROM messages WHERE expires_in_seconds > 0
             AND expiration_start_ms > 0
             AND (expiration_start_ms + expires_in_seconds * 1000) < ?1",
            params![now_ms],
        )?;
        Ok(deleted)
    }

    // --- Polls ---

    pub fn upsert_poll_data(
        &self,
        conv_id: &str,
        timestamp_ms: i64,
        poll_data: &PollData,
    ) -> Result<()> {
        let json = serde_json::to_string(poll_data)?;
        self.conn.execute(
            "UPDATE messages SET poll_data = ?3
             WHERE conversation_id = ?1 AND timestamp_ms = ?2",
            params![conv_id, timestamp_ms, json],
        )?;
        Ok(())
    }

    pub fn upsert_link_preview(
        &self,
        conv_id: &str,
        timestamp_ms: i64,
        preview: &LinkPreview,
    ) -> Result<()> {
        let json = serde_json::to_string(preview)?;
        self.conn.execute(
            "UPDATE messages SET link_preview = ?3
             WHERE conversation_id = ?1 AND timestamp_ms = ?2",
            params![conv_id, timestamp_ms, json],
        )?;
        Ok(())
    }

    /// Store the raw body (with U+FFFC placeholders) and raw mentions for a message,
    /// so later contact list updates can re-resolve the display body.
    pub fn upsert_message_mentions(
        &self,
        conv_id: &str,
        timestamp_ms: i64,
        body_raw: &str,
        mentions: &[Mention],
    ) -> Result<()> {
        let json = serde_json::to_string(mentions)?;
        self.conn.execute(
            "UPDATE messages SET body_raw = ?3, mentions_json = ?4
             WHERE conversation_id = ?1 AND timestamp_ms = ?2",
            params![conv_id, timestamp_ms, body_raw, json],
        )?;
        Ok(())
    }

    pub fn upsert_poll_vote(
        &self,
        conv_id: &str,
        poll_timestamp: i64,
        voter: &str,
        voter_name: Option<&str>,
        option_indexes: &[i64],
        vote_count: i64,
    ) -> Result<()> {
        let indexes_json = serde_json::to_string(option_indexes)?;
        self.conn.execute(
            "INSERT INTO poll_votes (conv_id, poll_timestamp, voter, voter_name, option_indexes, vote_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(conv_id, poll_timestamp, voter)
             DO UPDATE SET option_indexes = excluded.option_indexes, vote_count = excluded.vote_count, voter_name = excluded.voter_name",
            params![conv_id, poll_timestamp, voter, voter_name, indexes_json, vote_count],
        )?;
        Ok(())
    }

    pub fn load_poll_votes(&self, conv_id: &str, poll_timestamp: i64) -> Result<Vec<PollVote>> {
        let mut stmt = self.conn.prepare(
            "SELECT voter, voter_name, option_indexes, vote_count FROM poll_votes
             WHERE conv_id = ?1 AND poll_timestamp = ?2",
        )?;
        let rows: Vec<PollVote> = stmt
            .query_map(params![conv_id, poll_timestamp], |row| {
                let voter: String = row.get(0)?;
                let voter_name: Option<String> = row.get(1)?;
                let indexes_json: String = row.get(2)?;
                let vote_count: i64 = row.get(3)?;
                let option_indexes: Vec<i64> =
                    serde_json::from_str(&indexes_json).unwrap_or_default();
                Ok(PollVote {
                    voter,
                    voter_name,
                    option_indexes,
                    vote_count,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn close_poll(&self, conv_id: &str, poll_timestamp: i64) -> Result<()> {
        let poll_json: Option<String> = self
            .conn
            .query_row(
                "SELECT poll_data FROM messages WHERE conversation_id = ?1 AND timestamp_ms = ?2",
                params![conv_id, poll_timestamp],
                |row| row.get(0),
            )
            .ok()
            .flatten();
        if let Some(json_str) = poll_json
            && let Ok(mut poll_data) = serde_json::from_str::<PollData>(&json_str)
        {
            poll_data.closed = true;
            let updated = serde_json::to_string(&poll_data)?;
            self.conn.execute(
                "UPDATE messages SET poll_data = ?3
                     WHERE conversation_id = ?1 AND timestamp_ms = ?2",
                params![conv_id, poll_timestamp, updated],
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::{fixture, rstest};

    #[fixture]
    fn db() -> Database {
        Database::open_in_memory().unwrap()
    }

    // --- Native receive journal (#642 U11, KTD-2) ---

    /// Simulate the adapter side (a second connection appending) with a
    /// direct insert; the Database API only replays and deletes.
    fn journal_append(db: &Database, payload: &str) -> i64 {
        db.conn
            .execute(
                "INSERT INTO native_journal (payload) VALUES (?1)",
                params![payload],
            )
            .unwrap();
        db.conn.last_insert_rowid()
    }

    #[rstest]
    fn journal_pending_returns_rows_in_insertion_order(db: Database) {
        assert!(db.journal_pending().unwrap().is_empty());
        let first = journal_append(&db, "a");
        let second = journal_append(&db, "b");
        let rows = db.journal_pending().unwrap();
        assert_eq!(
            rows,
            vec![(first, "a".to_string()), (second, "b".to_string())]
        );
    }

    #[rstest]
    fn journal_delete_removes_exactly_one_row(db: Database) {
        let first = journal_append(&db, "a");
        let second = journal_append(&db, "b");
        db.journal_delete(first).unwrap();
        let rows = db.journal_pending().unwrap();
        assert_eq!(rows, vec![(second, "b".to_string())]);
        // Deleting an already-gone row is a no-op, not an error (replay
        // after a crash between delete and commit hits this).
        db.journal_delete(first).unwrap();
    }

    #[rstest]
    fn path_is_none_for_in_memory(db: Database) {
        assert_eq!(db.path(), None);
    }

    /// KTD-6 re-key plumbing (#642 U11 stage 3): rows move atomically, a
    /// row colliding with an identical twin under the target dedups away,
    /// and the SAVEPOINT nests correctly inside a drain batch transaction.
    #[rstest]
    fn merge_conversation_moves_dedups_and_nests_in_a_batch(db: Database) {
        let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let number = "+15551234567";
        db.upsert_conversation(uuid, uuid, false).unwrap();
        db.upsert_conversation(number, "Ada", false).unwrap();
        let insert = |conv: &str, ts_ms: i64, body: &str| {
            db.insert_message_full(
                conv,
                "Ada",
                "2026-07-24T12:00:00+00:00",
                body,
                false,
                None,
                ts_ms,
                uuid,
                None,
                None,
                None,
                0,
                0,
                0,
            )
            .unwrap()
        };
        // Unique message under the uuid conversation, plus one delivered on
        // BOTH sides of the re-key boundary (same dedup key either side).
        insert(uuid, 1000, "only under uuid").unwrap();
        insert(uuid, 2000, "delivered twice").unwrap();
        insert(number, 2000, "delivered twice").unwrap();

        db.begin_batch();
        db.merge_conversation(uuid, number).unwrap();
        db.commit_batch();

        let merged = db.load_messages_page(number, 50, 0).unwrap();
        assert_eq!(
            merged.len(),
            2,
            "unique row moved, colliding twin deduped away"
        );
        assert!(db.load_messages_page(uuid, 50, 0).unwrap().is_empty());
    }

    #[test]
    fn path_reports_file_backed_location() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.db");
        let db = Database::open(&file).unwrap();
        // SQLite reports the symlink-resolved absolute path; on macOS the
        // temp dir crosses the /var -> /private/var symlink, so compare
        // canonicalized forms rather than raw strings.
        assert_eq!(
            db.path().map(|p| p.canonicalize().unwrap()),
            Some(file.canonicalize().unwrap())
        );
    }

    /// Build an in-memory database migrated only up to schema version `k`, to
    /// simulate an existing user's database before a later release. Mirrors
    /// `Database::migrate`'s setup so a subsequent `migrate()` applies exactly
    /// the migrations above `k` against populated tables (#502).
    fn db_at_version(k: i32) -> Database {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        conn.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);")
            .unwrap();
        for m in MIGRATIONS.iter().filter(|m| m.version <= k) {
            conn.execute_batch(m.sql).unwrap();
        }
        Database { conn }
    }

    // #502: migrations were only ever tested against fresh databases. These
    // exercise the path every existing user hits on upgrade day: ALTER TABLE
    // backfills and new-table creation against rows that already exist.

    #[test]
    fn db_at_version_stops_at_requested_version() {
        let db = db_at_version(8);
        let v: i64 = db
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 8);
        // A column added after v8 must not exist yet.
        assert!(
            db.conn
                .query_row("SELECT blocked FROM conversations LIMIT 1", [], |_| Ok(()))
                .is_err()
        );
    }

    #[test]
    fn upgrade_from_v1_backfills_columns_and_preserves_rows() {
        let db = db_at_version(1);
        // Insert using only columns that exist at v1.
        db.conn
            .execute(
                "INSERT INTO conversations (id, name, is_group) VALUES ('+1', 'Alice', 0)",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO messages (conversation_id, sender, timestamp, body)
                 VALUES ('+1', 'Alice', '2025-01-01T00:00:00Z', 'hi')",
                [],
            )
            .unwrap();

        db.migrate().unwrap();

        // Schema reaches the latest version.
        let v: i64 = db
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
        // The existing row survives the upgrade.
        let msgs: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(msgs, 1);
        // NOT NULL DEFAULT columns added later are backfilled on the old row.
        let status: i64 = db
            .conn
            .query_row(
                "SELECT status FROM messages WHERE conversation_id='+1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, 0); // v3 default
        let (accepted, blocked): (i64, i64) = db
            .conn
            .query_row(
                "SELECT accepted, blocked FROM conversations WHERE id='+1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((accepted, blocked), (1, 0)); // v8 / v9 defaults
        // Tables added later exist and coexist with the old data.
        let reactions: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM reactions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(reactions, 0);
    }

    #[test]
    fn upgrade_from_v8_preserves_existing_values_and_loads() {
        let db = db_at_version(8);
        // accepted=0 to confirm a later migration does not reset it.
        db.conn
            .execute(
                "INSERT INTO conversations (id, name, is_group, accepted) VALUES ('+1', 'Alice', 0, 0)",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO messages (conversation_id, sender, timestamp, body, status, timestamp_ms)
                 VALUES ('+1', 'Alice', '2025-01-01T00:00:00Z', 'hi', 3, 1000)",
                [],
            )
            .unwrap();

        db.migrate().unwrap();

        let (accepted, blocked): (i64, i64) = db
            .conn
            .query_row(
                "SELECT accepted, blocked FROM conversations WHERE id='+1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((accepted, blocked), (0, 0)); // accepted preserved, blocked defaulted
        let mute: Option<String> = db
            .conn
            .query_row(
                "SELECT mute_expires_at FROM conversations WHERE id='+1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(mute.is_none()); // v14 column present, NULL on the old row
        // The real loader works against the upgraded schema (catches a
        // migration that forgot a column the code selects).
        let rows = db.load_messages_page("+1", 10, 0).unwrap();
        assert_eq!(rows.len(), 1);
    }

    // #484: your own sent messages are never "unread". Without the
    // incoming-only filter, sending N messages and restarting showed an
    // unread badge of N with the divider above your own messages.
    #[rstest]
    fn unread_count_excludes_outgoing_messages(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:00:00Z",
            "incoming",
            false,
            None,
            1000,
        )
        .unwrap();
        db.insert_message(
            "+1",
            "you",
            "2025-01-01T00:01:00Z",
            "outgoing",
            false,
            Some(MessageStatus::Sent),
            2000,
        )
        .unwrap();

        assert_eq!(db.unread_count("+1").unwrap(), 1);
    }

    // #489: drain_events wraps its writes in begin_batch/commit_batch. The
    // wrapper must be transparent (writes persist) and resilient to
    // unbalanced calls (a failed BEGIN means COMMIT runs with no open txn).
    #[rstest]
    fn batch_wrapped_writes_persist_and_unbalanced_calls_are_safe(db: Database) {
        db.begin_batch();
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:00:00Z",
            "hi",
            false,
            None,
            1000,
        )
        .unwrap();
        db.commit_batch();
        let msgs = db.load_messages_page("+1", 10, 0).unwrap();
        assert_eq!(msgs.len(), 1);

        // Unbalanced commit and empty batch must not panic or poison the conn
        db.commit_batch();
        db.begin_batch();
        db.commit_batch();
        assert_eq!(db.load_messages_page("+1", 10, 0).unwrap().len(), 1);
    }

    #[rstest]
    fn set_archived_round_trips_through_load(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.upsert_conversation("+2", "Bob", false).unwrap();
        db.set_archived("+1", true).unwrap();

        let convs = db.load_conversations(10).unwrap();
        let archived: Vec<bool> = {
            let mut by_id: Vec<(&str, bool)> =
                convs.iter().map(|c| (c.id.as_str(), c.archived)).collect();
            by_id.sort();
            by_id.into_iter().map(|(_, a)| a).collect()
        };
        assert_eq!(archived, vec![true, false]);

        db.set_archived("+1", false).unwrap();
        let convs = db.load_conversations(10).unwrap();
        assert!(convs.iter().all(|c| !c.archived));
    }

    #[rstest]
    fn mark_unread_restores_one_unread_incoming(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:00:00Z",
            "first",
            false,
            None,
            1000,
        )
        .unwrap();
        let last = db
            .insert_message(
                "+1",
                "Alice",
                "2025-01-01T00:01:00Z",
                "second",
                false,
                None,
                2000,
            )
            .unwrap();

        // Fully read, then mark unread: exactly the newest incoming counts again.
        db.save_read_marker("+1", last).unwrap();
        assert_eq!(db.unread_count("+1").unwrap(), 0);
        assert!(db.mark_unread("+1").unwrap());
        assert_eq!(db.unread_count("+1").unwrap(), 1);
    }

    #[rstest]
    fn mark_unread_without_incoming_returns_false(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        // Only an outgoing message: nothing can be marked unread.
        db.insert_message(
            "+1",
            "you",
            "2025-01-01T00:00:00Z",
            "sent",
            false,
            Some(MessageStatus::Sent),
            1000,
        )
        .unwrap();
        assert!(!db.mark_unread("+1").unwrap());
        assert_eq!(db.unread_count("+1").unwrap(), 0);
    }

    #[rstest]
    fn migration_creates_tables(db: Database) {
        // Should be able to query conversations table
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM conversations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    /// Regression guard: after migrate(), schema_version must match the last
    /// migration in the table. Catches the obvious "added a migration but
    /// forgot to append it" or "renumbered a version" mistakes.
    #[rstest]
    fn migrations_advance_schema_version_monotonically(db: Database) {
        let max_in_table: i32 = db
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        let expected = MIGRATIONS
            .last()
            .expect("MIGRATIONS must not be empty")
            .version;
        assert_eq!(max_in_table, expected);

        // Versions in the table must be strictly increasing and contiguous.
        let mut prev = 0;
        for migration in MIGRATIONS {
            assert_eq!(
                migration.version,
                prev + 1,
                "migration versions must be contiguous (got {} after {})",
                migration.version,
                prev
            );
            prev = migration.version;
        }
    }

    #[rstest]
    fn upsert_and_load_conversations(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.upsert_conversation("g1", "Family", true).unwrap();

        let convs = db.load_conversations(100).unwrap();
        assert_eq!(convs.len(), 2);
    }

    #[rstest]
    fn name_update_on_conflict(db: Database) {
        db.upsert_conversation("+1", "Unknown", false).unwrap();
        db.upsert_conversation("+1", "Alice", false).unwrap();

        let convs = db.load_conversations(100).unwrap();
        assert_eq!(convs.len(), 1);
        assert_eq!(convs[0].name, "Alice");
    }

    #[rstest]
    fn insert_and_load_messages(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:00:00Z",
            "hello",
            false,
            None,
            0,
        )
        .unwrap();
        db.insert_message("+1", "you", "2025-01-01T00:01:00Z", "hi!", false, None, 0)
            .unwrap();

        let convs = db.load_conversations(100).unwrap();
        assert_eq!(convs[0].messages.len(), 2);
        assert_eq!(convs[0].messages[0].body, "hello");
        assert_eq!(convs[0].messages[1].body, "hi!");
    }

    #[rstest]
    fn unread_count_with_read_markers(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        let r1 = db
            .insert_message(
                "+1",
                "Alice",
                "2025-01-01T00:00:00Z",
                "msg1",
                false,
                None,
                0,
            )
            .unwrap();
        db.insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:01:00Z",
            "msg2",
            false,
            None,
            0,
        )
        .unwrap();
        db.insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:02:00Z",
            "msg3",
            false,
            None,
            0,
        )
        .unwrap();

        // Mark first message as read
        db.save_read_marker("+1", r1).unwrap();
        assert_eq!(db.unread_count("+1").unwrap(), 2);
    }

    #[rstest]
    fn system_messages_excluded_from_unread(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.insert_message(
            "+1",
            "",
            "2025-01-01T00:00:00Z",
            "system msg",
            true,
            None,
            0,
        )
        .unwrap();
        db.insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:01:00Z",
            "real msg",
            false,
            None,
            0,
        )
        .unwrap();

        // No read marker → only non-system messages count as unread
        assert_eq!(db.unread_count("+1").unwrap(), 1);
    }

    #[rstest]
    fn conversation_order(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.upsert_conversation("+2", "Bob", false).unwrap();
        // Alice gets an older message, Bob gets a newer one
        db.insert_message("+1", "Alice", "2025-01-01T00:00:00Z", "old", false, None, 0)
            .unwrap();
        db.insert_message("+2", "Bob", "2025-01-02T00:00:00Z", "new", false, None, 0)
            .unwrap();

        let order = db.load_conversation_order().unwrap();
        // Most recent message first
        assert_eq!(order[0], "+2");
        assert_eq!(order[1], "+1");
    }

    // --- Boolean flag round-trip: blocked ---

    #[rstest]
    fn blocked_flag_round_trip(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.upsert_conversation("+2", "Bob", false).unwrap();

        db.set_blocked("+1", true).unwrap();
        let set = db.load_blocked().unwrap();
        assert!(set.contains("+1"));
        assert!(!set.contains("+2"));

        db.set_blocked("+1", false).unwrap();
        let set = db.load_blocked().unwrap();
        assert!(!set.contains("+1"));
    }

    // --- Muted flag round-trips ---

    #[rstest]
    fn permanent_mute_round_trip(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.upsert_conversation("+2", "Bob", false).unwrap();

        db.set_mute("+1", Some(MuteState::Permanent)).unwrap();
        let map = db.load_mutes().unwrap();
        assert_eq!(map.get("+1"), Some(&MuteState::Permanent));
        assert!(!map.contains_key("+2"));

        db.set_mute("+1", None).unwrap();
        let map = db.load_mutes().unwrap();
        assert!(!map.contains_key("+1"));
    }

    #[rstest]
    fn timed_mute_round_trip(db: Database) {
        use chrono::TimeZone;
        db.upsert_conversation("+1", "Alice", false).unwrap();

        let expiry = Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap();
        db.set_mute("+1", Some(MuteState::Until(expiry))).unwrap();
        let map = db.load_mutes().unwrap();
        assert_eq!(map["+1"], MuteState::Until(expiry));
    }

    #[rstest]
    fn clear_expired_mutes_clears_past(db: Database) {
        use chrono::TimeZone;
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.upsert_conversation("+2", "Bob", false).unwrap();

        let past = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let future = Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();

        // +1 has expired timed mute, +2 has future timed mute
        db.set_mute("+1", Some(MuteState::Until(past))).unwrap();
        db.set_mute("+2", Some(MuteState::Until(future))).unwrap();

        let cleared = db.clear_expired_mutes(now).unwrap();
        assert_eq!(cleared, vec!["+1"]);

        let map = db.load_mutes().unwrap();
        assert!(!map.contains_key("+1")); // cleared
        assert!(map.contains_key("+2")); // still muted
    }

    #[rstest]
    fn clear_expired_mutes_skips_permanent(db: Database) {
        use chrono::TimeZone;
        db.upsert_conversation("+1", "Alice", false).unwrap();

        db.set_mute("+1", Some(MuteState::Permanent)).unwrap();

        let now = Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap();
        let cleared = db.clear_expired_mutes(now).unwrap();
        assert!(cleared.is_empty());

        let map = db.load_mutes().unwrap();
        assert!(map.contains_key("+1")); // still muted
    }

    #[rstest]
    fn last_message_rowid(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();

        assert_eq!(db.last_message_rowid("+1").unwrap(), None);

        db.insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:00:00Z",
            "msg1",
            false,
            None,
            0,
        )
        .unwrap();
        let r2 = db
            .insert_message(
                "+1",
                "Alice",
                "2025-01-01T00:01:00Z",
                "msg2",
                false,
                None,
                0,
            )
            .unwrap();

        assert_eq!(db.last_message_rowid("+1").unwrap(), Some(r2));
    }

    #[rstest]
    fn max_rowid_up_to_timestamp(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();

        // No messages → None
        assert_eq!(db.max_rowid_up_to_timestamp("+1", 5000).unwrap(), None);

        let r1 = db
            .insert_message(
                "+1",
                "Alice",
                "2025-01-01T00:00:00Z",
                "msg1",
                false,
                None,
                1000,
            )
            .unwrap();
        let r2 = db
            .insert_message(
                "+1",
                "Alice",
                "2025-01-01T00:01:00Z",
                "msg2",
                false,
                None,
                2000,
            )
            .unwrap();
        let _r3 = db
            .insert_message(
                "+1",
                "Alice",
                "2025-01-01T00:02:00Z",
                "msg3",
                false,
                None,
                3000,
            )
            .unwrap();

        // Timestamp before all messages → None
        assert_eq!(db.max_rowid_up_to_timestamp("+1", 500).unwrap(), None);

        // Timestamp matching first message
        assert_eq!(db.max_rowid_up_to_timestamp("+1", 1000).unwrap(), Some(r1));

        // Timestamp matching second message
        assert_eq!(db.max_rowid_up_to_timestamp("+1", 2000).unwrap(), Some(r2));

        // Timestamp between second and third
        assert_eq!(db.max_rowid_up_to_timestamp("+1", 2500).unwrap(), Some(r2));
    }

    #[rstest]
    fn load_messages_page_pagination(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        for i in 0..5 {
            db.insert_message(
                "+1",
                "Alice",
                &format!("2025-01-01T00:0{i}:00Z"),
                &format!("msg{i}"),
                false,
                None,
                i * 1000,
            )
            .unwrap();
        }

        // Load first page (most recent 3)
        let page1 = db.load_messages_page("+1", 3, 0).unwrap();
        assert_eq!(page1.len(), 3);
        assert_eq!(page1[0].body, "msg2"); // oldest of the 3 most recent
        assert_eq!(page1[2].body, "msg4"); // newest

        // Load second page (next 2 older)
        let page2 = db.load_messages_page("+1", 3, 3).unwrap();
        assert_eq!(page2.len(), 2);
        assert_eq!(page2[0].body, "msg0"); // oldest overall
        assert_eq!(page2[1].body, "msg1");

        // Load third page (nothing left)
        let page3 = db.load_messages_page("+1", 3, 5).unwrap();
        assert!(page3.is_empty());
    }

    #[rstest]
    fn migration_v4_creates_reactions_table(db: Database) {
        // Should be able to query reactions table
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM reactions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[rstest]
    fn upsert_reaction_insert_and_replace(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:00:00Z",
            "hello",
            false,
            None,
            1000,
        )
        .unwrap();

        // Insert a reaction
        db.upsert_reaction("+1", 1000, "Alice", "Bob", "👍")
            .unwrap();
        let reactions = db.load_reactions("+1").unwrap();
        assert_eq!(reactions.len(), 1);
        assert_eq!(
            reactions[0],
            (
                1000,
                "Alice".to_string(),
                "👍".to_string(),
                "Bob".to_string()
            )
        );

        // Replace: same sender reacts with different emoji
        db.upsert_reaction("+1", 1000, "Alice", "Bob", "❤️")
            .unwrap();
        let reactions = db.load_reactions("+1").unwrap();
        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0].2, "❤️");
    }

    #[rstest]
    fn remove_reaction(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();

        db.upsert_reaction("+1", 1000, "Alice", "Bob", "👍")
            .unwrap();
        assert_eq!(db.load_reactions("+1").unwrap().len(), 1);

        db.remove_reaction("+1", 1000, "Alice", "Bob").unwrap();
        assert_eq!(db.load_reactions("+1").unwrap().len(), 0);
    }

    #[rstest]
    fn load_reactions_attaches_to_messages(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:00:00Z",
            "hello",
            false,
            None,
            1000,
        )
        .unwrap();
        db.insert_message("+1", "you", "2025-01-01T00:01:00Z", "hi", false, None, 2000)
            .unwrap();

        db.upsert_reaction("+1", 1000, "Alice", "Bob", "👍")
            .unwrap();
        db.upsert_reaction("+1", 2000, "you", "Alice", "❤️")
            .unwrap();

        let convs = db.load_conversations(100).unwrap();
        assert_eq!(convs[0].messages[0].reactions.len(), 1);
        assert_eq!(convs[0].messages[0].reactions[0].emoji, "👍");
        assert_eq!(convs[0].messages[1].reactions.len(), 1);
        assert_eq!(convs[0].messages[1].reactions[0].emoji, "❤️");
    }

    #[rstest]
    fn search_messages_in_conversation(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:00:00Z",
            "hello world",
            false,
            None,
            1000,
        )
        .unwrap();
        db.insert_message(
            "+1",
            "you",
            "2025-01-01T00:01:00Z",
            "hi there",
            false,
            None,
            2000,
        )
        .unwrap();
        db.insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:02:00Z",
            "Hello again",
            false,
            None,
            3000,
        )
        .unwrap();

        // Case-insensitive search for "hello"
        let results = db.search_messages("+1", "hello", 50).unwrap();
        assert_eq!(results.len(), 2);
        // Most recent first
        assert_eq!(results[0].1, "Hello again");
        assert_eq!(results[1].1, "hello world");
    }

    #[rstest]
    fn search_messages_excludes_system_and_deleted(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.insert_message(
            "+1",
            "",
            "2025-01-01T00:00:00Z",
            "system hello",
            true,
            None,
            1000,
        )
        .unwrap();
        db.insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:01:00Z",
            "real hello",
            false,
            None,
            2000,
        )
        .unwrap();

        let results = db.search_messages("+1", "hello", 50).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, "real hello");
    }

    #[rstest]
    fn migration_v8_defaults_accepted_to_1(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        let convs = db.load_conversations(100).unwrap();
        assert!(convs[0].accepted);
    }

    #[rstest]
    fn update_accepted_round_trip(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.update_accepted("+1", false).unwrap();
        let convs = db.load_conversations(100).unwrap();
        assert!(!convs[0].accepted);

        db.update_accepted("+1", true).unwrap();
        let convs = db.load_conversations(100).unwrap();
        assert!(convs[0].accepted);
    }

    #[rstest]
    fn delete_conversation_removes_all_data(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:00:00Z",
            "hello",
            false,
            None,
            1000,
        )
        .unwrap();
        db.upsert_reaction("+1", 1000, "Alice", "Bob", "👍")
            .unwrap();
        db.save_read_marker("+1", 1).unwrap();

        db.delete_conversation("+1").unwrap();

        let convs = db.load_conversations(100).unwrap();
        assert!(convs.is_empty());
        assert_eq!(db.load_reactions("+1").unwrap().len(), 0);
    }

    #[rstest]
    fn migration_v9_defaults_blocked_to_0(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        let blocked = db.load_blocked().unwrap();
        assert!(!blocked.contains("+1"));
    }

    #[rstest]
    fn search_all_messages_across_conversations(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.upsert_conversation("+2", "Bob", false).unwrap();
        db.insert_message(
            "+1",
            "Alice",
            "2025-01-01T00:00:00Z",
            "hello from alice",
            false,
            None,
            1000,
        )
        .unwrap();
        db.insert_message(
            "+2",
            "Bob",
            "2025-01-01T00:01:00Z",
            "hello from bob",
            false,
            None,
            2000,
        )
        .unwrap();

        let results = db.search_all_messages("hello", 50).unwrap();
        assert_eq!(results.len(), 2);
        // Most recent first
        assert_eq!(results[0].3, "+2"); // Bob's conversation
        assert_eq!(results[1].3, "+1"); // Alice's conversation
    }

    // --- U6 (#640): incoming-message dedup index ---

    // Upgrade-day path for the v16 migration: a populated messages table with
    // a legitimate multi-entry message (body + attachment rows share conv,
    // sender_id, and timestamp) AND a pre-existing true duplicate. The
    // migration must build the index without deleting anything: entry_seq
    // backfills by rowid, making every existing row unique under the new key.
    #[test]
    fn upgrade_from_v15_backfills_entry_seq_and_builds_dedup_index() {
        let db = db_at_version(15);
        db.conn
            .execute(
                "INSERT INTO conversations (id, name, is_group) VALUES ('+1', 'Alice', 0)",
                [],
            )
            .unwrap();
        for body in ["hello", "[attachment: photo.jpg]", "hello"] {
            // Third row simulates a historical replay of the body row.
            db.conn
                .execute(
                    "INSERT INTO messages (conversation_id, sender, timestamp, body, sender_id, timestamp_ms)
                     VALUES ('+1', 'Alice', '2026-01-01T00:00:00Z', ?1, '+1', 1000)",
                    params![body],
                )
                .unwrap();
        }

        db.migrate().unwrap();

        // All three rows survive (a wrong dedup is unrecoverable; historical
        // duplicates are only cosmetic), numbered by insertion order.
        let seqs: Vec<i64> = db
            .conn
            .prepare("SELECT entry_seq FROM messages ORDER BY rowid")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(seqs, vec![0, 1, 2]);

        // The index now enforces idempotency: a post-migration replay of the
        // first entry is conflict-skipped, not inserted and not an error.
        let replay = db
            .insert_message_full(
                "+1",
                "Alice",
                "2026-01-01T00:00:00Z",
                "hello",
                false,
                None,
                1000,
                "+1",
                None,
                None,
                None,
                0,
                0,
                0,
            )
            .unwrap();
        assert!(replay.is_none(), "replayed entry must be conflict-skipped");
        // A genuinely new entry_seq still inserts.
        let new_entry = db
            .insert_message_full(
                "+1",
                "Alice",
                "2026-01-01T00:00:00Z",
                "[attachment: second.jpg]",
                false,
                None,
                1000,
                "+1",
                None,
                None,
                None,
                0,
                0,
                3,
            )
            .unwrap();
        assert!(new_entry.is_some());
    }

    // Outgoing rows stay OUTSIDE the dedup index so the #480 send-confirm
    // timestamp rewrite can land on a timestamp an incoming row already
    // occupies (sender-chosen clocks collide across senders routinely).
    #[rstest]
    fn outgoing_rewrite_onto_incoming_timestamp_succeeds(db: Database) {
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.insert_message_full(
            "+1", "Alice", "t", "incoming", false, None, 5000, "+1", None, None, None, 0, 0, 0,
        )
        .unwrap();
        // Outgoing row at a local timestamp, then rewritten to 5000.
        db.insert_message(
            "+1",
            "you",
            "t",
            "mine",
            false,
            Some(MessageStatus::Sending),
            4900,
        )
        .unwrap();
        db.update_message_timestamp_ms("+1", 4900, 5000, MessageStatus::Sent.to_i32())
            .unwrap();
        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE timestamp_ms = 5000",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2, "incoming and rewritten outgoing rows coexist");
    }

    // Connection topology settled by U6: a second connection (the future
    // native adapter's durable-insert path, #640 U11) inserting while the
    // main connection holds an open drain batch must queue on busy_timeout
    // and succeed once the batch commits -- never error with SQLITE_BUSY.
    #[test]
    fn adapter_connection_insert_queues_behind_main_batch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dedup-test.db");
        let main_db = Database::open(&path).unwrap();
        main_db.upsert_conversation("+1", "Alice", false).unwrap();
        let adapter_db = Database::open(&path).unwrap();

        main_db.begin_batch();
        main_db
            .insert_message("+1", "Alice", "t", "from main", false, None, 1000)
            .unwrap();

        let handle = std::thread::spawn(move || {
            adapter_db.insert_message_full(
                "+1",
                "Bob",
                "t",
                "from adapter",
                false,
                None,
                2000,
                "+2",
                None,
                None,
                None,
                0,
                0,
                0,
            )
        });
        // Let the adapter thread hit the write lock, then release it.
        std::thread::sleep(std::time::Duration::from_millis(100));
        main_db.commit_batch();

        let inserted = handle.join().unwrap().unwrap();
        assert!(
            inserted.is_some(),
            "adapter insert must succeed after the batch commits"
        );
        let count: i64 = main_db
            .conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}
