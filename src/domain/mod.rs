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
mod media;
mod mouse;
mod notification;
mod overlays;
mod pending;
mod reaction;
mod scroll;
mod search;
mod send;
mod typing;

pub use emoji_picker::{CATEGORIES, EmojiPickerAction, EmojiPickerSource, EmojiPickerState};
pub use file_picker::{FilePickerOutcome, FilePickerState};
pub use image::{ImageMode, ImageRenderResult, ImageState, LinkRegion, VisibleImage};
pub use input::InputState;
pub use lock::{LockPhase, LockState};
pub use lock::{hash_passphrase, load_hash, lock_hash_path, save_hash, verify_passphrase};
pub use media::MediaState;
pub use mouse::MouseState;
pub use notification::{NotificationPreview, NotificationState};
pub use overlays::{
    ActionMenuState, ContactsOverlayState, ForwardOverlayState, GroupMenuOverlayState,
    GroupMenuState, KeybindingsOverlayState, PaletteItem, PaletteState, PinDurationOverlayState,
    PinPending, PollVoteOverlayState, PollVotePending, ProfileOverlayState, SettingsOverlayState,
    SettingsProfileOverlayState, SidebarFilterState, ThemePickerState, VerifyOverlayState,
};
pub use pending::{BufferedReceipt, PendingState};
pub use reaction::ReactionState;
pub use scroll::ScrollState;
pub use search::{SearchAction, SearchState};
pub use send::SendRequest;
pub use typing::TypingState;
