//! Poll vote overlay and pending poll-data buffer.
//!
//! `index` and `selections` drive the multi-select cursor for the open
//! vote dialog; `pending` carries the in-flight `PollVotePending`
//! context. `pending_polls` buffers `PollData` keyed by
//! `(conv_id, timestamp)` so a poll that arrives before its parent
//! message is preserved until the message is rendered.

use std::collections::HashMap;

use crate::app::PollVotePending;
use crate::signal::types::PollData;

/// State for the poll vote overlay and pending poll data.
#[derive(Default)]
pub struct PollVoteOverlayState {
    /// Cursor position in poll vote overlay
    pub index: usize,
    /// Multi-select tracking for poll vote options
    pub selections: Vec<bool>,
    /// Pending poll vote context
    pub pending: Option<PollVotePending>,
    /// Buffered poll data for races (keyed by conv_id + timestamp)
    pub pending_polls: HashMap<(String, i64), PollData>,
}
