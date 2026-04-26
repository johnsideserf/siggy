//! Modal overlays rendered on top of the main UI.
//!
//! One submodule per `OverlayKind`, each exporting a `draw_*`
//! function. All overlays share `centered_popup` (defined in
//! `ui/mod.rs`) for the standard "clear area + bordered popup"
//! frame, plus per-overlay popup-width constants for sizing.

pub(super) mod about;
pub(super) mod action_menu;
pub(super) mod contacts;
pub(super) mod customize;
pub(super) mod delete_confirm;
pub(super) mod emoji_picker;
pub(super) mod file_browser;
pub(super) mod forward;
pub(super) mod group_menu;
pub(super) mod help;
pub(super) mod keybindings;
pub(super) mod message_request;
pub(super) mod pin_duration;
pub(super) mod poll_vote;
pub(super) mod profile;
pub(super) mod reaction_picker;
pub(super) mod search;
pub(super) mod settings;
pub(super) mod settings_profile;
pub(super) mod theme_picker;
pub(super) mod verify;
