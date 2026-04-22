//! Mouse hit-test areas and capture state.
//!
//! Holds the rendered regions the main loop uses to route mouse clicks
//! (sidebar, messages, composer), along with the runtime mouse-capture
//! flag and a one-shot toggle queued from the settings overlay.

use ratatui::layout::Rect;

/// State for mouse hit-testing and capture.
#[derive(Default)]
pub struct MouseState {
    /// Inner area of the sidebar List widget (`None` when the sidebar is hidden).
    pub sidebar_inner: Option<Rect>,
    /// Inner area of the messages block.
    pub messages_area: Rect,
    /// Outer area of the composer input box (includes borders).
    pub input_area: Rect,
    /// Badge + "> " length in the composer input box.
    pub input_prefix_len: u16,
    /// Whether mouse support is active (click sidebar, scroll messages, click links).
    pub enabled: bool,
    /// Pending mouse-capture toggle: set by the settings overlay's `on_toggle`,
    /// drained by the main loop to apply `EnableMouseCapture` / `DisableMouseCapture`.
    pub pending_toggle: Option<bool>,
}
