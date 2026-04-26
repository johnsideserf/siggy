//! Contacts browser overlay state.
//!
//! Backs the `/contacts` overlay: tracks the cursor `index`, the
//! type-to-`filter` query, and the `filtered` `(phone, display_name)`
//! list rebuilt as the user types.

/// State for the contacts list overlay.
#[derive(Default)]
pub struct ContactsOverlayState {
    /// Cursor position in contacts list
    pub index: usize,
    /// Type-to-filter text for contacts overlay
    pub filter: String,
    /// Filtered list of (phone_number, display_name)
    pub filtered: Vec<(String, String)>,
}
