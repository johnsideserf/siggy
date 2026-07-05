//! Input composer state: text buffer, cursor, and history recall.
//!
//! Everything the user types into the composer lives here: the working
//! `buffer`, the `cursor` byte offset, and the Up/Down history stack
//! (`history`, `history_index`, `history_draft`).

/// Cap on retained submitted-input history. A long-running session would
/// otherwise grow `history` without bound (#492); older entries are dropped.
const MAX_INPUT_HISTORY: usize = 500;

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

    /// Record a submitted line for Up/Down recall, bounded at
    /// `MAX_INPUT_HISTORY` so the stack cannot grow without limit over a long
    /// session (#492). The oldest entries are dropped first.
    pub fn push_history(&mut self, entry: String) {
        self.history.push(entry);
        if self.history.len() > MAX_INPUT_HISTORY {
            let overflow = self.history.len() - MAX_INPUT_HISTORY;
            self.history.drain(..overflow);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_history_bounds_at_cap_keeping_newest() {
        let mut input = InputState::default();
        for i in 0..(MAX_INPUT_HISTORY + 10) {
            input.push_history(format!("msg{i}"));
        }
        assert_eq!(input.history.len(), MAX_INPUT_HISTORY);
        // Oldest dropped, newest retained.
        assert_eq!(
            input.history.last().unwrap(),
            &format!("msg{}", MAX_INPUT_HISTORY + 9)
        );
        assert_eq!(input.history.first().unwrap(), "msg10");
    }
}
