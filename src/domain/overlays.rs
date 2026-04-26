//! Cursor / filter / temp-buffer state for the modal overlays.
//!
//! These eleven structs share the same shape (cursor `index`, often a
//! `filter` string, sometimes a `filtered` list and a `pending` context
//! captured when the overlay was opened) and are each ~10-30 lines of
//! pure `#[derive(Default)]` data. Keeping them as one file beats one
//! file per overlay -- there's nothing to navigate to that isn't here.
//!
//! Substantive overlay state with real behaviour (typing indicators,
//! search, emoji picker, file picker, image cache) lives in its own
//! file under `src/domain/` because it earns the separation.

use std::collections::HashMap;

use crate::app::{GroupMenuState, PinPending, PollVotePending};
use crate::keybindings::{KeyAction, KeyCombo};
use crate::settings_profile::SettingsProfile;
use crate::signal::types::{IdentityInfo, PollData};
use crate::theme::Theme;

/// State for the message action menu overlay.
#[derive(Default)]
pub struct ActionMenuState {
    /// Cursor position in action menu
    pub index: usize,
}

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

/// State for the pin duration picker overlay.
#[derive(Default)]
pub struct PinDurationOverlayState {
    /// Cursor position in pin duration picker
    pub index: usize,
    /// Pending pin context (conversation, target message)
    pub pending: Option<PinPending>,
}

/// State for the theme picker overlay.
#[derive(Default)]
pub struct ThemePickerState {
    /// Cursor position in theme picker
    pub index: usize,
    /// All available themes (built-in + custom)
    pub available_themes: Vec<Theme>,
}

/// State for the identity verification overlay.
#[derive(Default)]
pub struct VerifyOverlayState {
    /// Cursor position in verify overlay (for group member list)
    pub index: usize,
    /// Identity info entries filtered for the current overlay
    pub identities: Vec<IdentityInfo>,
    /// Confirmation pending for verify action
    pub confirming: bool,
}

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

/// State for the poll vote overlay and pending poll data.
#[derive(Default)]
pub struct PollVoteOverlayState {
    /// Cursor position in poll vote overlay
    pub index: usize,
    /// Multi-select tracking for poll vote options
    pub selections: Vec<bool>,
    /// Pending poll vote context
    pub pending: Option<PollVotePending>,
    /// Buffered poll data for races (keyed by conv_id + timestamp)
    pub pending_polls: HashMap<(String, i64), PollData>,
}

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
