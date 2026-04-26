//! Pin duration picker overlay state.
//!
//! Tracks the cursor `index` over the duration choices and the
//! `pending` `PinPending` context (target conversation + message)
//! captured when the picker was opened.

use crate::app::PinPending;

/// State for the pin duration picker overlay.
#[derive(Default)]
pub struct PinDurationOverlayState {
    /// Cursor position in pin duration picker
    pub index: usize,
    /// Pending pin context (conversation, target message)
    pub pending: Option<PinPending>,
}
