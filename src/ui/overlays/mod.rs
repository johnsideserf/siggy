//! Modal overlays rendered on top of the main UI.
//!
//! Most submodules correspond 1:1 with an `OverlayKind` variant and
//! export a `draw_*` function. Two helpers ride along on a host file:
//! `draw_customize` lives in `settings.rs` (Settings is its only
//! caller) and `draw_delete_confirm` lives in `action_menu.rs` (the
//! action menu's "delete" choice is its only entry point). All
//! overlays share `centered_popup` (defined in `ui/mod.rs`) for the
//! standard "clear area + bordered popup" frame.

pub(super) mod about;
pub(super) mod action_menu;
pub(super) mod contacts;
pub(super) mod delete_conversation_confirm;
pub(super) mod emoji_picker;
pub(super) mod file_browser;
pub(super) mod forward;
pub(super) mod group_menu;
pub(super) mod help;
pub(super) mod keybindings;
pub(super) mod lock_screen;
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
