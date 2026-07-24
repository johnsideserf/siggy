//! Durable receive journal (#642 U11, plan KTD-2).
//!
//! libsignal-service acks each envelope on the websocket *before* yielding
//! it to the stream, so anything the process has pulled but not persisted
//! is gone if it crashes. The contract: the engine thread appends each
//! mapped event here, on its own connection, before pulling the next
//! stream item; the main thread deletes the row after committing the
//! event's effects; startup replays leftovers, and the v16 entry_seq dedup
//! index makes replays silent.
//!
//! PLAN DEVIATION (documented for the U19 gate): plan U6 had the adapter
//! writing finished rows straight into `messages` from its own connection.
//! That is not implementable - `messages` rows store app-resolved display
//! data (contact names, conversation upserts, unread counts) that only the
//! main thread's state can produce. The journal keeps U6's durability
//! ordering (persist-before-next-pull, second connection, busy_timeout)
//! while leaving row production where the data lives. Same crash windows,
//! same dedup discipline.
//!
//! Payload rows are JSON of [`JournalEvent`], a mirror of the
//! persistent-effect subset of [`SignalEvent`]. Rows normally live
//! milliseconds; a row that survives a crash *and* a version upgrade whose
//! payload no longer parses is logged and dropped (the envelope was acked
//! either way - this is the documented residual window, not a new one).

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::signal::types::{ReceiptKind, SignalEvent, SignalMessage};

/// The journaled subset of [`SignalEvent`]: exactly the variants whose loss
/// at a crash would be user-visible data loss. Transient events (typing,
/// sync lifecycle, directory refreshes) are deliberately absent - they are
/// re-derivable or harmless to drop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JournalEvent {
    Message(SignalMessage),
    Receipt {
        sender: String,
        receipt_type: ReceiptKind,
        timestamps: Vec<i64>,
    },
    Reaction {
        conv_id: String,
        emoji: String,
        sender: String,
        sender_name: Option<String>,
        target_author: String,
        target_timestamp: i64,
        is_remove: bool,
    },
    Edit {
        conv_id: String,
        sender: String,
        sender_name: Option<String>,
        target_timestamp: i64,
        new_body: String,
        new_timestamp: i64,
        is_outgoing: bool,
    },
    RemoteDelete {
        conv_id: String,
        sender: String,
        target_timestamp: i64,
    },
    TimerChange {
        conv_id: String,
        seconds: i64,
        body: String,
        timestamp_ms: i64,
    },
    System {
        conv_id: String,
        body: String,
        timestamp_ms: i64,
    },
    ReadSync {
        read_messages: Vec<(String, i64)>,
    },
}

/// Millisecond timestamp → the `DateTime<Utc>` twin the event variants
/// carry. Out-of-range values clamp to epoch, matching the mapper's own
/// fallback behavior.
fn ts_utc(ms: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default()
}

impl JournalEvent {
    /// The journal twin of a boundary event, `None` when the event is
    /// transient and not worth a durable row.
    pub fn from_signal(event: &SignalEvent) -> Option<Self> {
        match event {
            SignalEvent::MessageReceived(msg) => Some(Self::Message(msg.clone())),
            SignalEvent::ReceiptReceived {
                sender,
                receipt_type,
                timestamps,
            } => Some(Self::Receipt {
                sender: sender.clone(),
                receipt_type: *receipt_type,
                timestamps: timestamps.clone(),
            }),
            SignalEvent::ReactionReceived {
                conv_id,
                emoji,
                sender,
                sender_name,
                target_author,
                target_timestamp,
                is_remove,
            } => Some(Self::Reaction {
                conv_id: conv_id.clone(),
                emoji: emoji.clone(),
                sender: sender.clone(),
                sender_name: sender_name.clone(),
                target_author: target_author.clone(),
                target_timestamp: *target_timestamp,
                is_remove: *is_remove,
            }),
            SignalEvent::EditReceived {
                conv_id,
                sender,
                sender_name,
                target_timestamp,
                new_body,
                new_timestamp,
                is_outgoing,
            } => Some(Self::Edit {
                conv_id: conv_id.clone(),
                sender: sender.clone(),
                sender_name: sender_name.clone(),
                target_timestamp: *target_timestamp,
                new_body: new_body.clone(),
                new_timestamp: *new_timestamp,
                is_outgoing: *is_outgoing,
            }),
            SignalEvent::RemoteDeleteReceived {
                conv_id,
                sender,
                target_timestamp,
            } => Some(Self::RemoteDelete {
                conv_id: conv_id.clone(),
                sender: sender.clone(),
                target_timestamp: *target_timestamp,
            }),
            SignalEvent::ExpirationTimerChanged {
                conv_id,
                seconds,
                body,
                timestamp_ms,
                ..
            } => Some(Self::TimerChange {
                conv_id: conv_id.clone(),
                seconds: *seconds,
                body: body.clone(),
                timestamp_ms: *timestamp_ms,
            }),
            SignalEvent::SystemMessage {
                conv_id,
                body,
                timestamp_ms,
                ..
            } => Some(Self::System {
                conv_id: conv_id.clone(),
                body: body.clone(),
                timestamp_ms: *timestamp_ms,
            }),
            SignalEvent::ReadSyncReceived { read_messages } => Some(Self::ReadSync {
                read_messages: read_messages.clone(),
            }),
            _ => None,
        }
    }

    /// Back to the boundary vocabulary for replay through
    /// `App::handle_signal_event`.
    pub fn into_signal(self) -> SignalEvent {
        match self {
            Self::Message(msg) => SignalEvent::MessageReceived(msg),
            Self::Receipt {
                sender,
                receipt_type,
                timestamps,
            } => SignalEvent::ReceiptReceived {
                sender,
                receipt_type,
                timestamps,
            },
            Self::Reaction {
                conv_id,
                emoji,
                sender,
                sender_name,
                target_author,
                target_timestamp,
                is_remove,
            } => SignalEvent::ReactionReceived {
                conv_id,
                emoji,
                sender,
                sender_name,
                target_author,
                target_timestamp,
                is_remove,
            },
            Self::Edit {
                conv_id,
                sender,
                sender_name,
                target_timestamp,
                new_body,
                new_timestamp,
                is_outgoing,
            } => SignalEvent::EditReceived {
                conv_id,
                sender,
                sender_name,
                target_timestamp,
                new_body,
                new_timestamp,
                is_outgoing,
            },
            Self::RemoteDelete {
                conv_id,
                sender,
                target_timestamp,
            } => SignalEvent::RemoteDeleteReceived {
                conv_id,
                sender,
                target_timestamp,
            },
            Self::TimerChange {
                conv_id,
                seconds,
                body,
                timestamp_ms,
            } => SignalEvent::ExpirationTimerChanged {
                conv_id,
                seconds,
                body,
                timestamp: ts_utc(timestamp_ms),
                timestamp_ms,
            },
            Self::System {
                conv_id,
                body,
                timestamp_ms,
            } => SignalEvent::SystemMessage {
                conv_id,
                body,
                timestamp: ts_utc(timestamp_ms),
                timestamp_ms,
            },
            Self::ReadSync { read_messages } => SignalEvent::ReadSyncReceived { read_messages },
        }
    }
}

/// The engine thread's own connection to siggy.db, used exclusively for
/// journal appends (plan U6 topology: second connection, busy_timeout on
/// both sides, so adapter writes queue behind a busy main thread instead of
/// erroring). WAL is a database-level property already set by the main
/// connection; `synchronous=NORMAL` matches it - commits survive app
/// crash (the KTD-2 bar), not power loss.
pub struct JournalWriter {
    conn: Connection,
}

impl JournalWriter {
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("open journal connection to {}", db_path.display()))?;
        conn.execute_batch("PRAGMA synchronous=NORMAL;")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(Self { conn })
    }

    /// Durably append one event; returns the row id the main thread deletes
    /// after processing. The INSERT autocommits, so when this returns the
    /// row survives an app crash - the caller may then (and only then) pull
    /// the next stream item (KTD-2 ordering).
    pub fn append(&self, event: &JournalEvent) -> Result<i64> {
        let payload = serde_json::to_string(event).context("serialize journal event")?;
        self.conn.execute(
            "INSERT INTO native_journal (payload) VALUES (?1)",
            params![payload],
        )?;
        Ok(self.conn.last_insert_rowid())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::types::StyleType;

    fn sample_message() -> SignalMessage {
        SignalMessage {
            source: "+15550001111".into(),
            source_name: Some("Ada".into()),
            source_uuid: Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into()),
            timestamp: ts_utc(1_700_000_000_123),
            body: Some("hello *world*".into()),
            group_id: Some("Z3JvdXBpZA==".into()),
            quote: Some((5, "+15552223333".into(), "quoted".into())),
            expires_in_seconds: 3600,
            text_styles: vec![crate::signal::types::TextStyle {
                start: 6,
                length: 5,
                style: StyleType::Bold,
            }],
            ..Default::default()
        }
    }

    fn roundtrip(event: &SignalEvent) -> SignalEvent {
        let journaled = JournalEvent::from_signal(event).expect("journalable");
        let json = serde_json::to_string(&journaled).unwrap();
        let parsed: JournalEvent = serde_json::from_str(&json).unwrap();
        parsed.into_signal()
    }

    #[test]
    fn message_roundtrips_with_full_fidelity() {
        let event = SignalEvent::MessageReceived(sample_message());
        match roundtrip(&event) {
            SignalEvent::MessageReceived(msg) => {
                let original = sample_message();
                assert_eq!(msg.source, original.source);
                assert_eq!(msg.source_name, original.source_name);
                assert_eq!(msg.timestamp, original.timestamp);
                assert_eq!(msg.body, original.body);
                assert_eq!(msg.group_id, original.group_id);
                assert_eq!(msg.quote, original.quote);
                assert_eq!(msg.expires_in_seconds, original.expires_in_seconds);
                assert_eq!(msg.text_styles.len(), 1);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn every_persistent_variant_roundtrips() {
        let events = vec![
            SignalEvent::ReceiptReceived {
                sender: "+15550001111".into(),
                receipt_type: ReceiptKind::Read,
                timestamps: vec![1, 2],
            },
            SignalEvent::ReactionReceived {
                conv_id: "+15550001111".into(),
                emoji: "👍".into(),
                sender: "+15550001111".into(),
                sender_name: None,
                target_author: "+15552223333".into(),
                target_timestamp: 42,
                is_remove: false,
            },
            SignalEvent::EditReceived {
                conv_id: "+15550001111".into(),
                sender: "+15550001111".into(),
                sender_name: None,
                target_timestamp: 42,
                new_body: "edited".into(),
                new_timestamp: 43,
                is_outgoing: false,
            },
            SignalEvent::RemoteDeleteReceived {
                conv_id: "+15550001111".into(),
                sender: "+15550001111".into(),
                target_timestamp: 42,
            },
            SignalEvent::ExpirationTimerChanged {
                conv_id: "+15550001111".into(),
                seconds: 3600,
                body: "Disappearing messages set to 1 hour".into(),
                timestamp: ts_utc(9),
                timestamp_ms: 9,
            },
            SignalEvent::SystemMessage {
                conv_id: "+15550001111".into(),
                body: "sys".into(),
                timestamp: ts_utc(9),
                timestamp_ms: 9,
            },
            SignalEvent::ReadSyncReceived {
                read_messages: vec![("+15550001111".into(), 42)],
            },
        ];
        for event in &events {
            // Round-trips without panicking and lands on the same variant.
            let back = roundtrip(event);
            assert_eq!(
                std::mem::discriminant(event),
                std::mem::discriminant(&back),
                "variant changed across journal roundtrip: {event:?} -> {back:?}"
            );
        }
    }

    #[test]
    fn transient_events_are_not_journaled() {
        for event in [
            SignalEvent::SyncComplete,
            SignalEvent::Disconnected,
            SignalEvent::Error("x".into()),
            SignalEvent::TypingIndicator {
                sender: "+15550001111".into(),
                sender_name: None,
                is_typing: true,
                group_id: None,
            },
            SignalEvent::ContactList(vec![]),
            SignalEvent::GroupList(vec![]),
        ] {
            assert!(
                JournalEvent::from_signal(&event).is_none(),
                "unexpectedly journaled: {event:?}"
            );
        }
    }

    #[test]
    fn writer_appends_rows_the_main_connection_sees() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.db");
        // Main connection creates the schema (migration 17)...
        let db = crate::db::Database::open(&file).unwrap();
        // ...and the adapter connection appends through its own handle.
        let writer = JournalWriter::open(&file).unwrap();
        let id = writer
            .append(
                &JournalEvent::from_signal(&SignalEvent::MessageReceived(sample_message()))
                    .unwrap(),
            )
            .unwrap();
        let rows = db.journal_pending().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, id);
        let parsed: JournalEvent = serde_json::from_str(&rows[0].1).unwrap();
        assert!(matches!(parsed, JournalEvent::Message(_)));
        db.journal_delete(id).unwrap();
        assert!(db.journal_pending().unwrap().is_empty());
    }
}
