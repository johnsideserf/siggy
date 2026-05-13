//! Domain state structs extracted from [`crate::app::App`].
//!
//! Each submodule owns the fields and helpers for one logical concern
//! (file picker, search, typing indicators, image cache). The simpler
//! cursor/filter/temp-buffer overlays live together in `overlays`
//! since each was ~10-30 lines of pure-data struct and the per-file
//! split added navigation cost without payoff.

mod emoji_picker;
mod file_picker;
mod image;
mod input;
mod lock;
mod mouse;
mod notification;
mod overlays;
mod pending;
mod reaction;
mod scroll;
mod search;
mod typing;

pub use emoji_picker::{CATEGORIES, EmojiPickerAction, EmojiPickerSource, EmojiPickerState};
pub use file_picker::{FilePickerOutcome, FilePickerState};
pub use image::ImageState;
pub use input::InputState;
#[allow(unused_imports)]
pub use lock::{LockPhase, LockState};
#[allow(unused_imports)]
pub use lock::{hash_passphrase, load_hash, lock_hash_path, save_hash, verify_passphrase};
pub use mouse::MouseState;
pub use notification::NotificationState;
pub use overlays::{
    ActionMenuState, ContactsOverlayState, ForwardOverlayState, GroupMenuOverlayState,
    KeybindingsOverlayState, PinDurationOverlayState, PollVoteOverlayState, ProfileOverlayState,
    SettingsProfileOverlayState, ThemePickerState, VerifyOverlayState,
};
pub use pending::PendingState;
pub use reaction::ReactionState;
pub use scroll::ScrollState;
pub use search::{SearchAction, SearchState};
pub use typing::TypingState;
