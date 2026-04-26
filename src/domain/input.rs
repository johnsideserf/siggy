//! Input composer state: text buffer, cursor, and history recall.
//!
//! Everything the user types into the composer lives here: the working
//! `buffer`, the `cursor` byte offset, and the Up/Down history stack
//! (`history`, `history_index`, `history_draft`).

/// State for the message composer: current draft and history recall.
#[derive(Default)]
pub struct InputState {
    /// Text input buffer.
    pub buffer: String,
    /// Cursor position (byte offset) in `buffer`.
    pub cursor: usize,
    /// Previously submitted inputs for Up/Down recall.
    pub history: Vec<String>,
    /// Current position in history (`None` means not browsing).
    pub history_index: Option<usize>,
    /// Saves in-progress input when browsing history.
    pub history_draft: String,
}

impl InputState {
    /// Reset the composer's transient state on conversation switch:
    /// clears the buffer, cursor, and history-browse position. The
    /// `history` vec is preserved because it's per-session, not
    /// per-conversation.
    pub fn reset_for_conv_switch(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.history_index = None;
        self.history_draft.clear();
    }
}
