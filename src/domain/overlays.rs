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

use crate::keybindings::{KeyAction, KeyCombo};
use crate::settings_profile::SettingsProfile;
use crate::signal::types::{IdentityInfo, PollData, PollOption};
use crate::theme::Theme;

/// Which sub-overlay of the /group menu is currently active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupMenuState {
    Menu,         // top-level flyout
    Members,      // read-only member list
    AddMember,    // contact picker (type-to-filter)
    RemoveMember, // member picker (type-to-filter)
    Rename,       // text input (pre-filled)
    Create,       // text input (empty)
    LeaveConfirm, // y/n confirmation
}

/// Context saved when the pin duration picker is open (remembers which message is being pinned).
pub struct PinPending {
    pub conv_id: String,
    pub is_group: bool,
    pub target_author: String,
    pub target_timestamp: i64,
}

/// Context saved when the poll vote overlay is open.
pub struct PollVotePending {
    pub conv_id: String,
    pub is_group: bool,
    pub poll_author: String,
    pub poll_timestamp: i64,
    pub allow_multiple: bool,
    pub options: Vec<PollOption>,
}

/// State for the message action menu overlay.
#[derive(Default)]
pub struct ActionMenuState {
    /// Cursor position in action menu
    pub index: usize,
}

/// State for the settings overlay and its Customize sub-overlay.
#[derive(Default)]
pub struct SettingsOverlayState {
    /// Cursor position in the settings list
    pub index: usize,
    /// Cursor position in the Customize sub-menu (spawned from the
    /// "Customize..." row in the settings overlay)
    pub customize_index: usize,
    /// Snapshot of `mouse.enabled` at overlay-open time, used by
    /// `fire_deferred_settings_hooks` to decide whether to queue the
    /// mouse-capture toggle on close.
    pub mouse_snapshot: bool,
}

/// State for the sidebar type-to-filter overlay (`/_`).
#[derive(Default)]
pub struct SidebarFilterState {
    /// Current filter text.
    pub query: String,
    /// Conversation IDs matching the filter.
    pub filtered: Vec<String>,
}

/// One selectable row in the command palette (#614).
#[derive(Debug, Clone, PartialEq)]
pub enum PaletteItem {
    /// A slash command; `args` is its usage hint (empty = runs immediately
    /// on select, non-empty = prefills the composer).
    Command {
        name: &'static str,
        args: &'static str,
        description: &'static str,
    },
    /// Jump to a conversation.
    Conversation {
        id: String,
        name: String,
        is_group: bool,
    },
}

/// State for the fuzzy command palette overlay (#614).
#[derive(Default)]
pub struct PaletteState {
    /// Type-to-filter query.
    pub query: String,
    /// Cursor position in the filtered list.
    pub index: usize,
    /// Filtered items, best match first.
    pub filtered: Vec<PaletteItem>,
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
