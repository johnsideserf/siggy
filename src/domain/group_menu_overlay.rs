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
