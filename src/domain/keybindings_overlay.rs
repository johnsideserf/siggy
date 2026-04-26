//! Keybindings configuration overlay state.
//!
//! Drives the binding editor: cursor (`index`), in-progress key capture
//! (`capturing`) with conflict detection (`conflict`), and a nested
//! profile sub-picker (`profile_picker`, `profile_index`,
//! `available_profiles`) for switching between named keybinding profiles
//! without leaving the overlay.

use crate::keybindings::{KeyAction, KeyCombo};

/// State for the keybindings configuration overlay.
#[derive(Default)]
pub struct KeybindingsOverlayState {
    /// Cursor position in keybindings overlay
    pub index: usize,
    /// Whether capturing a new key binding
    pub capturing: bool,
    /// Conflict detected during capture
    pub conflict: Option<(KeyAction, KeyCombo)>,
    /// Profile sub-picker visible within keybindings overlay
    pub profile_picker: bool,
    /// Cursor position in profile sub-picker
    pub profile_index: usize,
    /// All available keybinding profile names
    pub available_profiles: Vec<String>,
}
