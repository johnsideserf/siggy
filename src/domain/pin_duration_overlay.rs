use crate::app::PinPending;

/// State for the pin duration picker overlay.
#[derive(Default)]
pub struct PinDurationOverlayState {
    /// Pin duration picker overlay visible
    pub show: bool,
    /// Cursor position in pin duration picker
    pub index: usize,
    /// Pending pin context (conversation, target message)
    pub pending: Option<PinPending>,
}
