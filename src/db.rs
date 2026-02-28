use std::path::Path;

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::app::{Conversation, DisplayMessage};

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

    #[cfg(test)]
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
    /// Returns (Conversation, unread_count) pairs.
    pub fn load_conversations(&self, msg_limit: usize) -> Result<Vec<(Conversation, usize)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, is_group FROM conversations")?;

        let convs: Vec<(String, String, bool)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i32>(2)? != 0,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut result = Vec::new();

        for (id, name, is_group) in convs {
            // Load last N messages
            let mut msg_stmt = self.conn.prepare(
                "SELECT sender, timestamp, body, is_system FROM messages
                 WHERE conversation_id = ?1
                 ORDER BY rowid DESC LIMIT ?2",
            )?;

            let mut messages: Vec<DisplayMessage> = msg_stmt
                .query_map(params![id, msg_limit as i64], |row| {
                    let sender: String = row.get(0)?;
                    let ts_str: String = row.get(1)?;
                    let body: String = row.get(2)?;
                    let is_system: bool = row.get::<_, i32>(3)? != 0;
                    Ok((sender, ts_str, body, is_system))
                })?
                .filter_map(|r| r.ok())
                .filter_map(|(sender, ts_str, body, is_system)| {
                    let timestamp = chrono::DateTime::parse_from_rfc3339(&ts_str)
                        .ok()?
                        .with_timezone(&chrono::Utc);
                    Some(DisplayMessage {
                        sender,
                        timestamp,
                        body,
                        is_system,
                    })
                })
                .collect();

            // Reverse so oldest first
            messages.reverse();

            let unread = self.unread_count(&id).unwrap_or(0);

            result.push((
                Conversation {
                    name,
                    id: id.clone(),
                    messages,
                    unread: 0, // will be set by caller
                    is_group,
                },
                unread,
            ));
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

    pub fn insert_message(
        &self,
        conv_id: &str,
        sender: &str,
        timestamp: &str,
        body: &str,
        is_system: bool,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO messages (conversation_id, sender, timestamp, body, is_system)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![conv_id, sender, timestamp, body, is_system as i32],
        )?;
        Ok(self.conn.last_insert_rowid())
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
}
