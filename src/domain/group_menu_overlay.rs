//! Group management menu overlay state.
//!
//! Backs the `/group` overlay across its sub-screens (`state`): main
//! menu, add/remove member pickers (with type-to-`filter`), and the
//! rename/create flow that uses `input` as a separate text buffer so
//! the composer is not disturbed.

use crate::app::GroupMenuState;

/// State for the group management menu overlay.
#[derive(Default)]
pub struct GroupMenuOverlayState {
    /// Group management menu state (which submenu is active)
    pub state: Option<GroupMenuState>,
    /// Cursor position in group menu / member lists
    pub index: usize,
    /// Type-to-filter text for add/remove member pickers
    pub filter: String,
    /// Filtered list of (phone, display_name)
    pub filtered: Vec<(String, String)>,
    /// Separate text input buffer for rename/create
    pub input: String,
}
