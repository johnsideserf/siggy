use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use crate::app::{Conversation, DisplayMessage};
use crate::signal::types::{LinkPreview, MessageStatus, PollData, PollVote, Reaction};

/// (sender, body, timestamp_ms, conversation_id, conversation_name)
pub type SearchRow = (String, String, i64, String, String);

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        conn.execute_batch("PRAGMA secure_delete=ON;")?;
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

    fn migrate(&self) -> Result<()> {
        // Create schema_version table if it doesn't exist
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);",
        )?;

        let version: i32 = self.conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )?;

        if version < 1 {
            self.conn.execute_batch(
                "
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
            )?;
        }

        if version < 2 {
            self.conn.execute_batch(
                "
                BEGIN;
                ALTER TABLE conversations ADD COLUMN muted INTEGER NOT NULL DEFAULT 0;
                UPDATE schema_version SET version = 2;
                COMMIT;
                ",
            )?;
        }

        if version < 3 {
            self.conn.execute_batch(
                "
                BEGIN;
                ALTER TABLE messages ADD COLUMN status INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE messages ADD COLUMN timestamp_ms INTEGER NOT NULL DEFAULT 0;
                UPDATE schema_version SET version = 3;
                COMMIT;
                ",
            )?;
        }

        if version < 4 {
            self.conn.execute_batch(
                "
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
            )?;
        }

        if version < 5 {
            self.conn.execute_batch(
                "
                BEGIN;
                CREATE INDEX IF NOT EXISTS idx_messages_conv_ts_ms ON messages(conversation_id, timestamp_ms);
                UPDATE schema_version SET version = 5;
                COMMIT;
                ",
            )?;
        }

        if version < 6 {
            self.conn.execute_batch(
                "
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
            )?;
        }

        if version < 7 {
            self.conn.execute_batch(
                "
                BEGIN;
                ALTER TABLE conversations ADD COLUMN expiration_timer INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE messages ADD COLUMN expires_in_seconds INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE messages ADD COLUMN expiration_start_ms INTEGER NOT NULL DEFAULT 0;
                UPDATE schema_version SET version = 7;
                COMMIT;
                ",
            )?;
        }

        if version < 8 {
            self.conn.execute_batch(
                "
                BEGIN;
                ALTER TABLE conversations ADD COLUMN accepted INTEGER NOT NULL DEFAULT 1;
                UPDATE schema_version SET version = 8;
                COMMIT;
                ",
            )?;
        }

        if version < 9 {
            self.conn.execute_batch(
                "
                BEGIN;
                ALTER TABLE conversations ADD COLUMN blocked INTEGER NOT NULL DEFAULT 0;
                UPDATE schema_version SET version = 9;
                COMMIT;
                ",
            )?;
        }

        if version < 10 {
            self.conn.execute_batch(
                "
                BEGIN;
                ALTER TABLE messages ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
                UPDATE schema_version SET version = 10;
                COMMIT;
                ",
            )?;
        }

        if version < 11 {
            self.conn.execute_batch(
                "
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
            )?;
        }

        if version < 12 {
            self.conn.execute_batch(
                "
                BEGIN;
                ALTER TABLE messages ADD COLUMN link_preview TEXT;
                UPDATE schema_version SET version = 12;
                COMMIT;
                ",
            )?;
        }

        if version < 13 {
            self.conn.execute_batch(
                "
                BEGIN;
                ALTER TABLE conversations ADD COLUMN mute_expires_at TEXT;
                UPDATE schema_version SET version = 13;
                COMMIT;
                ",
            )?;
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
            "SELECT sender, timestamp, body, is_system, status, timestamp_ms, is_edited, is_deleted, quote_author, quote_body, quote_ts_ms, sender_id, expires_in_seconds, expiration_start_ms, pinned, poll_data, link_preview FROM messages
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
        if let Ok(reactions) = self.load_reactions(conv_id) {
            for (target_ts, target_author, emoji, sender) in reactions {
                let idx = ts_to_idx.get(&target_ts).and_then(|idxs| {
                    idxs.iter()
                        .find(|&&i| {
                            messages[i].sender == target_author || messages[i].sender == "you"
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
            if msg.poll_data.is_some() {
                if let Ok(votes) = self.load_poll_votes(conv_id, msg.timestamp_ms) {
                    msg.poll_votes = votes;
                }
            }
        }

        Ok(messages)
    }

    /// Load all conversations with their most recent messages (up to `msg_limit`).
    pub fn load_conversations(&self, msg_limit: usize) -> Result<Vec<Conversation>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, is_group, expiration_timer, accepted FROM conversations")?;

        let convs: Vec<(String, String, bool, i64, bool)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)? != 0,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i32>(4)? != 0,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut result = Vec::with_capacity(convs.len());

        for (id, name, is_group, expiration_timer, accepted) in convs {
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
        self.insert_message_full(
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
        )
    }

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
    ) -> Result<i64> {
        let status_i32 = status.map(|s| s.to_i32()).unwrap_or(0);
        self.conn.execute(
            "INSERT INTO messages (conversation_id, sender, timestamp, body, is_system, status, timestamp_ms, sender_id, quote_author, quote_body, quote_ts_ms, expires_in_seconds, expiration_start_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![conv_id, sender, timestamp, body, is_system as i32, status_i32, timestamp_ms, sender_id, quote_author, quote_body, quote_ts_ms, expires_in_seconds, expiration_start_ms],
        )?;
        Ok(self.conn.last_insert_rowid())
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

        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM messages
             WHERE conversation_id = ?1 AND rowid > ?2 AND is_system = 0",
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

    /// Load all reactions for a conversation.
    /// Returns (target_ts_ms, target_author, emoji, sender) tuples.
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

    pub fn set_muted(
        &self,
        conv_id: &str,
        muted: bool,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        let expires_str = if muted {
            expires_at.map(|t| t.to_rfc3339())
        } else {
            None
        };
        self.conn.execute(
            "UPDATE conversations SET muted = ?2, mute_expires_at = ?3 WHERE id = ?1",
            params![conv_id, muted as i32, expires_str],
        )?;
        Ok(())
    }

    pub fn load_muted(&self) -> Result<HashMap<String, Option<DateTime<Utc>>>> {
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
            let expires = expires_str
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));
            map.insert(id, expires);
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
        if let Some(json_str) = poll_json {
            if let Ok(mut poll_data) = serde_json::from_str::<PollData>(&json_str) {
                poll_data.closed = true;
                let updated = serde_json::to_string(&poll_data)?;
                self.conn.execute(
                    "UPDATE messages SET poll_data = ?3
                     WHERE conversation_id = ?1 AND timestamp_ms = ?2",
                    params![conv_id, poll_timestamp, updated],
                )?;
            }
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

    #[rstest]
    fn migration_creates_tables(db: Database) {
        // Should be able to query conversations table
        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM conversations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
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

        db.set_muted("+1", true, None).unwrap();
        let map = db.load_muted().unwrap();
        assert!(map.contains_key("+1"));
        assert_eq!(map["+1"], None); // permanent mute
        assert!(!map.contains_key("+2"));

        db.set_muted("+1", false, None).unwrap();
        let map = db.load_muted().unwrap();
        assert!(!map.contains_key("+1"));
    }

    #[rstest]
    fn timed_mute_round_trip(db: Database) {
        use chrono::TimeZone;
        db.upsert_conversation("+1", "Alice", false).unwrap();

        let expiry = Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap();
        db.set_muted("+1", true, Some(expiry)).unwrap();
        let map = db.load_muted().unwrap();
        assert_eq!(map["+1"], Some(expiry));
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
        db.set_muted("+1", true, Some(past)).unwrap();
        db.set_muted("+2", true, Some(future)).unwrap();

        let cleared = db.clear_expired_mutes(now).unwrap();
        assert_eq!(cleared, vec!["+1"]);

        let map = db.load_muted().unwrap();
        assert!(!map.contains_key("+1")); // cleared
        assert!(map.contains_key("+2")); // still muted
    }

    #[rstest]
    fn clear_expired_mutes_skips_permanent(db: Database) {
        use chrono::TimeZone;
        db.upsert_conversation("+1", "Alice", false).unwrap();

        // permanent mute (no expiry)
        db.set_muted("+1", true, None).unwrap();

        let now = Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap();
        let cleared = db.clear_expired_mutes(now).unwrap();
        assert!(cleared.is_empty());

        let map = db.load_muted().unwrap();
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
}
