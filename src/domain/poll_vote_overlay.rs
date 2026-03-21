use std::collections::HashMap;

use crate::app::PollVotePending;
use crate::signal::types::PollData;

/// State for the poll vote overlay and pending poll data.
#[derive(Default)]
pub struct PollVoteOverlayState {
    /// Poll vote overlay visible
    pub show: bool,
    /// Cursor position in poll vote overlay
    pub index: usize,
    /// Multi-select tracking for poll vote options
    pub selections: Vec<bool>,
    /// Pending poll vote context
    pub pending: Option<PollVotePending>,
    /// Buffered poll data for races (keyed by conv_id + timestamp)
    pub pending_polls: HashMap<(String, i64), PollData>,
}
