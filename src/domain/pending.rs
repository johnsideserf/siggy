//! In-flight signal-cli work awaiting confirmation or dispatch.
//!
//! Tracks send RPCs awaiting a `SendTimestamp` response (`sends`),
//! receipts that arrived before their matching send (`receipts`), the
//! queued typing-stop request from conversation switches (`typing_stop`),
//! and queued outgoing read receipts (`read_receipts`). All four are
//! drained by the main event loop.

use std::collections::HashMap;

use crate::app::SendRequest;

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
    /// Populated by `handle_receipt()` when no matching pending send exists
    /// yet. Drained immediately after each `SendTimestamp` confirms a send.
    pub receipts: Vec<(String, String, Vec<i64>)>,
    /// Queued typing-stop request from conversation switches.
    ///
    /// Drained by the main loop.
    pub typing_stop: Option<SendRequest>,
    /// Queued read receipts to dispatch: `(recipient_phone, timestamps)`.
    pub read_receipts: Vec<(String, Vec<i64>)>,
}
