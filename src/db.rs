use std::path::Path;

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::app::{Conversation, DisplayMessage};
use crate::signal::types::{MessageStatus, Reaction};

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

        let version: i32 = self
            .conn
            .query_row(
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

    /// Load all conversations with their most recent messages (up to `msg_limit`).
    pub fn load_conversations(&self, msg_limit: usize) -> Result<Vec<Conversation>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, is_group, expiration_timer FROM conversations")?;

        let convs: Vec<(String, String, bool, i64)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)? != 0,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut result = Vec::with_capacity(convs.len());

        for (id, name, is_group, expiration_timer) in convs {
            // Load last N messages
            let mut msg_stmt = self.conn.prepare(
                "SELECT sender, timestamp, body, is_system, status, timestamp_ms, is_edited, is_deleted, quote_author, quote_body, quote_ts_ms, sender_id, expires_in_seconds, expiration_start_ms FROM messages
                 WHERE conversation_id = ?1
                 ORDER BY timestamp_ms DESC, rowid DESC LIMIT ?2",
            )?;

            let mut messages: Vec<DisplayMessage> = msg_stmt
                .query_map(params![id, msg_limit as i64], |row| {
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
                    Ok((sender, ts_str, body, is_system, status_i32, timestamp_ms, is_edited, is_deleted, quote_author, quote_body, quote_ts_ms, sender_id, expires_in_seconds, expiration_start_ms))
                })?
                .filter_map(|r| r.ok())
                .filter_map(|(sender, ts_str, body, is_system, status_i32, timestamp_ms, is_edited, is_deleted, quote_author, quote_body, quote_ts_ms, sender_id, expires_in_seconds, expiration_start_ms)| {
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
                        sender_id,
                        expires_in_seconds,
                        expiration_start_ms,
                    })
                })
                .collect();

            // Reverse so oldest first
            messages.reverse();

            // Attach reactions from DB to matching messages.
            // Match on timestamp AND author when possible. Since msg.sender may be
            // a display name while target_author is a phone number, we accept:
            // exact match, msg.sender == "you", or fall back to timestamp-only.
            if let Ok(reactions) = self.load_reactions(&id) {
                for (target_ts, target_author, emoji, sender) in reactions {
                    // Find best match: prefer author+timestamp, fall back to timestamp-only
                    let idx = messages.iter().position(|m| {
                        m.timestamp_ms == target_ts
                            && (m.sender == target_author || m.sender == "you")
                    }).or_else(|| {
                        messages.iter().position(|m| m.timestamp_ms == target_ts)
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

            let unread = self.unread_count(&id).unwrap_or(0);

            result.push(Conversation {
                name,
                id: id.clone(),
                messages,
                unread,
                is_group,
                expiration_timer,
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
        self.insert_message_full(conv_id, sender, timestamp, body, is_system, status, timestamp_ms, "", None, None, None)
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
    ) -> Result<i64> {
        let status_i32 = status.map(|s| s.to_i32()).unwrap_or(0);
        self.conn.execute(
            "INSERT INTO messages (conversation_id, sender, timestamp, body, is_system, status, timestamp_ms, sender_id, quote_author, quote_body, quote_ts_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![conv_id, sender, timestamp, body, is_system as i32, status_i32, timestamp_ms, sender_id, quote_author, quote_body, quote_ts_ms],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Update delivery status for an outgoing message by its ms epoch timestamp.
    pub fn update_message_status(&self, conv_id: &str, timestamp_ms: i64, status: i32) -> Result<()> {
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
        let last_read: i64 = self
            .conn
            .query_row(
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
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                ))
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
        let escaped = query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
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
    pub fn search_all_messages(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchRow>> {
        let escaped = query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
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
    pub fn max_rowid_up_to_timestamp(&self, conv_id: &str, timestamp_ms: i64) -> Result<Option<i64>> {
        let result = self.conn.query_row(
            "SELECT MAX(rowid) FROM messages WHERE conversation_id = ?1 AND timestamp_ms <= ?2",
            params![conv_id, timestamp_ms],
            |row| row.get::<_, Option<i64>>(0),
        )?;
        Ok(result)
    }

    // --- Muted conversations ---

    pub fn set_muted(&self, conv_id: &str, muted: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE conversations SET muted = ?2 WHERE id = ?1",
            params![conv_id, muted as i32],
        )?;
        Ok(())
    }

    pub fn load_muted(&self) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM conversations WHERE muted = 1",
        )?;
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

    #[allow(dead_code)]
    pub fn load_expiration_timers(&self) -> Result<std::collections::HashMap<String, i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, expiration_timer FROM conversations WHERE expiration_timer > 0",
        )?;
        let rows: Vec<(String, i64)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().collect())
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

    #[allow(clippy::too_many_arguments)]
    pub fn insert_message_with_expiry(
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        Database::open_in_memory().unwrap()
    }

    #[test]
    fn migration_creates_tables() {
        let db = test_db();
        // Should be able to query conversations table
        let count: i64 = db.conn.query_row(
            "SELECT COUNT(*) FROM conversations", [], |row| row.get(0),
        ).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn upsert_and_load_conversations() {
        let db = test_db();
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.upsert_conversation("g1", "Family", true).unwrap();

        let convs = db.load_conversations(100).unwrap();
        assert_eq!(convs.len(), 2);
    }

    #[test]
    fn name_update_on_conflict() {
        let db = test_db();
        db.upsert_conversation("+1", "Unknown", false).unwrap();
        db.upsert_conversation("+1", "Alice", false).unwrap();

        let convs = db.load_conversations(100).unwrap();
        assert_eq!(convs.len(), 1);
        assert_eq!(convs[0].name, "Alice");
    }

    #[test]
    fn insert_and_load_messages() {
        let db = test_db();
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.insert_message("+1", "Alice", "2025-01-01T00:00:00Z", "hello", false, None, 0).unwrap();
        db.insert_message("+1", "you", "2025-01-01T00:01:00Z", "hi!", false, None, 0).unwrap();

        let convs = db.load_conversations(100).unwrap();
        assert_eq!(convs[0].messages.len(), 2);
        assert_eq!(convs[0].messages[0].body, "hello");
        assert_eq!(convs[0].messages[1].body, "hi!");
    }

    #[test]
    fn unread_count_with_read_markers() {
        let db = test_db();
        db.upsert_conversation("+1", "Alice", false).unwrap();
        let r1 = db.insert_message("+1", "Alice", "2025-01-01T00:00:00Z", "msg1", false, None, 0).unwrap();
        db.insert_message("+1", "Alice", "2025-01-01T00:01:00Z", "msg2", false, None, 0).unwrap();
        db.insert_message("+1", "Alice", "2025-01-01T00:02:00Z", "msg3", false, None, 0).unwrap();

        // Mark first message as read
        db.save_read_marker("+1", r1).unwrap();
        assert_eq!(db.unread_count("+1").unwrap(), 2);
    }

    #[test]
    fn system_messages_excluded_from_unread() {
        let db = test_db();
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.insert_message("+1", "", "2025-01-01T00:00:00Z", "system msg", true, None, 0).unwrap();
        db.insert_message("+1", "Alice", "2025-01-01T00:01:00Z", "real msg", false, None, 0).unwrap();

        // No read marker → only non-system messages count as unread
        assert_eq!(db.unread_count("+1").unwrap(), 1);
    }

    #[test]
    fn conversation_order() {
        let db = test_db();
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.upsert_conversation("+2", "Bob", false).unwrap();
        // Alice gets an older message, Bob gets a newer one
        db.insert_message("+1", "Alice", "2025-01-01T00:00:00Z", "old", false, None, 0).unwrap();
        db.insert_message("+2", "Bob", "2025-01-02T00:00:00Z", "new", false, None, 0).unwrap();

        let order = db.load_conversation_order().unwrap();
        // Most recent message first
        assert_eq!(order[0], "+2");
        assert_eq!(order[1], "+1");
    }

    #[test]
    fn mute_round_trip() {
        let db = test_db();
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.upsert_conversation("+2", "Bob", false).unwrap();

        db.set_muted("+1", true).unwrap();
        let muted = db.load_muted().unwrap();
        assert!(muted.contains("+1"));
        assert!(!muted.contains("+2"));

        db.set_muted("+1", false).unwrap();
        let muted = db.load_muted().unwrap();
        assert!(!muted.contains("+1"));
    }

    #[test]
    fn last_message_rowid() {
        let db = test_db();
        db.upsert_conversation("+1", "Alice", false).unwrap();

        assert_eq!(db.last_message_rowid("+1").unwrap(), None);

        db.insert_message("+1", "Alice", "2025-01-01T00:00:00Z", "msg1", false, None, 0).unwrap();
        let r2 = db.insert_message("+1", "Alice", "2025-01-01T00:01:00Z", "msg2", false, None, 0).unwrap();

        assert_eq!(db.last_message_rowid("+1").unwrap(), Some(r2));
    }

    #[test]
    fn max_rowid_up_to_timestamp() {
        let db = test_db();
        db.upsert_conversation("+1", "Alice", false).unwrap();

        // No messages → None
        assert_eq!(db.max_rowid_up_to_timestamp("+1", 5000).unwrap(), None);

        let r1 = db.insert_message("+1", "Alice", "2025-01-01T00:00:00Z", "msg1", false, None, 1000).unwrap();
        let r2 = db.insert_message("+1", "Alice", "2025-01-01T00:01:00Z", "msg2", false, None, 2000).unwrap();
        let _r3 = db.insert_message("+1", "Alice", "2025-01-01T00:02:00Z", "msg3", false, None, 3000).unwrap();

        // Timestamp before all messages → None
        assert_eq!(db.max_rowid_up_to_timestamp("+1", 500).unwrap(), None);

        // Timestamp matching first message
        assert_eq!(db.max_rowid_up_to_timestamp("+1", 1000).unwrap(), Some(r1));

        // Timestamp matching second message
        assert_eq!(db.max_rowid_up_to_timestamp("+1", 2000).unwrap(), Some(r2));

        // Timestamp between second and third
        assert_eq!(db.max_rowid_up_to_timestamp("+1", 2500).unwrap(), Some(r2));
    }

    #[test]
    fn migration_v4_creates_reactions_table() {
        let db = test_db();
        // Should be able to query reactions table
        let count: i64 = db.conn.query_row(
            "SELECT COUNT(*) FROM reactions", [], |row| row.get(0),
        ).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn upsert_reaction_insert_and_replace() {
        let db = test_db();
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.insert_message("+1", "Alice", "2025-01-01T00:00:00Z", "hello", false, None, 1000).unwrap();

        // Insert a reaction
        db.upsert_reaction("+1", 1000, "Alice", "Bob", "👍").unwrap();
        let reactions = db.load_reactions("+1").unwrap();
        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0], (1000, "Alice".to_string(), "👍".to_string(), "Bob".to_string()));

        // Replace: same sender reacts with different emoji
        db.upsert_reaction("+1", 1000, "Alice", "Bob", "❤️").unwrap();
        let reactions = db.load_reactions("+1").unwrap();
        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0].2, "❤️");
    }

    #[test]
    fn remove_reaction() {
        let db = test_db();
        db.upsert_conversation("+1", "Alice", false).unwrap();

        db.upsert_reaction("+1", 1000, "Alice", "Bob", "👍").unwrap();
        assert_eq!(db.load_reactions("+1").unwrap().len(), 1);

        db.remove_reaction("+1", 1000, "Alice", "Bob").unwrap();
        assert_eq!(db.load_reactions("+1").unwrap().len(), 0);
    }

    #[test]
    fn load_reactions_attaches_to_messages() {
        let db = test_db();
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.insert_message("+1", "Alice", "2025-01-01T00:00:00Z", "hello", false, None, 1000).unwrap();
        db.insert_message("+1", "you", "2025-01-01T00:01:00Z", "hi", false, None, 2000).unwrap();

        db.upsert_reaction("+1", 1000, "Alice", "Bob", "👍").unwrap();
        db.upsert_reaction("+1", 2000, "you", "Alice", "❤️").unwrap();

        let convs = db.load_conversations(100).unwrap();
        assert_eq!(convs[0].messages[0].reactions.len(), 1);
        assert_eq!(convs[0].messages[0].reactions[0].emoji, "👍");
        assert_eq!(convs[0].messages[1].reactions.len(), 1);
        assert_eq!(convs[0].messages[1].reactions[0].emoji, "❤️");
    }

    #[test]
    fn search_messages_in_conversation() {
        let db = test_db();
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.insert_message("+1", "Alice", "2025-01-01T00:00:00Z", "hello world", false, None, 1000).unwrap();
        db.insert_message("+1", "you", "2025-01-01T00:01:00Z", "hi there", false, None, 2000).unwrap();
        db.insert_message("+1", "Alice", "2025-01-01T00:02:00Z", "Hello again", false, None, 3000).unwrap();

        // Case-insensitive search for "hello"
        let results = db.search_messages("+1", "hello", 50).unwrap();
        assert_eq!(results.len(), 2);
        // Most recent first
        assert_eq!(results[0].1, "Hello again");
        assert_eq!(results[1].1, "hello world");
    }

    #[test]
    fn search_messages_excludes_system_and_deleted() {
        let db = test_db();
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.insert_message("+1", "", "2025-01-01T00:00:00Z", "system hello", true, None, 1000).unwrap();
        db.insert_message("+1", "Alice", "2025-01-01T00:01:00Z", "real hello", false, None, 2000).unwrap();

        let results = db.search_messages("+1", "hello", 50).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, "real hello");
    }

    #[test]
    fn search_all_messages_across_conversations() {
        let db = test_db();
        db.upsert_conversation("+1", "Alice", false).unwrap();
        db.upsert_conversation("+2", "Bob", false).unwrap();
        db.insert_message("+1", "Alice", "2025-01-01T00:00:00Z", "hello from alice", false, None, 1000).unwrap();
        db.insert_message("+2", "Bob", "2025-01-01T00:01:00Z", "hello from bob", false, None, 2000).unwrap();

        let results = db.search_all_messages("hello", 50).unwrap();
        assert_eq!(results.len(), 2);
        // Most recent first
        assert_eq!(results[0].3, "+2"); // Bob's conversation
        assert_eq!(results[1].3, "+1"); // Alice's conversation
    }
}
