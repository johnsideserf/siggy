//! Settings profile manager overlay state.
//!
//! Tracks the currently active profile `name`, the cursor `index`
//! over the `available` profiles, and the "save as" sub-flow
//! (`save_as` flag + `save_as_input` text buffer) used to capture a
//! new profile name without touching the main composer.

use crate::settings_profile::SettingsProfile;

/// State for the settings profile manager overlay.
pub struct SettingsProfileOverlayState {
    /// Current settings profile name
    pub name: String,
    /// Cursor position in settings profile manager
    pub index: usize,
    /// All available settings profiles
    pub available: Vec<SettingsProfile>,
    /// Save-as mode active in profile manager
    pub save_as: bool,
    /// Text input buffer for save-as name
    pub save_as_input: String,
}

impl Default for SettingsProfileOverlayState {
    fn default() -> Self {
        Self {
            name: "Default".to_string(),
            index: 0,
            available: Vec::new(),
            save_as: false,
            save_as_input: String::new(),
        }
    }
}
