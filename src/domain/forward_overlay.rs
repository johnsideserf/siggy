//! Forward-message picker overlay state.
//!
//! Carries the message `body` being forwarded and a type-to-`filter`
//! conversation picker (`filtered` `(conv_id, display_name)`) so the
//! user can pick a destination conversation for the forwarded text.

/// State for the forward message picker overlay.
#[derive(Default)]
pub struct ForwardOverlayState {
    /// Cursor position in forward picker
    pub index: usize,
    /// Type-to-filter text for forward picker
    pub filter: String,
    /// Filtered list of (conv_id, display_name)
    pub filtered: Vec<(String, String)>,
    /// Body of the message being forwarded
    pub body: String,
}
