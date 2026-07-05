//! In-flight signal-cli work awaiting confirmation or dispatch.
//!
//! Tracks send RPCs awaiting a `SendTimestamp` response (`sends`),
//! receipts that arrived before their matching send (`receipts`), the
//! queued typing-stop request from conversation switches (`typing_stop`),
//! and queued outgoing read receipts (`read_receipts`). All four are
//! drained by the main event loop.

use std::collections::HashMap;

use super::send::SendRequest;
use crate::signal::types::ReceiptKind;

/// A receipt that arrived before the message it targets was matchable
/// (typically before the `SendTimestamp` that rewrites the local timestamp
/// to the server one). Replayed after each subsequent `SendTimestamp`;
/// `attempts` counts failed replays so an entry that can never match is
/// eventually resolved against the DB and dropped instead of re-buffering
/// forever (#484).
pub struct BufferedReceipt {
    pub sender: String,
    pub receipt_type: ReceiptKind,
    /// Only the timestamps that have not matched yet.
    pub timestamps: Vec<i64>,
    pub attempts: u8,
}

/// State for in-flight signal-cli work awaiting confirmation or dispatch.
#[derive(Default)]
pub struct PendingState {
    /// Pending send RPCs: `rpc_id -> (conv_id, local_timestamp_ms)`.
    ///
    /// Populated by `dispatch_send()` on message send. Entries removed on
    /// `SendTimestamp` (success) or `SendFailed` (error). Used to correlate
    /// signal-cli responses with local messages.
    pub sends: HashMap<String, (String, i64)>,
    /// Receipts that arrived before their matching `SendTimestamp`.
    ///
    /// Populated by `handle_receipt()` for timestamps with no matching
    /// message yet. Replayed after each `SendTimestamp`; bounded by attempt
    /// count and buffer size with a direct-DB fallback (#484).
    pub receipts: Vec<BufferedReceipt>,
    /// Queued typing-stop request from conversation switches.
    ///
    /// Drained by the main loop.
    pub typing_stop: Option<SendRequest>,
    /// Queued read receipts to dispatch: `(recipient_phone, timestamps)`.
    pub read_receipts: Vec<(String, Vec<i64>)>,
    /// Auto-replies queued by message triggers (#615), drained by the main
    /// loop into `dispatch_send` like any composer send.
    pub trigger_sends: Vec<SendRequest>,
}
