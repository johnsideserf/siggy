//! Message action menu state.
//!
//! Tracks the cursor position in the per-message action menu (delete,
//! copy, forward, react, reply, edit) opened from a focused message.

/// State for the message action menu overlay.
#[derive(Default)]
pub struct ActionMenuState {
    /// Cursor position in action menu
    pub index: usize,
}
