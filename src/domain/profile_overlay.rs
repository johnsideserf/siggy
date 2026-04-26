//! Profile editor overlay state.
//!
//! Holds the four editable profile fields (`given_name`,
//! `family_name`, `about`, `about_emoji`) as a fixed-size array, along
//! with the cursor `index`, the `editing` flag, and a temporary
//! `edit_buffer` used to stage changes before they are committed back
//! to the field array.

/// State for the profile editor overlay.
#[derive(Default)]
pub struct ProfileOverlayState {
    /// Cursor position in profile editor
    pub index: usize,
    /// Whether currently editing a profile field
    pub editing: bool,
    /// Profile fields: [given_name, family_name, about, about_emoji]
    pub fields: [String; 4],
    /// Temp buffer while editing a profile field
    pub edit_buffer: String,
}
