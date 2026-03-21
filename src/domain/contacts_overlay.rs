/// State for the contacts list overlay.
#[derive(Default)]
pub struct ContactsOverlayState {
    /// Contacts overlay visible
    pub show: bool,
    /// Cursor position in contacts list
    pub index: usize,
    /// Type-to-filter text for contacts overlay
    pub filter: String,
    /// Filtered list of (phone_number, display_name)
    pub filtered: Vec<(String, String)>,
}
