//! Theme picker overlay state.
//!
//! Holds the cursor `index` and the populated `available_themes` list
//! (built-in plus any user-defined themes loaded from disk) shown in
//! the `/theme` overlay.

use crate::theme::Theme;

/// State for the theme picker overlay.
#[derive(Default)]
pub struct ThemePickerState {
    /// Cursor position in theme picker
    pub index: usize,
    /// All available themes (built-in + custom)
    pub available_themes: Vec<Theme>,
}
