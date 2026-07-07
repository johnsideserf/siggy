//! Central application state and event handling.
//!
//! [`App`] owns conversations, input buffer, mode (Normal/Insert), and every
//! overlay state struct. [`App::handle_signal_event`] is the single entry point
//! for all backend events from `signal::client`. Conversations are keyed by
//! phone number (1:1) or group ID; the [`ConversationStore`] sub-struct holds
//! the persistent map and ordered sidebar list.

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyModifiers, MouseEvent};
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

pub use crate::autocomplete::AutocompleteMode;
use crate::autocomplete::AutocompleteState;
pub use crate::conversation_store::{Conversation, DisplayMessage, Quote};
use crate::conversation_store::{ConversationStore, db_warn};
use crate::db::Database;
use crate::domain::{
    ActionMenuState, ContactsOverlayState, EmojiPickerState, FilePickerState, ForwardOverlayState,
    GroupMenuOverlayState, ImageState, InputState, KeybindingsOverlayState, LockState, MediaState,
    MouseState, NotificationState, PaletteItem, PaletteState, PendingState,
    PinDurationOverlayState, PollVoteOverlayState, ProfileOverlayState, ReactionState, ScrollState,
    SearchAction, SearchState, SettingsOverlayState, SettingsProfileOverlayState,
    SidebarFilterState, ThemePickerState, TypingState, VerifyOverlayState,
};
// These value types were relocated into `domain` so the domain layer is a leaf
// (#495). Re-export them here so existing `crate::app::*` references across the
// handlers, UI, and main loop keep resolving unchanged.
pub use crate::domain::{
    GroupMenuState, ImageRenderResult, PinPending, PollVotePending, SendRequest, VisibleImage,
};
use crate::image_render;
use crate::image_render::ImageProtocol;
use crate::input::COMMANDS;
use crate::input::{next_char_pos, prev_char_pos};
use crate::keybindings::{self, BindingMode, KeyAction, KeyBindings};
use crate::list_overlay;
use crate::mute::MuteState;
use crate::signal::types::{MessageStatus, Reaction, SignalEvent, TrustLevel};
use crate::theme::{self, Theme};

/// Sentinel lifetime for paste temp files awaiting send confirmation from signal-cli.
/// If signal-cli never confirms, the file is deleted after this many seconds.
pub const PASTE_CLEANUP_SENTINEL_SECS: u64 = 3600;

/// How long after send confirmation to wait before deleting a paste temp file.
pub(crate) const PASTE_CLEANUP_DELAY_SECS: u64 = 10;

/// Snap a byte position to the nearest valid char boundary at or before `pos`.
pub(crate) fn floor_char_boundary(buf: &str, pos: usize) -> usize {
    let pos = pos.min(buf.len());
    if buf.is_char_boundary(pos) {
        return pos;
    }
    let mut p = pos;
    while p > 0 && !buf.is_char_boundary(p) {
        p -= 1;
    }
    p
}

/// Wire-protocol quote fields needed only for DB persistence.
///
/// `DisplayMessage.quote` holds the resolved author display name, which differs
/// from the phone-number/UUID we persist in the DB for cross-session recovery.
#[derive(Default, Clone)]
pub struct WireQuote {
    pub author: Option<String>,
    pub body: Option<String>,
    pub timestamp: Option<i64>,
}

impl App {
    /// Like `db_warn` but also surfaces the error in the status bar so the user sees it.
    pub(crate) fn db_warn_visible<T>(
        &mut self,
        result: Result<T, impl std::fmt::Display>,
        context: &str,
    ) {
        if let Err(e) = result {
            crate::debug_log::logf(format_args!("db {context}: {e}"));
            self.status_message = format!("DB error ({context}): {e}");
        }
    }

    /// Common hook for all message insertions. Handles the side effects shared
    /// by the incoming, local-send, poll, and system-message paths:
    /// - Inserts into the conversation store (ordered by timestamp or appended)
    /// - Bumps `last_read_index` if the insert came before the read marker
    /// - Increments `expiring_msg_count` when the message has a disappearing timer
    /// - Persists to the database
    /// - Moves the conversation to the top of the sidebar (refreshing the filter
    ///   if one is active)
    ///
    /// Path-specific side effects (unread counter, notifications, read receipts,
    /// scroll/focus reset) remain at the call sites; see issue #209 for further
    /// unification.
    ///
    /// Returns the index where the message was placed, or `None` if the
    /// conversation no longer exists.
    pub(crate) fn on_message_added(
        &mut self,
        conv_id: &str,
        msg: DisplayMessage,
        wire_quote: WireQuote,
        ordered_insert: bool,
    ) -> Option<usize> {
        // Snapshot the fields we need for DB persistence before the message moves.
        let ts_rfc3339 = msg.timestamp.to_rfc3339();
        let sender = msg.sender.clone();
        let sender_id = msg.sender_id.clone();
        let body = msg.body.clone();
        let is_system = msg.is_system;
        let status = msg.status;
        let timestamp_ms = msg.timestamp_ms;
        let expires_in_seconds = msg.expires_in_seconds;
        let expiration_start_ms = msg.expiration_start_ms;

        // Insert into the in-memory store.
        let (insert_idx, unarchived) = {
            let conv = self.store.conversations.get_mut(conv_id)?;
            let pos = if ordered_insert {
                conv.messages
                    .partition_point(|m| m.timestamp_ms <= timestamp_ms)
            } else {
                conv.messages.len()
            };
            conv.messages.insert(pos, msg);
            // Any new message pulls the conversation out of the archive (#611).
            let unarchived = conv.archived;
            conv.archived = false;
            (pos, unarchived)
        };
        if unarchived {
            db_warn(self.db.set_archived(conv_id, false), "set_archived");
        }

        // Bump the read marker when an ordered insert lands before it.
        if ordered_insert
            && let Some(read_idx) = self.store.last_read_index.get_mut(conv_id)
            && insert_idx <= *read_idx
        {
            *read_idx += 1;
        }

        if expires_in_seconds > 0 {
            self.expiring_msg_count += 1;
        }

        // DB persist.
        let db_result = if is_system {
            self.db.insert_message(
                conv_id,
                &sender,
                &ts_rfc3339,
                &body,
                true,
                status,
                timestamp_ms,
            )
        } else {
            self.db.insert_message_full(
                conv_id,
                &sender,
                &ts_rfc3339,
                &body,
                false,
                status,
                timestamp_ms,
                &sender_id,
                wire_quote.author.as_deref(),
                wire_quote.body.as_deref(),
                wire_quote.timestamp,
                expires_in_seconds,
                expiration_start_ms,
            )
        };
        self.db_warn_visible(db_result, "on_message_added");

        // Sidebar reorder (skip for system messages, which shouldn't bump
        // conversations to the top just because someone changed the group name).
        if !is_system
            && self.store.move_conversation_to_top(conv_id)
            && self.is_overlay(OverlayKind::SidebarFilter)
        {
            self.refresh_sidebar_filter();
        }

        Some(insert_idx)
    }
}

/// Fire an OS-level desktop notification.
///
/// Runs on `tokio::task::spawn_blocking` so it never stalls the event loop, but it
/// also does NOT deduplicate or rate-limit: each call produces one OS notification.
///
/// **Caller contract:** gate this behind the sync-burst check (`!app.sync.active`)
/// or its equivalent. Firing it from inside an initial-sync replay produces a flood
/// of "new message" toasts for messages the user already saw on their phone.
pub(crate) fn show_desktop_notification(
    sender: &str,
    body: &str,
    is_group: bool,
    group_name: Option<&str>,
    preview_level: crate::domain::NotificationPreview,
) {
    use crate::domain::NotificationPreview;
    let sender_title = || -> String {
        if is_group {
            match group_name {
                Some(gn) => format!("{} - {}", gn, sender),
                None => sender.to_string(),
            }
        } else {
            sender.to_string()
        }
    };
    let (title, preview) = match preview_level {
        NotificationPreview::Minimal => ("New message".to_string(), String::new()),
        NotificationPreview::Sender => (sender_title(), "New message".to_string()),
        NotificationPreview::Full => (sender_title(), body.chars().take(100).collect()),
    };
    // Sender/group names and message bodies are remote-controlled; strip
    // control chars before handing them to the OS notification daemon, some of
    // which interpret embedded escapes or markup (#504).
    let title = crate::debug_log::strip_control_chars(&title, false);
    let preview = crate::debug_log::strip_control_chars(&preview, false);

    tokio::task::spawn_blocking(move || {
        let _ = notify_rust::Notification::new()
            .summary(&title)
            .body(&preview)
            .timeout(notify_rust::Timeout::Milliseconds(5000))
            .show();
    });
}

/// Tag identifying which overlay is currently active.
///
/// Stored on `App.current_overlay` as the single source of truth for overlay
/// visibility. Adding a new overlay requires adding a variant here and
/// handling it in `App::handle_overlay_key`, which the compiler enforces
/// via the exhaustive match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayKind {
    SidebarFilter,
    PollVote,
    PinDuration,
    ActionMenu,
    DeleteConfirm,
    DeleteConversationConfirm,
    FilePicker,
    EmojiPicker,
    ReactionPicker,
    MessageRequest,
    GroupMenu,
    About,
    Profile,
    Help,
    Verify,
    Forward,
    Contacts,
    Search,
    SettingsProfiles,
    ThemePicker,
    Keybindings,
    Customize,
    Settings,
    Autocomplete,
    Palette,
}

// VisibleImage and ImageRenderResult moved to `domain::image` (#495);
// re-exported from `crate::domain` below.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Insert,
}

// GroupMenuState moved to `domain::overlays` (#495); re-exported below.

/// An item in the group-menu overlay (Manage members / Rename / Leave / ...).
pub struct GroupMenuItem {
    pub label: &'static str,
    pub key_hint: GroupMenuHint,
    pub nerd_icon: &'static str,
}

/// Compile-time-checked discriminator for the group menu, mirroring
/// [`ActionMenuHint`]. Replaces the previous stringly-typed `key_hint` whose
/// `transition_group_menu` match had a silent `_ => {}` fallthrough: a typo in
/// either the builder or the dispatcher compiled fine and produced a menu item
/// that did nothing. Now adding an entry forces the exhaustive match to handle
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupMenuHint {
    Members,
    AddMember,
    RemoveMember,
    Rename,
    Leave,
    Create,
}

impl GroupMenuHint {
    /// Single-letter shortcut, used both as the display hint and the binding.
    pub fn key_char(self) -> char {
        match self {
            Self::Members => 'm',
            Self::AddMember => 'a',
            Self::RemoveMember => 'r',
            Self::Rename => 'n',
            Self::Leave => 'l',
            Self::Create => 'c',
        }
    }

    /// Parse a keypress into a hint, or `None` if it maps to no menu item.
    pub fn from_char(c: char) -> Option<Self> {
        Some(match c {
            'm' => Self::Members,
            'a' => Self::AddMember,
            'r' => Self::RemoveMember,
            'n' => Self::Rename,
            'l' => Self::Leave,
            'c' => Self::Create,
            _ => return None,
        })
    }
}

/// An item in the per-message action-menu overlay (Reply / Edit / React / ...).
/// `key_hint` is type-safe: every variant of [`ActionMenuHint`] must be handled
/// by [`App::execute_action_by_hint`] (compiler-enforced exhaustive match), and
/// adding a new action means adding a variant, an `action_menu_items` entry,
/// and an `execute_action_by_hint` arm at the same time.
pub struct ActionMenuItem {
    pub label: &'static str,
    pub key_hint: ActionMenuHint,
    pub nerd_icon: &'static str,
}

/// Compile-time-checked discriminator for the per-message action menu.
///
/// Each variant maps to exactly one keyboard shortcut character. Changes here
/// force the dispatcher and the menu builder to be updated in lockstep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionMenuHint {
    Reply,
    Edit,
    React,
    Forward,
    Copy,
    Delete,
    PinToggle,
    Vote,
    EndPoll,
    OpenAttachment,
    OpenLink,
}

impl ActionMenuHint {
    /// Single-letter shortcut for this action. Used both as the display hint
    /// in the menu overlay and as the keyboard binding.
    pub fn key_char(self) -> char {
        match self {
            Self::Reply => 'q',
            Self::Edit => 'e',
            Self::React => 'r',
            Self::Forward => 'f',
            Self::Copy => 'y',
            Self::Delete => 'd',
            Self::PinToggle => 'p',
            Self::Vote => 'v',
            Self::EndPoll => 'x',
            Self::OpenAttachment => 'o',
            Self::OpenLink => 'l',
        }
    }

    /// Parse a keypress back into a hint. Returns `None` for any char that
    /// doesn't correspond to a menu action.
    pub fn from_char(c: char) -> Option<Self> {
        Some(match c {
            'q' => Self::Reply,
            'e' => Self::Edit,
            'r' => Self::React,
            'f' => Self::Forward,
            'y' => Self::Copy,
            'd' => Self::Delete,
            'p' => Self::PinToggle,
            'v' => Self::Vote,
            'x' => Self::EndPoll,
            'o' => Self::OpenAttachment,
            'l' => Self::OpenLink,
            _ => return None,
        })
    }
}

// PinPending and PollVotePending moved to `domain::overlays` (#495);
// re-exported below.

/// Tracks the initial sync burst when the app starts.
/// During sync, notifications and viewport jumps are suppressed.
pub struct SyncState {
    /// Whether the sync burst is still considered active.
    pub active: bool,
    /// Number of messages received since sync started.
    pub message_count: usize,
    /// Time the most recent message arrived (None if no messages yet).
    pub last_message_time: Option<Instant>,
    /// Time when sync started (used to enforce a minimum quiet period).
    pub started_at: Instant,
    /// Notifications suppressed per conversation during sync (conv_id → count).
    pub suppressed_notifications: HashMap<String, usize>,
    /// True if the user manually scrolled the viewport during sync.
    pub user_scrolled: bool,
    /// Viewport pin anchor for the active conversation: (timestamp of the
    /// message that was at viewport bottom when sync started, scroll.offset
    /// at that moment). Captured once per active-conversation sync session;
    /// used by the chat-pane renderer to keep that message at its original
    /// screen position regardless of how many lines incoming messages add.
    /// Cleared on conversation switch and on sync end.
    pub pin: Option<(DateTime<Utc>, usize)>,
}

impl SyncState {
    pub fn new() -> Self {
        Self {
            active: true,
            message_count: 0,
            last_message_time: None,
            started_at: Instant::now(),
            suppressed_notifications: HashMap::new(),
            user_scrolled: false,
            pin: None,
        }
    }

    /// Returns true when sync should end: at least 10 s have elapsed since start,
    /// and either no messages have been received or the last one arrived >= 3 s ago.
    pub fn should_end(&self) -> bool {
        let elapsed = self.started_at.elapsed();
        if elapsed.as_secs() < 10 {
            return false;
        }
        match self.last_message_time {
            None => true,
            Some(last) => last.elapsed().as_secs() >= 3,
        }
    }
}

/// Application state
pub struct App {
    /// Conversation data: conversations, ordering, contact names, groups, read markers.
    pub store: ConversationStore,
    /// Currently selected conversation ID
    pub active_conversation: Option<String>,
    /// Message composer: text buffer, cursor, and history recall.
    pub input: InputState,
    /// Whether sidebar is visible
    pub sidebar_visible: bool,
    /// Messages-pane scroll viewport, focus cursor, jump stack, and per-conversation
    /// saved positions.
    pub scroll: ScrollState,
    /// Status bar message
    pub status_message: String,
    /// Whether the app should quit
    pub should_quit: bool,
    /// Message trigger rules and cooldown state (#615). Empty in tests and
    /// demo mode; loaded from triggers.toml at startup and by /triggers.
    pub triggers: crate::trigger::TriggerEngine,
    /// Our own account number for identifying outgoing messages
    pub account: String,
    /// Resizable sidebar width (min 14, max 40)
    pub sidebar_width: u16,
    /// Display sidebar on the right side instead of left
    pub sidebar_on_right: bool,
    /// Sidebar type-to-filter overlay state (query + matching IDs).
    pub sidebar_filter: SidebarFilterState,
    /// Fuzzy command palette overlay state (#614).
    pub palette: PaletteState,
    /// Typing indicator state (inbound indicators + outbound typing tracking).
    pub typing: TypingState,
    /// Whether we are connected to signal-cli
    pub connected: bool,
    /// True until the first ContactList event arrives (initial sync in progress)
    pub loading: bool,
    /// Status message shown on the loading screen (e.g. "Loading contacts...")
    pub startup_status: String,
    /// Tick counter for the loading spinner animation
    pub spinner_tick: usize,
    /// Current input mode (Normal or Insert)
    pub mode: InputMode,
    /// SQLite database for persistent storage
    pub db: Database,
    /// Persistent error from signal-cli connection failure
    pub connection_error: Option<String>,
    /// Notification preferences and clipboard auto-clear state
    pub notifications: NotificationState,
    /// Conversations muted from notifications, keyed by conversation id.
    pub muted_conversations: HashMap<String, MuteState>,
    /// Conversations blocked via signal-cli
    pub blocked_conversations: HashSet<String>,
    /// Autocomplete popup state: candidates, selection, pending mentions.
    pub autocomplete: AutocompleteState,
    /// Settings overlay state (cursor, customize sub-menu cursor, mouse snapshot)
    pub settings_overlay: SettingsOverlayState,
    /// State for the contacts list overlay
    pub contacts_overlay: ContactsOverlayState,
    /// State for the identity verification overlay
    pub verify: VerifyOverlayState,
    /// Cached trust levels keyed by phone number.
    /// Populated: IdentityList events (full clear + repopulate on each event).
    /// Refreshed: startup via list_identities() RPC, and after verify/trust actions.
    pub identity_trust: HashMap<String, TrustLevel>,
    /// Image rendering, caching, and link overlay state.
    pub image: ImageState,
    /// Previous active conversation ID, for detecting chat switches
    pub prev_active_conversation: Option<String>,
    /// Incognito mode — in-memory DB, no local persistence
    pub incognito: bool,
    /// Media handling: attachment download dir and voice playback override.
    pub media: MediaState,
    /// Show date separator lines between messages from different days
    pub date_separators: bool,
    /// Show delivery/read receipt status symbols on outgoing messages
    pub show_receipts: bool,
    /// Use colored status symbols (vs monochrome DarkGray)
    pub color_receipts: bool,
    /// Use Nerd Font glyphs for status symbols
    pub nerd_fonts: bool,
    /// In-flight signal-cli work awaiting confirmation or dispatch:
    /// pending sends, out-of-order receipts, queued typing-stop, and queued read receipts.
    pub pending: PendingState,
    /// Pending normal-mode prefix key (e.g. first `g` of `gg`, first `d` of `dd`)
    pub pending_normal_key: Option<char>,
    /// Reaction display preferences and picker overlay state
    pub reactions: ReactionState,
    /// Emoji picker overlay state
    pub emoji_picker: EmojiPickerState,
    /// Demo mode — prevents config writes
    pub is_demo: bool,
    /// File browser overlay state
    pub file_picker: FilePickerState,
    /// File selected for sending as attachment
    pub pending_attachment: Option<PathBuf>,
    /// Directory for temporary clipboard paste files (PID-scoped to avoid conflicts)
    pub paste_temp_path: PathBuf,
    /// Paste temp files pending deletion: rpc_id → (path, delete_after)
    /// Populated when a paste attachment send is dispatched; deletion deferred 10s after
    /// signal-cli confirms or fails the send, to avoid deleting before signal-cli reads the file.
    pub pending_paste_cleanups: HashMap<String, (PathBuf, Instant)>,
    /// Reply target: (author_phone, body_snippet, timestamp_ms)
    pub reply_target: Option<(String, String, i64)>,
    /// Message being edited: (timestamp_ms, conv_id)
    pub editing_message: Option<(i64, String)>,
    /// Search overlay state
    pub search: SearchState,
    /// Send read receipts to message senders when viewing conversations
    pub send_read_receipts: bool,
    /// Action menu overlay state
    pub action_menu: ActionMenuState,
    /// Forward message picker overlay state
    pub forward: ForwardOverlayState,
    /// Group management menu overlay state
    pub group_menu: GroupMenuOverlayState,
    /// Mouse hit-test areas, capture flag, and queued capture toggle.
    pub mouse: MouseState,
    /// Active color theme
    pub theme: Theme,
    /// Theme picker overlay state
    pub theme_picker: ThemePickerState,
    /// Active keybindings
    pub keybindings: KeyBindings,
    /// Keybindings overlay state
    pub keybindings_overlay: KeybindingsOverlayState,
    /// Pin duration picker overlay state
    pub pin_duration: PinDurationOverlayState,
    /// Poll vote overlay state
    pub poll_vote: PollVoteOverlayState,
    /// Number of in-memory messages with expiration > 0 (skip sweeps when zero)
    pub expiring_msg_count: usize,
    /// Profile editor overlay state
    pub profile: ProfileOverlayState,
    /// Settings profile overlay state
    pub settings_profiles: SettingsProfileOverlayState,
    /// Sync state: tracks the initial message burst on startup.
    pub sync: SyncState,
    /// Currently active overlay, when one is open.
    ///
    /// Single source of truth for overlay visibility. Drive all writes
    /// through `open_overlay`/`close_overlay`/`try_open_overlay` so callers
    /// can't accidentally bypass the dispatch layer. Per-overlay state
    /// (filter text, cursor index, candidate lists) lives on the per-overlay
    /// state structs and persists across close/reopen.
    pub current_overlay: Option<OverlayKind>,
    /// Session-lock state (boss-key / auto-lock).
    pub lock: LockState,
}

pub const QUICK_REACTIONS: &[&str] = &[
    "\u{1f44d}",
    "\u{1f44e}",
    "\u{2764}\u{fe0f}",
    "\u{1f602}",
    "\u{1f62e}",
    "\u{1f622}",
    "\u{1f64f}",
    "\u{1f525}",
];

pub const PIN_DURATIONS: &[(i64, &str)] = &[
    (-1, "Forever"),
    (86400, "24 hours"),
    (604800, "7 days"),
    (2592000, "30 days"),
];

// SendRequest moved to `domain::send` so domain stays a leaf layer (#495).
// Re-exported below from `crate::domain` alongside the other moved value types.

/// A single settings toggle entry: label, getter, setter, and optional config
/// persistence. `save` (App -> Config) and `load` (Config -> App) are a pair:
/// a persisted setting must be loadable and vice versa, or it silently works in
/// only one direction (#498). The pairing is enforced by a test, and startup
/// applies `load` through [`App::apply_settings_from_config`] rather than
/// hand-copying each field.
pub struct SettingDef {
    pub label: &'static str,
    pub hint: &'static str,
    get: fn(&App) -> bool,
    set: fn(&mut App, bool),
    save: Option<fn(&mut crate::config::Config, bool)>,
    load: Option<fn(&crate::config::Config) -> bool>,
}

/// Section boundary indices within the SETTINGS array.
pub const SETTINGS_SECTION_DISPLAY: usize = 3;
pub const SETTINGS_SECTION_MESSAGES: usize = 10;
pub const SETTINGS_SECTION_INTERFACE: usize = 13;

/// Visual order of settings items (logical indices into the combined toggle+special list).
/// Toggle indices 0..SETTINGS.len() map to SETTINGS entries.
/// Special indices: SETTINGS.len() = preview, +1 = image mode, +2 = customize.
/// Navigation walks this array so j/k follows the visual layout.
pub const SETTINGS_VISUAL_ORDER: &[usize] = &[
    // Notifications
    0, 1, 2, 16, // DM, Group, Desktop, Notification preview
    // Display
    3, 4, 5, 6, 7, 8, 9, 17, // Link previews .. Show usernames, Image mode
    // Messages
    10, 11, 12, // Show reactions, Verbose reactions, Send read receipts
    // Interface
    13, 14, 15, 18, // Sidebar visible, Mouse, Sidebar on right, Customize...
];

pub const SETTINGS: &[SettingDef] = &[
    // — Notifications (0–2) —
    SettingDef {
        label: "Direct message notifications",
        hint: "Play a sound for incoming direct messages",
        get: |a| a.notifications.notify_direct,
        set: |a, v| a.notifications.notify_direct = v,
        save: Some(|c, v| c.notify_direct = v),
        load: Some(|c| c.notify_direct),
    },
    SettingDef {
        label: "Group message notifications",
        hint: "Play a sound for incoming group messages",
        get: |a| a.notifications.notify_group,
        set: |a, v| a.notifications.notify_group = v,
        save: Some(|c, v| c.notify_group = v),
        load: Some(|c| c.notify_group),
    },
    SettingDef {
        label: "Desktop notifications",
        hint: "Show system notifications for new messages",
        get: |a| a.notifications.desktop_notifications,
        set: |a, v| a.notifications.desktop_notifications = v,
        save: Some(|c, v| c.desktop_notifications = v),
        load: Some(|c| c.desktop_notifications),
    },
    // — Display (3–8) —
    SettingDef {
        label: "Link previews",
        hint: "Show title and thumbnail for URLs",
        get: |a| a.image.show_link_previews,
        set: |a, v| a.image.show_link_previews = v,
        save: Some(|c, v| c.show_link_previews = v),
        load: Some(|c| c.show_link_previews),
    },
    SettingDef {
        label: "Date separators",
        hint: "Show date lines between messages from different days",
        get: |a| a.date_separators,
        set: |a, v| a.date_separators = v,
        save: Some(|c, v| c.date_separators = v),
        load: Some(|c| c.date_separators),
    },
    SettingDef {
        label: "Read receipts",
        hint: "Show delivery and read status on messages",
        get: |a| a.show_receipts,
        set: |a, v| a.show_receipts = v,
        save: Some(|c, v| c.show_receipts = v),
        load: Some(|c| c.show_receipts),
    },
    SettingDef {
        label: "Receipt colors",
        hint: "Colorize receipt indicators",
        get: |a| a.color_receipts,
        set: |a, v| a.color_receipts = v,
        save: Some(|c, v| c.color_receipts = v),
        load: Some(|c| c.color_receipts),
    },
    SettingDef {
        label: "Nerd Font icons",
        hint: "Use Nerd Font glyphs (requires a Nerd Font)",
        get: |a| a.nerd_fonts,
        set: |a, v| a.nerd_fonts = v,
        save: Some(|c, v| c.nerd_fonts = v),
        load: Some(|c| c.nerd_fonts),
    },
    SettingDef {
        label: "Emoji to text",
        hint: "Convert emoji to text emoticons/shortcodes",
        get: |a| a.reactions.emoji_to_text,
        set: |a, v| a.reactions.emoji_to_text = v,
        save: Some(|c, v| c.emoji_to_text = v),
        load: Some(|c| c.emoji_to_text),
    },
    SettingDef {
        label: "Show usernames",
        hint: "Show @handle next to 1:1 conversation names",
        get: |a| a.store.show_usernames,
        set: |a, v| a.store.show_usernames = v,
        save: Some(|c, v| c.show_usernames = v),
        load: Some(|c| c.show_usernames),
    },
    // — Messages (10–12) —
    SettingDef {
        label: "Show reactions",
        hint: "Show emoji reactions on messages",
        get: |a| a.reactions.show_reactions,
        set: |a, v| a.reactions.show_reactions = v,
        save: Some(|c, v| c.show_reactions = v),
        load: Some(|c| c.show_reactions),
    },
    SettingDef {
        label: "Verbose reactions",
        hint: "Show names instead of just emoji counts",
        get: |a| a.reactions.verbose,
        set: |a, v| a.reactions.verbose = v,
        save: Some(|c, v| c.reaction_verbose = v),
        load: Some(|c| c.reaction_verbose),
    },
    SettingDef {
        label: "Send read receipts",
        hint: "Let contacts know when you read messages",
        get: |a| a.send_read_receipts,
        set: |a, v| a.send_read_receipts = v,
        save: Some(|c, v| c.send_read_receipts = v),
        load: Some(|c| c.send_read_receipts),
    },
    // — Interface (13–15) —
    SettingDef {
        label: "Sidebar visible",
        hint: "Show the conversation list sidebar",
        get: |a| a.sidebar_visible,
        set: |a, v| a.sidebar_visible = v,
        save: None, // runtime-only, not persisted
        load: None,
    },
    SettingDef {
        label: "Mouse support",
        hint: "Enable mouse click and scroll support",
        get: |a| a.mouse.enabled,
        set: |a, v| a.mouse.enabled = v,
        save: Some(|c, v| c.mouse_enabled = v),
        load: Some(|c| c.mouse_enabled),
    },
    SettingDef {
        label: "Sidebar on right",
        hint: "Move the sidebar to the right side",
        get: |a| a.sidebar_on_right,
        set: |a, v| a.sidebar_on_right = v,
        save: Some(|c, v| c.sidebar_on_right = v),
        load: Some(|c| c.sidebar_on_right),
    },
];

impl App {
    pub fn toggle_setting(&mut self, index: usize) {
        if let Some(def) = SETTINGS.get(index) {
            let cur = (def.get)(self);
            (def.set)(self, !cur);
        }
    }

    /// Apply every table-driven toggle from `config` into this `App`. The
    /// inverse of the `save` loop in `save_settings`; both go through the same
    /// SETTINGS table so a new toggle cannot silently load or persist in only
    /// one direction (#498).
    pub fn apply_settings_from_config(&mut self, config: &crate::config::Config) {
        for def in SETTINGS {
            if let Some(load) = def.load {
                (def.set)(self, load(config));
            }
        }
    }

    pub fn setting_value(&self, index: usize) -> bool {
        SETTINGS.get(index).is_some_and(|def| (def.get)(self))
    }

    /// Persist current settings to the config file.
    pub(crate) fn save_settings(&self) {
        if self.is_demo {
            return;
        }
        let mut config = crate::config::Config::load(None).unwrap_or_default();
        config.account = self.account.clone();
        config.theme = self.theme.name.clone();
        config.keybinding_profile = self.keybindings.profile_name.clone();
        config.settings_profile = self.settings_profiles.name.clone();
        config.notification_preview = self.notifications.notification_preview;
        config.image_mode = Some(self.image.image_mode);
        config.image_max_width = self.image.image_max_width;
        config.preview_image_max_width = self.image.preview_image_max_width;
        config.image_max_height = self.image.image_max_height;
        config.sixel_max_colors = self.image.sixel_encode.max_colors;
        config.sixel_diffusion = self.image.sixel_encode.diffusion;
        config.sidebar_width = self.sidebar_width;
        for def in SETTINGS {
            if let Some(save_fn) = def.save {
                save_fn(&mut config, (def.get)(self));
            }
        }
        if let Err(e) = config.save() {
            crate::debug_log::logf(format_args!("settings save error: {e}"));
        }
        // Persist in-app keybinding rebinds
        let overrides = self.keybindings.diff_from_profile();
        keybindings::save_overrides(&overrides);
    }

    // Image lines are always cached in memory; the UI checks image_mode/show_link_previews
    // before displaying them. No refresh needed on toggle — it's just a visibility flag now.

    /// Drain completed background image renders and spawn new ones for the viewport.
    /// Called each frame from the main loop. Returns true if any images were applied.
    pub fn ensure_active_images(&mut self) -> bool {
        // Always drain completed background renders (even if inline_images is off)
        let mut drained = false;
        while let Ok(result) = self.image.image_render_rx.try_recv() {
            self.image.image_render_in_flight.remove(&(
                result.conv_id.clone(),
                result.timestamp_ms,
                result.is_preview,
            ));
            if let Some(conv) = self.store.conversations.get_mut(&result.conv_id)
                && let Some(idx) = conv.find_msg_idx(result.timestamp_ms)
            {
                if result.is_preview {
                    // Store empty vec on None to prevent infinite retry for broken images
                    conv.messages[idx].preview_image_lines = Some(result.lines.unwrap_or_default());
                    if let Some(p) = result.image_path {
                        conv.messages[idx].preview_image_path = Some(p);
                    }
                } else {
                    conv.messages[idx].image_lines = Some(result.lines.unwrap_or_default());
                }
                // Pre-populate native image caches from background task
                if let Some((path, b64, pw, ph)) = result.pre_native_png {
                    self.image.native_image_cache.insert(path, (b64, pw, ph));
                }
                if let Some((path, sixel)) = result.pre_sixel {
                    self.image.sixel_cache.insert(path, sixel);
                }
                drained = true;
            }
        }

        if self.image.image_mode == crate::domain::ImageMode::None {
            return drained;
        }
        let Some(ref id) = self.active_conversation else {
            return drained;
        };
        let id = id.clone();
        let Some(conv) = self.store.conversations.get(&id) else {
            return drained;
        };
        let len = conv.messages.len();
        if len == 0 {
            return drained;
        }
        let is_native = self.image.image_mode == crate::domain::ImageMode::Native;
        let is_sixel = self.image.image_protocol == image_render::ImageProtocol::Sixel;
        let pane_width_cap = if self.image.render_width_cap > 0 {
            self.image.render_width_cap
        } else {
            self.image.image_max_width
        };
        let image_max_width = self.image.image_max_width.min(pane_width_cap).max(1);
        let preview_image_max_width = self
            .image
            .preview_image_max_width
            .min(pane_width_cap)
            .max(1);

        // Skip the viewport scan when nothing that affects it has changed since
        // the last scan. ensure_active_images runs every 50ms tick; without this
        // gate it rescans up to 60 messages and rebuilds probe keys on every idle
        // frame (#492). The signature observes the scan's inputs directly (conv,
        // scroll, message count, image settings) so it can't miss a trigger the
        // way per-mutation dirty flags can. A completed background render
        // (`drained`) frees an in-flight slot, so always rescan after one.
        let signature = (
            id.clone(),
            self.scroll.offset,
            len,
            self.image.image_mode,
            self.image.show_link_previews,
            self.image.render_width_cap,
        );
        if !drained && self.image.scan_signature.as_ref() == Some(&signature) {
            return drained;
        }
        self.image.scan_signature = Some(signature);

        let end = len
            .saturating_sub(self.scroll.offset.saturating_sub(5))
            .min(len);
        let start = end.saturating_sub(60);

        // Collect work items to avoid borrow conflicts:
        // (timestamp, path, max_width, max_height_cells, is_preview)
        let mut work: Vec<(i64, String, u32, u32, bool)> = Vec::new();
        for msg in &conv.messages[start..end] {
            if self.image.image_render_in_flight.len() + work.len() >= 4 {
                break;
            }
            if msg.body.starts_with("[image:")
                && let Some(ref p) = msg.image_path
            {
                let has_rendered_lines = msg
                    .image_lines
                    .as_ref()
                    .is_some_and(|lines| !lines.is_empty());
                let image_too_wide = msg.image_lines.as_ref().is_some_and(|lines| {
                    lines
                        .first()
                        .is_some_and(|line| line.width().saturating_sub(2) as u32 > image_max_width)
                });
                let native_cache_missing = is_native
                    && has_rendered_lines
                    && if is_sixel {
                        !self.image.sixel_cache.contains_key(p)
                    } else {
                        !self.image.native_image_cache.contains_key(p)
                    };
                let key = (id.clone(), msg.timestamp_ms, false);
                if (msg.image_lines.is_none() || image_too_wide || native_cache_missing)
                    && !self.image.image_render_in_flight.contains(&key)
                {
                    work.push((
                        msg.timestamp_ms,
                        p.clone(),
                        image_max_width,
                        self.image.image_max_height,
                        false,
                    ));
                }
            }
            if self.image.show_link_previews
                && let Some(ref preview) = msg.preview
                && let Some(ref p) = preview.image_path
            {
                let has_rendered_lines = msg
                    .preview_image_lines
                    .as_ref()
                    .is_some_and(|lines| !lines.is_empty());
                let image_too_wide = msg.preview_image_lines.as_ref().is_some_and(|lines| {
                    lines.first().is_some_and(|line| {
                        line.width().saturating_sub(2) as u32 > preview_image_max_width
                    })
                });
                let native_cache_missing = is_native
                    && has_rendered_lines
                    && if is_sixel {
                        !self.image.sixel_cache.contains_key(p)
                    } else {
                        !self.image.native_image_cache.contains_key(p)
                    };
                let key = (id.clone(), msg.timestamp_ms, true);
                if (msg.preview_image_lines.is_none() || image_too_wide || native_cache_missing)
                    && !self.image.image_render_in_flight.contains(&key)
                {
                    work.push((
                        msg.timestamp_ms,
                        p.clone(),
                        preview_image_max_width,
                        self.image.image_max_height,
                        true,
                    ));
                }
            }
        }

        // Evict cached image_lines for messages before the render window so they
        // don't accumulate across a long scrolling session (#492). Those messages
        // are never drawn this frame (chat_pane records render_window_start), and
        // they regenerate via the work path above if scrolled back into view.
        let evict_before = self.scroll.render_window_start.min(start);
        if evict_before > 0
            && let Some(conv) = self.store.conversations.get_mut(&id)
        {
            let upto = evict_before.min(conv.messages.len());
            for msg in &mut conv.messages[..upto] {
                if msg.image_lines.is_some() {
                    msg.image_lines = None;
                }
                if msg.preview_image_lines.is_some() {
                    msg.preview_image_lines = None;
                }
            }
        }

        // Spawn background render tasks
        let cell_px = self.image.cell_px;
        let sixel_encode = self.image.sixel_encode;
        for (ts, path, max_width, max_height, is_preview) in work {
            self.image
                .image_render_in_flight
                .insert((id.clone(), ts, is_preview));
            let tx = self.image.image_render_tx.clone();
            let cid = id.clone();
            tokio::task::spawn_blocking(move || {
                let lines =
                    image_render::render_image_with_limits(Path::new(&path), max_width, max_height);

                // Pre-encode PNG (all native protocols) and Sixel alongside halfblock
                // so caches are populated before the image first appears in the viewport.
                // Without this, Kitty/iTerm2 would encode synchronously on first scroll-in.
                let (pre_native_png, pre_sixel) = if is_native {
                    let cell_w = lines
                        .as_ref()
                        .and_then(|l| l.first())
                        .map(|l| l.width().saturating_sub(2) as u32)
                        .unwrap_or(0);
                    let cell_h = lines.as_ref().map(|l| l.len() as u32).unwrap_or(0);
                    if cell_w > 0 && cell_h > 0 {
                        let png = image_render::encode_native_png(Path::new(&path), cell_w, cell_h);
                        let sixel = if is_sixel {
                            png.as_ref().and_then(|p| {
                                image_render::encode_sixel(
                                    &p.0,
                                    cell_w as u16,
                                    cell_h as u16,
                                    cell_px,
                                    sixel_encode,
                                )
                            })
                        } else {
                            None
                        };
                        let pre_png = png.map(|(b64, pw, ph)| (path.clone(), b64, pw, ph));
                        let pre_six = sixel.map(|s| (path.clone(), s));
                        (pre_png, pre_six)
                    } else {
                        (None, None)
                    }
                } else {
                    (None, None)
                };

                let _ = tx.send(ImageRenderResult {
                    conv_id: cid,
                    timestamp_ms: ts,
                    is_preview,
                    lines,
                    image_path: if is_preview { Some(path) } else { None },
                    pre_native_png,
                    pre_sixel,
                });
            });
        }

        drained
    }

    /// Handle a key press while the settings overlay is open.
    /// Navigation follows SETTINGS_VISUAL_ORDER so j/k matches the visual layout.
    /// After toggles: Preview at SETTINGS.len(), Image mode at +1, Customize at +2.
    pub fn handle_settings_key(&mut self, code: KeyCode) {
        crate::handlers::keys::handle_settings_key(self, code)
    }

    /// Handle a key press in the Customize sub-menu (Theme, Keybindings, Profile).
    pub fn handle_customize_key(&mut self, code: KeyCode) {
        crate::handlers::keys::handle_customize_key(self, code)
    }

    /// Apply a profile without firing expensive hooks (image re-rendering).
    /// Hooks fire when the overlay closes (settings or profile manager Esc handler).
    pub(crate) fn apply_settings_profile_deferred(
        &mut self,
        profile: &crate::settings_profile::SettingsProfile,
    ) {
        profile.apply_to(self);
        self.settings_profiles.name = profile.name.clone();
    }

    /// Fire deferred side-effects for settings that changed since the overlay opened.
    /// Currently just the mouse-capture toggle: applying it immediately on each
    /// keystroke would clobber input mid-edit, so we queue the new state and
    /// `main.rs` flips the terminal mode on the next tick.
    pub(crate) fn fire_deferred_settings_hooks(&mut self) {
        if self.mouse.enabled != self.settings_overlay.mouse_snapshot {
            self.mouse.pending_toggle = Some(self.mouse.enabled);
        }
    }

    /// Open the settings profile manager overlay.
    pub(crate) fn open_settings_profile_manager(&mut self) {
        self.settings_profiles.available = crate::settings_profile::all_settings_profiles();
        self.settings_profiles.index = self
            .settings_profiles
            .available
            .iter()
            .position(|p| p.name == self.settings_profiles.name)
            .unwrap_or(0);
        self.open_overlay(OverlayKind::SettingsProfiles);
        self.settings_profiles.save_as = false;
        self.settings_profiles.save_as_input.clear();
        // Don't overwrite settings_snapshot - keep the one from when /settings opened
    }

    /// Handle a key press while the settings profile manager is open.
    pub fn handle_settings_profile_manager_key(&mut self, code: KeyCode) {
        crate::handlers::keys::handle_settings_profile_manager_key(self, code)
    }

    /// Handle a key press while the theme picker overlay is open.
    pub fn handle_theme_key(&mut self, code: KeyCode) {
        crate::handlers::keys::handle_theme_key(self, code)
    }

    /// Handle a key press while the keybindings overlay is open.
    pub fn handle_keybindings_key(&mut self, code: KeyCode) {
        crate::handlers::keys::handle_keybindings_key(self, code)
    }

    /// Handle keybinding capture: intercepts ALL keys when capturing a new binding.
    pub fn handle_keybinding_capture(&mut self, modifiers: KeyModifiers, code: KeyCode) {
        crate::handlers::keys::handle_keybinding_capture(self, modifiers, code)
    }

    /// Total number of rows in the keybindings overlay (profile + sections + actions).
    pub fn keybindings_overlay_total(&self) -> usize {
        // profile row + 3 section headers + action counts
        1 + 1
            + keybindings::GLOBAL_ACTIONS.len()
            + 1
            + keybindings::NORMAL_ACTIONS.len()
            + 1
            + keybindings::INSERT_ACTIONS.len()
    }

    /// Get the (mode, action) for a keybindings overlay row index.
    /// Returns (mode, None) for section headers and the profile row.
    pub fn keybindings_overlay_item(&self, index: usize) -> (BindingMode, Option<KeyAction>) {
        if index == 0 {
            return (BindingMode::Global, None); // profile row
        }
        let mut i = 1;
        // Global section header
        if index == i {
            return (BindingMode::Global, None);
        }
        i += 1;
        if index < i + keybindings::GLOBAL_ACTIONS.len() {
            return (
                BindingMode::Global,
                Some(keybindings::GLOBAL_ACTIONS[index - i]),
            );
        }
        i += keybindings::GLOBAL_ACTIONS.len();
        // Normal section header
        if index == i {
            return (BindingMode::Normal, None);
        }
        i += 1;
        if index < i + keybindings::NORMAL_ACTIONS.len() {
            return (
                BindingMode::Normal,
                Some(keybindings::NORMAL_ACTIONS[index - i]),
            );
        }
        i += keybindings::NORMAL_ACTIONS.len();
        // Insert section header
        if index == i {
            return (BindingMode::Insert, None);
        }
        i += 1;
        if index < i + keybindings::INSERT_ACTIONS.len() {
            return (
                BindingMode::Insert,
                Some(keybindings::INSERT_ACTIONS[index - i]),
            );
        }
        (BindingMode::Insert, None)
    }

    /// Build the filtered contacts list from contact_names using the current filter.
    /// Whether a contacts-overlay row identifier already has a conversation.
    /// Rows display `@handle` for uuid-keyed contacts, so handles must be
    /// resolved back to the conversation key before the lookup (#612).
    pub fn contact_row_has_conversation(&self, id: &str) -> bool {
        let key = id
            .strip_prefix('@')
            .and_then(|handle| self.store.username_to_id.get(&handle.to_lowercase()))
            .map(|k| k.as_str())
            .unwrap_or(id);
        self.store.conversation_order.iter().any(|c| c == key)
    }

    pub fn refresh_contacts_filter(&mut self) {
        let pairs: Vec<(String, String)> = self
            .store
            .contact_names
            .iter()
            .filter(|(_, name)| !name.is_empty())
            .map(|(key, name)| {
                // uuid-keyed (username-only) contacts show their @handle
                // instead of an opaque uuid; @handles re-resolve on select
                // via the /join username path (#612)
                let id = if !key.starts_with('+')
                    && let Some(username) = self.store.usernames.get(key)
                {
                    format!("@{username}")
                } else {
                    key.clone()
                };
                (id, name.clone())
            })
            .collect();
        self.contacts_overlay.filtered =
            list_overlay::filter_name_number_pairs(pairs, &self.contacts_overlay.filter);
        list_overlay::clamp_index(
            &mut self.contacts_overlay.index,
            self.contacts_overlay.filtered.len(),
        );
    }

    /// Drain the background `/preview` fetch, if one is running (#267).
    /// Returns true when a result was applied (so the caller can redraw).
    pub fn poll_preview_fetch(&mut self) -> bool {
        let Some(rx) = &self.media.preview_rx else {
            return false;
        };
        match rx.try_recv() {
            Ok(Ok(preview)) => {
                self.status_message = format!(
                    "preview ready: {} (sends with your next message; /preview to discard)",
                    preview.title.as_deref().unwrap_or(&preview.url)
                );
                self.media.pending_preview = Some(preview);
                self.media.preview_rx = None;
                true
            }
            Ok(Err(e)) => {
                self.status_message = format!("preview failed: {e}");
                self.media.preview_rx = None;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.status_message = "preview failed: fetch thread died".to_string();
                self.media.preview_rx = None;
                true
            }
        }
    }

    /// Kill and reap the voice player child, if one is playing (#618).
    /// Returns true when something was actually stopped.
    pub(crate) fn stop_voice_playback(&mut self) -> bool {
        if let Some(mut playing) = self.media.playing.take() {
            let _ = playing.child.kill();
            let _ = playing.child.wait();
            true
        } else {
            false
        }
    }

    /// Advance voice-playback bookkeeping each tick (#618): reap the player
    /// when it exits and keep the status-bar progress line fresh. Returns
    /// true when the status line changed (so the caller redraws).
    pub fn tick_voice_playback(&mut self) -> bool {
        let Some(playing) = &mut self.media.playing else {
            return false;
        };
        if matches!(playing.child.try_wait(), Ok(Some(_))) {
            self.media.playing = None;
            self.update_status();
            return true;
        }
        let elapsed = playing.started.elapsed();
        let line = match playing.duration {
            Some(total) => format!(
                "playing {} {} / {} (o to stop)",
                playing.label,
                crate::audio::format_mmss(elapsed.min(total)),
                crate::audio::format_mmss(total)
            ),
            None => format!(
                "playing {} {} (o to stop)",
                playing.label,
                crate::audio::format_mmss(elapsed)
            ),
        };
        if self.status_message != line {
            self.status_message = line;
            true
        } else {
            false
        }
    }

    /// Open the fuzzy command palette (#614).
    pub(crate) fn open_palette(&mut self) {
        self.palette.query.clear();
        self.palette.index = 0;
        self.refresh_palette_filter();
        self.open_overlay(OverlayKind::Palette);
    }

    /// Rebuild the palette's filtered list: conversations and commands scored
    /// against the query, best match first. With an empty query every item
    /// scores 0 and the stable sort keeps conversations (in sidebar order)
    /// ahead of the command list.
    pub(crate) fn refresh_palette_filter(&mut self) {
        let query = self.palette.query.clone();
        // (score, kind_rank, item); kind_rank breaks score ties in favor of
        // conversations, since jump-to-chat is the common palette use.
        let mut scored: Vec<(i64, i64, PaletteItem)> = Vec::new();
        for id in &self.store.conversation_order {
            let Some(conv) = self.store.conversations.get(id) else {
                continue;
            };
            if conv.is_stale() {
                continue;
            }
            if let Some(score) = list_overlay::fuzzy_score(&query, &conv.name) {
                scored.push((
                    score,
                    1,
                    PaletteItem::Conversation {
                        id: id.clone(),
                        name: conv.name.clone(),
                        is_group: conv.is_group,
                    },
                ));
            }
        }
        for cmd in crate::input::COMMANDS {
            let haystack = format!("{} {}", cmd.name, cmd.description);
            if let Some(score) = list_overlay::fuzzy_score(&query, &haystack) {
                scored.push((
                    score,
                    0,
                    PaletteItem::Command {
                        name: cmd.name,
                        args: cmd.args,
                        description: cmd.description,
                    },
                ));
            }
        }
        scored.sort_by_key(|item| (std::cmp::Reverse(item.0), std::cmp::Reverse(item.1)));
        self.palette.filtered = scored.into_iter().map(|(_, _, item)| item).collect();
        list_overlay::clamp_index(&mut self.palette.index, self.palette.filtered.len());
    }

    /// Build the list of available group menu actions (context-dependent).
    pub fn group_menu_items(&self) -> Vec<GroupMenuItem> {
        let is_group = self
            .active_conversation
            .as_ref()
            .and_then(|id| self.store.conversations.get(id))
            .is_some_and(|c| c.is_group);
        if is_group {
            vec![
                GroupMenuItem {
                    label: "Members",
                    key_hint: GroupMenuHint::Members,
                    nerd_icon: "\u{f0849}",
                },
                GroupMenuItem {
                    label: "Add member",
                    key_hint: GroupMenuHint::AddMember,
                    nerd_icon: "\u{f0234}",
                },
                GroupMenuItem {
                    label: "Remove member",
                    key_hint: GroupMenuHint::RemoveMember,
                    nerd_icon: "\u{f0235}",
                },
                GroupMenuItem {
                    label: "Rename",
                    key_hint: GroupMenuHint::Rename,
                    nerd_icon: "\u{f03eb}",
                },
                GroupMenuItem {
                    label: "Leave",
                    key_hint: GroupMenuHint::Leave,
                    nerd_icon: "\u{f0a79}",
                },
            ]
        } else {
            vec![GroupMenuItem {
                label: "Create group",
                key_hint: GroupMenuHint::Create,
                nerd_icon: "\u{f0234}",
            }]
        }
    }

    /// Build filtered contacts list for the "Add member" picker (excludes existing group members).
    pub fn refresh_group_add_filter(&mut self) {
        let existing_members: HashSet<&str> = self
            .active_conversation
            .as_ref()
            .and_then(|id| self.store.groups.get(id))
            .map(|g| g.members.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();
        let pairs: Vec<(String, String)> = self
            .store
            .contact_names
            .iter()
            .filter(|(_, name)| !name.is_empty())
            .filter(|(number, _)| !existing_members.contains(number.as_str()))
            .map(|(number, name)| (number.clone(), name.clone()))
            .collect();
        self.group_menu.filtered =
            list_overlay::filter_name_number_pairs(pairs, &self.group_menu.filter);
        list_overlay::clamp_index(&mut self.group_menu.index, self.group_menu.filtered.len());
    }

    /// Build filtered member list for the "Remove member" picker (excludes self).
    pub fn refresh_group_remove_filter(&mut self) {
        let members: Vec<String> = self
            .active_conversation
            .as_ref()
            .and_then(|id| self.store.groups.get(id))
            .map(|g| g.members.clone())
            .unwrap_or_default();
        let pairs: Vec<(String, String)> = members
            .into_iter()
            .filter(|phone| *phone != self.account)
            .map(|phone| {
                let name = self
                    .store
                    .contact_names
                    .get(&phone)
                    .cloned()
                    .unwrap_or_else(|| phone.clone());
                (phone, name)
            })
            .collect();
        self.group_menu.filtered =
            list_overlay::filter_name_number_pairs(pairs, &self.group_menu.filter);
        list_overlay::clamp_index(&mut self.group_menu.index, self.group_menu.filtered.len());
    }

    /// Handle a key press while the group management menu is open.
    pub fn handle_group_menu_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        crate::handlers::keys::handle_group_menu_key(self, code)
    }

    /// Transition from the top-level group menu to a sub-state.
    pub(crate) fn transition_group_menu(&mut self, hint: GroupMenuHint) {
        self.group_menu.index = 0;
        self.group_menu.filter.clear();
        self.group_menu.input.clear();
        match hint {
            GroupMenuHint::Members => {
                // Populate member list for display
                let members: Vec<(String, String)> = self
                    .active_conversation
                    .as_ref()
                    .and_then(|id| self.store.groups.get(id))
                    .map(|g| {
                        g.members
                            .iter()
                            .map(|phone| {
                                let name = self
                                    .store
                                    .contact_names
                                    .get(phone)
                                    .cloned()
                                    .unwrap_or_else(|| phone.clone());
                                (phone.clone(), name)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.group_menu.filtered = members;
                self.open_overlay(OverlayKind::GroupMenu);
                self.group_menu.state = Some(GroupMenuState::Members);
            }
            GroupMenuHint::AddMember => {
                self.refresh_group_add_filter();
                self.open_overlay(OverlayKind::GroupMenu);
                self.group_menu.state = Some(GroupMenuState::AddMember);
            }
            GroupMenuHint::RemoveMember => {
                self.refresh_group_remove_filter();
                self.open_overlay(OverlayKind::GroupMenu);
                self.group_menu.state = Some(GroupMenuState::RemoveMember);
            }
            GroupMenuHint::Rename => {
                // Pre-fill with current group name
                let name = self
                    .active_conversation
                    .as_ref()
                    .and_then(|id| self.store.conversations.get(id))
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                self.group_menu.input = name;
                self.open_overlay(OverlayKind::GroupMenu);
                self.group_menu.state = Some(GroupMenuState::Rename);
            }
            GroupMenuHint::Leave => {
                self.open_overlay(OverlayKind::GroupMenu);
                self.group_menu.state = Some(GroupMenuState::LeaveConfirm);
            }
            GroupMenuHint::Create => {
                self.open_overlay(OverlayKind::GroupMenu);
                self.group_menu.state = Some(GroupMenuState::Create);
            }
        }
    }

    /// Handle a key press while the message-request overlay is open.
    pub(crate) fn handle_message_request_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        crate::handlers::keys::handle_message_request_key(self, code)
    }

    pub(crate) fn handle_reaction_picker_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        crate::handlers::keys::handle_reaction_picker_key(self, code)
    }

    /// Build a SendRequest::Reaction from the current picker selection and focused message.
    /// If the user already reacted with the same emoji, removes it instead (toggle behavior).
    pub(crate) fn prepare_reaction_send(&mut self) -> Option<SendRequest> {
        let emoji = QUICK_REACTIONS
            .get(self.reactions.picker_index)?
            .to_string();
        self.prepare_reaction_send_emoji(&emoji)
    }

    /// Build a SendRequest::Reaction for an arbitrary emoji string.
    /// If the user already reacted with the same emoji, removes it instead (toggle behavior).
    pub(crate) fn prepare_reaction_send_emoji(&mut self, emoji: &str) -> Option<SendRequest> {
        let conv_id = self.active_conversation.clone()?;
        let conv = self.store.conversations.get(&conv_id)?;
        let is_group = conv.is_group;

        let index = self
            .scroll
            .focused_index
            .unwrap_or_else(|| conv.messages.len().saturating_sub(1));
        let msg = conv.messages.get(index)?;

        let target_timestamp = msg.timestamp_ms;
        let target_author = if msg.is_outgoing() {
            self.account.clone()
        } else {
            // Reverse lookup: find the phone number for this display name
            self.store
                .contact_names
                .iter()
                .find(|(_, name)| name.as_str() == msg.sender)
                .map(|(num, _)| num.clone())
                .unwrap_or_else(|| msg.sender.clone())
        };

        // Check if user already reacted with the same emoji (toggle → remove)
        let is_remove = msg
            .reactions
            .iter()
            .any(|r| r.is_from_me() && r.emoji == emoji);

        // Optimistic local update
        if let Some(conv) = self.store.conversations.get_mut(&conv_id)
            && let Some(msg) = conv.messages.get_mut(index)
        {
            if is_remove {
                msg.reactions
                    .retain(|r| !(r.is_from_me() && r.emoji == emoji));
            } else {
                // One reaction per user — replace or push
                if let Some(existing) = msg.reactions.iter_mut().find(|r| r.is_from_me()) {
                    existing.emoji = emoji.to_string();
                } else {
                    msg.reactions.push(Reaction {
                        emoji: emoji.to_string(),
                        sender: "you".to_string(),
                    });
                }
            }
        }

        // Persist to DB
        if is_remove {
            self.db_warn_visible(
                self.db
                    .remove_reaction(&conv_id, target_timestamp, &target_author, "you"),
                "remove_reaction",
            );
        } else {
            self.db_warn_visible(
                self.db
                    .upsert_reaction(&conv_id, target_timestamp, &target_author, "you", emoji),
                "upsert_reaction",
            );
        }

        Some(SendRequest::Reaction {
            conv_id,
            emoji: emoji.to_string(),
            is_group,
            target_author,
            target_timestamp,
            remove: is_remove,
        })
    }

    /// Build the list of available actions for the focused message.
    pub fn action_menu_items(&self) -> Vec<ActionMenuItem> {
        let msg = match self.selected_message() {
            Some(m) => m,
            None => return Vec::new(),
        };
        let mut items = Vec::new();
        if !msg.is_system && !msg.is_deleted {
            items.push(ActionMenuItem {
                label: "Reply",
                key_hint: ActionMenuHint::Reply,
                nerd_icon: "\u{f045a}",
            });
        }
        if msg.is_outgoing() && !msg.is_system && !msg.is_deleted {
            items.push(ActionMenuItem {
                label: "Edit",
                key_hint: ActionMenuHint::Edit,
                nerd_icon: "\u{f03eb}",
            });
        }
        if !msg.is_system {
            items.push(ActionMenuItem {
                label: "React",
                key_hint: ActionMenuHint::React,
                nerd_icon: "\u{f0785}",
            });
        }
        if !msg.is_system && !msg.is_deleted {
            items.push(ActionMenuItem {
                label: "Forward",
                key_hint: ActionMenuHint::Forward,
                nerd_icon: "\u{f04d6}",
            });
        }
        items.push(ActionMenuItem {
            label: "Copy",
            key_hint: ActionMenuHint::Copy,
            nerd_icon: "\u{f018f}",
        });
        if !msg.is_system && !msg.is_deleted {
            items.push(ActionMenuItem {
                label: "Delete",
                key_hint: ActionMenuHint::Delete,
                nerd_icon: "\u{f0a79}",
            });
        }
        if !msg.is_system && !msg.is_deleted {
            items.push(ActionMenuItem {
                label: if msg.is_pinned { "Unpin" } else { "Pin" },
                key_hint: ActionMenuHint::PinToggle,
                nerd_icon: "\u{f0403}",
            });
        }
        if let Some(ref poll) = msg.poll_data {
            if !poll.closed {
                items.push(ActionMenuItem {
                    label: "Vote",
                    key_hint: ActionMenuHint::Vote,
                    nerd_icon: "\u{f0e73}",
                });
            }
            if msg.is_outgoing() && !poll.closed {
                items.push(ActionMenuItem {
                    label: "End Poll",
                    key_hint: ActionMenuHint::EndPoll,
                    nerd_icon: "\u{f073a}",
                });
            }
        }
        if !msg.is_deleted {
            if extract_file_uri(&msg.body).is_some() {
                items.push(ActionMenuItem {
                    label: "Open attachment",
                    key_hint: ActionMenuHint::OpenAttachment,
                    nerd_icon: "\u{f15b5}",
                });
            }
            if extract_http_url(&msg.body).is_some() {
                items.push(ActionMenuItem {
                    label: "Open link",
                    key_hint: ActionMenuHint::OpenLink,
                    nerd_icon: "\u{f0337}",
                });
            }
        }
        items
    }

    /// Handle a key press while the action menu overlay is open.
    pub fn handle_action_menu_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        crate::handlers::keys::handle_action_menu_key(self, code)
    }

    /// Execute an action by its hint. Reuses the same logic as the direct
    /// Normal-mode keybinds. Exhaustive match: adding a variant to
    /// [`ActionMenuHint`] is a compiler error here until handled.
    pub(crate) fn execute_action_by_hint(&mut self, hint: ActionMenuHint) -> Option<SendRequest> {
        use crate::handlers::keys;
        match hint {
            ActionMenuHint::Reply => keys::execute_reply(self),
            ActionMenuHint::Edit => keys::execute_edit(self),
            ActionMenuHint::React => keys::execute_react(self),
            ActionMenuHint::Forward => keys::execute_forward(self),
            ActionMenuHint::Copy => {
                self.copy_selected_message(false);
                None
            }
            ActionMenuHint::Delete => keys::execute_delete_confirm(self),
            ActionMenuHint::PinToggle => keys::execute_pin_toggle(self),
            ActionMenuHint::Vote => {
                // Vote on poll
                if let Some(msg) = self.selected_message()
                    && let Some(ref poll) = msg.poll_data
                    && !poll.closed
                {
                    let conv_id = self.active_conversation.clone().unwrap_or_default();
                    let is_group = self
                        .store
                        .conversations
                        .get(&conv_id)
                        .map(|c| c.is_group)
                        .unwrap_or(false);
                    let poll_author = msg.route_author(&self.account).to_string();
                    let options = poll.options.clone();
                    let allow_multiple = poll.allow_multiple;
                    let poll_timestamp = msg.timestamp_ms;
                    let option_count = options.len();
                    self.poll_vote.pending = Some(PollVotePending {
                        conv_id,
                        is_group,
                        poll_author,
                        poll_timestamp,
                        allow_multiple,
                        options,
                    });
                    self.poll_vote.selections = vec![false; option_count];
                    self.poll_vote.index = 0;
                    self.open_overlay(OverlayKind::PollVote);
                }
                None
            }
            ActionMenuHint::EndPoll => {
                // End poll
                if let Some(msg) = self.selected_message()
                    && msg.is_outgoing()
                    && msg.poll_data.as_ref().is_some_and(|p| !p.closed)
                {
                    let conv_id = self.active_conversation.clone()?;
                    let is_group = self
                        .store
                        .conversations
                        .get(&conv_id)
                        .map(|c| c.is_group)
                        .unwrap_or(false);
                    let poll_timestamp = msg.timestamp_ms;
                    // Optimistic close
                    if let Some(conv) = self.store.conversations.get_mut(&conv_id)
                        && let Some(idx) = conv.find_msg_idx(poll_timestamp)
                        && let Some(ref mut poll) = conv.messages[idx].poll_data
                    {
                        poll.closed = true;
                    }
                    self.db_warn_visible(
                        self.db.close_poll(&conv_id, poll_timestamp),
                        "close_poll",
                    );
                    return Some(SendRequest::PollTerminate {
                        recipient: conv_id,
                        is_group,
                        poll_timestamp,
                    });
                }
                None
            }
            ActionMenuHint::OpenAttachment => {
                // Open attachment
                if let Some(msg) = self.selected_message()
                    && let Some(uri) = extract_file_uri(&msg.body)
                {
                    self.open_file(&uri);
                }
                None
            }
            ActionMenuHint::OpenLink => {
                // Open link
                if let Some(msg) = self.selected_message()
                    && let Some(url) = extract_http_url(&msg.body)
                {
                    self.open_url(&url);
                }
                None
            }
        }
    }

    /// Handle a key press while the safety-number verify overlay is open.
    pub fn handle_verify_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        crate::handlers::keys::handle_verify_key(self, code)
    }

    pub(crate) fn open_forward_picker(&mut self) {
        self.open_overlay(OverlayKind::Forward);
        self.forward.index = 0;
        self.forward.filter.clear();
        self.update_forward_filter();
    }

    pub(crate) fn update_forward_filter(&mut self) {
        let filter = self.forward.filter.to_lowercase();
        self.forward.filtered = self
            .store
            .conversation_order
            .iter()
            .filter_map(|id| {
                let conv = self.store.conversations.get(id)?;
                if !conv.accepted {
                    return None;
                }
                // Exclude the current conversation
                if self.active_conversation.as_deref() == Some(id.as_str()) {
                    return None;
                }
                let name = &conv.name;
                if filter.is_empty() || name.to_lowercase().contains(&filter) {
                    Some((id.clone(), name.clone()))
                } else {
                    None
                }
            })
            .collect();
        list_overlay::clamp_index(&mut self.forward.index, self.forward.filtered.len());
    }

    pub fn handle_forward_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        crate::handlers::keys::handle_forward_key(self, code)
    }

    pub fn handle_contacts_key(&mut self, code: KeyCode) {
        crate::handlers::keys::handle_contacts_key(self, code)
    }

    /// Handle a key press while the search overlay is open.
    pub fn handle_search_key(&mut self, code: KeyCode) {
        crate::handlers::keys::handle_search_key(self, code)
    }

    /// Jump to a message by its timestamp_ms in the active conversation.
    /// Sets scroll.offset so the message is visible, and scroll.focused_index.
    fn jump_to_message_timestamp(&mut self, target_ts: i64) {
        let conv_id = match self.active_conversation.as_ref() {
            Some(id) => id.clone(),
            None => return,
        };
        let conv = match self.store.conversations.get(&conv_id) {
            Some(c) => c,
            None => return,
        };
        let total = conv.messages.len();
        if total == 0 {
            return;
        }

        // Find the message index matching this timestamp
        let idx = conv.find_msg_idx(target_ts);
        if let Some(i) = idx {
            // Set scroll.offset so the message is visible (roughly centered)
            let from_bottom = total.saturating_sub(i + 1);
            self.scroll.offset = from_bottom;
            self.scroll.focused_index = Some(i);
            self.mode = InputMode::Normal;
        }
    }

    /// Jump to the original message quoted by the currently focused message.
    pub(crate) fn jump_to_quote(&mut self) {
        let msg = match self.selected_message() {
            Some(m) => m,
            None => return,
        };
        let quote_ts = match &msg.quote {
            Some(q) => q.timestamp_ms,
            None => {
                self.status_message = "No quote on this message".to_string();
                return;
            }
        };

        // Save current position for jump-back
        self.scroll
            .jump_stack
            .push((self.scroll.offset, self.scroll.focused_index));

        // Try to find the quoted message
        let conv_id = match self.active_conversation.as_ref() {
            Some(id) => id.clone(),
            None => return,
        };
        let found = self
            .store
            .conversations
            .get(&conv_id)
            .and_then(|c| c.find_msg_idx(quote_ts))
            .is_some();

        if found {
            self.jump_to_message_timestamp(quote_ts);
        } else {
            // Pop the saved position since we didn't actually jump
            self.scroll.jump_stack.pop();
            self.status_message = "Quoted message not in loaded history".to_string();
        }
    }

    /// Jump back to the position before the last quote jump.
    pub(crate) fn jump_back(&mut self) {
        if let Some((offset, index)) = self.scroll.jump_stack.pop() {
            self.scroll.offset = offset;
            self.scroll.focused_index = index;
        }
    }

    /// Jump to the next/previous search result in the active conversation.
    pub(crate) fn jump_to_search_result(&mut self, forward: bool) {
        let active = self.active_conversation.as_deref();
        let action = self.search.jump_to_result(forward, active);
        self.dispatch_search_action(action);
    }

    /// Dispatch a `SearchAction` returned by `SearchState` methods.
    pub(crate) fn dispatch_search_action(&mut self, action: SearchAction) {
        match action {
            SearchAction::Select {
                conv_id,
                timestamp_ms,
                status,
            } => {
                self.close_overlay();
                self.join_conversation(&conv_id);
                self.jump_to_message_timestamp(timestamp_ms);
                if let Some(msg) = status {
                    self.status_message = msg;
                }
            }
            SearchAction::Status(msg) => {
                self.status_message = msg;
            }
            SearchAction::Cancel => {
                self.close_overlay();
            }
            SearchAction::None => {}
        }
    }

    /// Open the file browser overlay (validates active conversation first).
    pub fn open_file_browser(&mut self) {
        if self.active_conversation.is_none() {
            self.status_message = "No active conversation. Use /join <name> first.".to_string();
            return;
        }
        self.file_picker.open();
        self.open_overlay(OverlayKind::FilePicker);
    }

    /// Handle a key press while the file browser overlay is open.
    pub fn handle_file_browser_key(&mut self, code: KeyCode) {
        crate::handlers::keys::handle_file_browser_key(self, code)
    }

    /// Handle a key press while the autocomplete popup is visible.
    /// Returns `Some(SendRequest)` when the user submits a command
    /// that requires sending a message. Returns `None` otherwise.
    pub fn handle_autocomplete_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        crate::handlers::keys::handle_autocomplete_key(self, code)
    }

    pub fn new(account: String, db: Database, config_path: &std::path::Path) -> Self {
        let (image_render_tx, image_render_rx) = mpsc::channel();
        Self {
            store: ConversationStore::new(),
            active_conversation: None,
            input: InputState::default(),
            sidebar_visible: true,
            scroll: ScrollState::default(),
            status_message: "connecting...".to_string(),
            should_quit: false,
            triggers: crate::trigger::TriggerEngine::default(),
            account,
            sidebar_width: 22,
            sidebar_on_right: false,
            sidebar_filter: SidebarFilterState::default(),
            palette: PaletteState::default(),
            typing: TypingState::default(),
            connected: false,
            loading: true,
            startup_status: "Starting signal-cli...".to_string(),
            spinner_tick: 0,
            mode: InputMode::Insert,
            db,
            connection_error: None,
            notifications: NotificationState::new(),
            muted_conversations: HashMap::new(),
            blocked_conversations: HashSet::new(),
            autocomplete: AutocompleteState::new(),
            settings_overlay: SettingsOverlayState {
                mouse_snapshot: true,
                ..Default::default()
            },
            contacts_overlay: ContactsOverlayState::default(),
            verify: VerifyOverlayState::default(),
            identity_trust: HashMap::new(),
            image: ImageState::new(image_render_tx, image_render_rx),
            prev_active_conversation: None,
            incognito: false,
            media: MediaState::default(),
            date_separators: true,
            show_receipts: true,
            color_receipts: true,
            nerd_fonts: false,
            pending: PendingState::default(),
            pending_normal_key: None,
            reactions: ReactionState::new(),
            emoji_picker: EmojiPickerState::default(),
            is_demo: false,
            file_picker: FilePickerState::default(),
            pending_attachment: None,
            pending_paste_cleanups: HashMap::new(),
            paste_temp_path: {
                static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let dir = std::env::temp_dir().join(format!(
                    "siggy-paste-{}-{}",
                    std::process::id(),
                    unique
                ));
                // Best-effort: clean any stale files from a previous run with the same PID,
                // then recreate. Errors here are non-fatal; handle_clipboard_image re-checks.
                let _ = std::fs::remove_dir_all(&dir);
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    crate::debug_log::logf(format_args!("paste temp dir init failed: {e}"));
                }
                dir
            },
            reply_target: None,
            editing_message: None,
            search: SearchState::default(),
            send_read_receipts: true,
            action_menu: ActionMenuState::default(),
            forward: ForwardOverlayState::default(),
            group_menu: GroupMenuOverlayState::default(),
            mouse: MouseState {
                enabled: true,
                ..MouseState::default()
            },
            theme: theme::default_theme(),
            theme_picker: ThemePickerState {
                available_themes: theme::all_themes(),
                ..Default::default()
            },
            keybindings: keybindings::default_profile(),
            keybindings_overlay: KeybindingsOverlayState {
                available_profiles: keybindings::all_profile_names(),
                ..Default::default()
            },
            pin_duration: PinDurationOverlayState::default(),
            poll_vote: PollVoteOverlayState::default(),
            expiring_msg_count: 0,
            profile: ProfileOverlayState::default(),
            settings_profiles: SettingsProfileOverlayState {
                available: crate::settings_profile::all_settings_profiles(),
                ..Default::default()
            },
            sync: SyncState::new(),
            current_overlay: None,
            lock: LockState {
                hash_path: crate::domain::lock_hash_path(config_path),
                ..Default::default()
            },
        }
    }

    /// Load conversations and messages from the database on startup
    /// Number of messages loaded per page (initial load + pagination batches).
    const PAGE_SIZE: usize = 100;

    pub fn load_from_db(&mut self) -> anyhow::Result<()> {
        let conv_data = self.db.load_conversations(Self::PAGE_SIZE)?;
        let order = self.db.load_conversation_order()?;

        for mut conv in conv_data {
            let id = conv.id.clone();
            let msg_count = conv.messages.len();
            let unread = conv.unread;

            // Demote stale Sending messages to Failed. A row stuck in Sending
            // means no SendTimestamp ever confirmed it: the app exited before
            // the response, the RPC errored without a SendFailed, or the
            // request never reached signal-cli at all. Promoting to Sent here
            // displayed never-delivered messages as sent (#486); Failed is
            // honest and prompts the user to resend.
            for msg in &mut conv.messages {
                if msg.status == Some(MessageStatus::Sending) {
                    msg.status = Some(MessageStatus::Failed);
                }
            }

            // Resolve image paths from stored messages (rendering is deferred to main loop)
            for msg in &mut conv.messages {
                if msg.body.starts_with("[image:") {
                    let path_str = if let Some(uri_pos) = msg.body.find("file:///") {
                        let uri_slice = msg.body[uri_pos..].trim_end_matches(')');
                        Some(file_uri_to_path(uri_slice))
                    } else if let Some(arrow_pos) = msg.body.find(" -> ") {
                        Some(msg.body[arrow_pos + 4..].trim_end_matches(']').to_string())
                    } else {
                        None
                    };
                    if let Some(p) = path_str
                        && Path::new(&p).exists()
                    {
                        msg.image_path = Some(p);
                    }
                }
            }

            // Mark conversations that may have more messages in DB
            if msg_count >= Self::PAGE_SIZE {
                self.store.has_more_messages.insert(id.clone());
            }
            self.store.conversations.insert(id.clone(), conv);
            // Derive last_read_index from unread count
            if msg_count > 0 {
                let read_index = msg_count.saturating_sub(unread);
                self.store.last_read_index.insert(id, read_index);
            }
        }

        self.store.conversation_order = order;
        self.muted_conversations = self.db.load_mutes()?;
        self.blocked_conversations = self.db.load_blocked()?;

        // Fix 1:1 conversations still named as phone numbers: scan message senders
        // for a real display name (from source_name in previous sessions).
        for conv in self.store.conversations.values_mut() {
            if !conv.is_group && conv.name == conv.id && conv.name.starts_with('+') {
                // Find the most recent non-"you" sender with a real name
                if let Some(name) = conv
                    .messages
                    .iter()
                    .rev()
                    .find(|m| {
                        m.sender != "you" && m.sender != conv.id && !m.sender.starts_with('+')
                    })
                    .map(|m| m.sender.clone())
                {
                    db_warn(
                        self.db.upsert_conversation(&conv.id, &name, false),
                        "upsert_conversation",
                    );
                    conv.name = name;
                }
            }
        }

        Ok(())
    }

    /// How many additional messages the render window grows by per
    /// extend_scrollback call when older messages are already in memory.
    const SCROLLBACK_EXTEND_CHUNK: usize = 50;

    /// Extend the scrollback when the user hits the top of the render
    /// window (#488). Prefers widening the window over already-loaded
    /// messages; only pages from the DB when the window has reached
    /// message 0. Each call makes the window strictly larger, so the
    /// next draw computes a larger base_scroll and at_top stays false
    /// until the user scrolls further up -- the previous behavior
    /// (re-loading every 50ms tick while the viewport never moved)
    /// cannot recur.
    pub fn extend_scrollback(&mut self) {
        self.scroll.at_top = false;
        if self.scroll.can_extend_in_memory {
            self.scroll.window_extra += Self::SCROLLBACK_EXTEND_CHUNK;
            return;
        }
        // Window already reaches the oldest loaded message: page from the DB
        // and widen the window by what arrived so the new messages are
        // actually reachable.
        let before = self
            .active_conversation
            .as_ref()
            .and_then(|id| self.store.conversations.get(id))
            .map(|c| c.messages.len())
            .unwrap_or(0);
        self.load_more_messages();
        let after = self
            .active_conversation
            .as_ref()
            .and_then(|id| self.store.conversations.get(id))
            .map(|c| c.messages.len())
            .unwrap_or(0);
        self.scroll.window_extra += after.saturating_sub(before);
    }

    /// Load older messages for the active conversation when scrolled to the top.
    pub fn load_more_messages(&mut self) {
        self.scroll.at_top = false;
        let conv_id = match self.active_conversation.as_ref() {
            Some(id) if self.store.has_more_messages.contains(id) => id.clone(),
            _ => return,
        };

        let already_loaded = self
            .store
            .conversations
            .get(&conv_id)
            .map(|c| c.messages.len())
            .unwrap_or(0);

        let new_msgs = match self
            .db
            .load_messages_page(&conv_id, Self::PAGE_SIZE, already_loaded)
        {
            Ok(msgs) => msgs,
            Err(_) => return,
        };

        if new_msgs.len() < Self::PAGE_SIZE {
            self.store.has_more_messages.remove(&conv_id);
        }

        if new_msgs.is_empty() {
            return;
        }

        let prepend_count = new_msgs.len();

        // Post-process: demote stale Sending → Failed (see load_from_db, #486),
        // resolve image paths
        let mut processed: Vec<DisplayMessage> = new_msgs
            .into_iter()
            .map(|mut msg| {
                if msg.status == Some(MessageStatus::Sending) {
                    msg.status = Some(MessageStatus::Failed);
                }
                if msg.body.starts_with("[image:") {
                    let path_str = if let Some(uri_pos) = msg.body.find("file:///") {
                        let uri_slice = msg.body[uri_pos..].trim_end_matches(')');
                        Some(file_uri_to_path(uri_slice))
                    } else if let Some(arrow_pos) = msg.body.find(" -> ") {
                        Some(msg.body[arrow_pos + 4..].trim_end_matches(']').to_string())
                    } else {
                        None
                    };
                    if let Some(p) = path_str
                        && Path::new(&p).exists()
                    {
                        msg.image_path = Some(p);
                    }
                }
                msg
            })
            .collect();

        // Prepend to conversation
        if let Some(conv) = self.store.conversations.get_mut(&conv_id) {
            processed.append(&mut conv.messages);
            conv.messages = processed;
        }

        // Shift message indexes that reference this conversation
        if let Some(read_idx) = self.store.last_read_index.get_mut(&conv_id) {
            *read_idx += prepend_count;
        }
        if self.active_conversation.as_ref() == Some(&conv_id)
            && let Some(ref mut fi) = self.scroll.focused_index
        {
            *fi += prepend_count;
        }
    }

    /// Resize sidebar by delta, clamped between 14..=40
    pub fn resize_sidebar(&mut self, delta: i16) {
        let new_width = (self.sidebar_width as i16 + delta).clamp(14, 40) as u16;
        self.sidebar_width = new_width;
        self.save_settings();
    }

    /// Refresh the filtered sidebar list based on the current filter text.
    pub(crate) fn refresh_sidebar_filter(&mut self) {
        let query = self.sidebar_filter.query.to_lowercase();
        self.sidebar_filter.filtered = self
            .store
            .conversation_order
            .iter()
            .filter(|id| {
                self.store
                    .conversations
                    .get(*id)
                    .is_some_and(|c| c.name.to_lowercase().contains(&query))
            })
            .cloned()
            .collect();
    }

    /// Clear sidebar filter state and restore the full list.
    pub(crate) fn clear_sidebar_filter(&mut self) {
        if self.is_overlay(OverlayKind::SidebarFilter) {
            self.close_overlay();
        }
        self.sidebar_filter.query.clear();
        self.sidebar_filter.filtered.clear();
    }

    /// Handle a key press while sidebar filter is active.
    pub(crate) fn handle_sidebar_filter_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => {
                self.clear_sidebar_filter();
            }
            KeyCode::Enter => {
                // Select the first matching conversation
                let target = if self.sidebar_filter.filtered.is_empty() {
                    None
                } else {
                    Some(self.sidebar_filter.filtered[0].clone())
                };
                self.clear_sidebar_filter();
                if let Some(conv_id) = target {
                    self.join_conversation(&conv_id);
                }
            }
            KeyCode::Char(c) => {
                self.sidebar_filter.query.push(c);
                self.refresh_sidebar_filter();
            }
            KeyCode::Backspace => {
                self.sidebar_filter.query.pop();
                if self.sidebar_filter.query.is_empty() {
                    self.clear_sidebar_filter();
                } else {
                    self.refresh_sidebar_filter();
                }
            }
            _ => {}
        }
    }

    /// Mark current conversation as fully read
    pub fn mark_read(&mut self) {
        if let Some(ref conv_id) = self.active_conversation {
            if let Some(conv) = self.store.conversations.get(conv_id) {
                self.store
                    .last_read_index
                    .insert(conv_id.clone(), conv.messages.len());
            }
            // Persist read marker
            let conv_id = conv_id.clone();
            if let Ok(Some(rowid)) = self.db.last_message_rowid(&conv_id) {
                db_warn(
                    self.db.save_read_marker(&conv_id, rowid),
                    "save_read_marker",
                );
            }
        }
    }

    /// End the initial sync burst. Snaps viewport, fires summary notification,
    /// marks the active conversation read, and resets sync state.
    pub fn end_sync(&mut self) {
        self.sync.active = false;
        self.sync.pin = None;

        // Snap viewport to newest messages (unless user manually scrolled)
        if !self.sync.user_scrolled {
            self.scroll.offset = 0;
        }

        // Fire summary notification if any were suppressed
        let total: usize = self.sync.suppressed_notifications.values().sum();
        let conv_count = self.sync.suppressed_notifications.len();
        if total > 0 {
            self.notifications.pending_bell = true;
            if self.notifications.desktop_notifications {
                let conv_word = if conv_count == 1 {
                    "conversation"
                } else {
                    "conversations"
                };
                let body = format!("{total} new messages in {conv_count} {conv_word}");
                show_desktop_notification(
                    "siggy",
                    &body,
                    false,
                    None,
                    crate::domain::NotificationPreview::Full,
                );
            }
        }
        self.sync.suppressed_notifications.clear();

        // Send read receipts for messages that arrived during sync, then mark read
        if let Some(conv_id) = self.active_conversation.clone() {
            let read_from = self
                .store
                .last_read_index
                .get(&conv_id)
                .copied()
                .unwrap_or(0);
            self.queue_read_receipts_for_conv(&conv_id, read_from);
        }
        self.mark_read();

        // Update status
        self.status_message = if self.connected {
            "connected".to_string()
        } else {
            "disconnected".to_string()
        };
    }

    /// Queue read receipts for unread incoming messages in a conversation.
    /// Messages from `start_index` onward are considered unread.
    /// Groups timestamps by sender and appends to `pending.read_receipts`.
    fn queue_read_receipts_for_conv(&mut self, conv_id: &str, start_index: usize) {
        if !self.send_read_receipts {
            return;
        }
        let conv = match self.store.conversations.get(conv_id) {
            Some(c) => c,
            None => return,
        };
        if !conv.accepted {
            return;
        }
        if self.blocked_conversations.contains(conv_id) {
            return;
        }
        // Collect timestamps grouped by sender phone number
        let mut by_sender: HashMap<String, Vec<i64>> = HashMap::new();
        for msg in conv.messages.iter().skip(start_index) {
            // Only incoming messages: status is None, not system, has a real sender_id
            if msg.status.is_some() || msg.is_system || msg.sender_id.is_empty() {
                continue;
            }
            // Skip messages from ourselves (shouldn't happen for incoming, but guard)
            if msg.sender_id == self.account {
                continue;
            }
            by_sender
                .entry(msg.sender_id.clone())
                .or_default()
                .push(msg.timestamp_ms);
        }
        for (recipient, timestamps) in by_sender {
            if !timestamps.is_empty() {
                self.pending.read_receipts.push((recipient, timestamps));
            }
        }
    }

    /// Queue a read receipt for a single incoming message.
    ///
    /// Intended for use when a message arrives while its conversation is active and
    /// the user is therefore "reading" it immediately.
    ///
    /// **Caller contract:** `sender_id` must be the *remote peer* who sent the message
    /// (a phone or UUID), never `self.account`. The fn drops empty sender_ids and
    /// self-sent receipts as a safety net, but callers should not rely on those
    /// guards as a way to send "ambiguous" receipts — pre-resolve identity instead.
    /// The fn is also a no-op when `send_read_receipts` is disabled in settings.
    pub(crate) fn queue_single_read_receipt(&mut self, sender_id: &str, timestamp_ms: i64) {
        if !self.send_read_receipts {
            return;
        }
        if sender_id.is_empty() || sender_id == self.account {
            return;
        }
        self.pending
            .read_receipts
            .push((sender_id.to_string(), vec![timestamp_ms]));
    }

    /// Build a Typing SendRequest for the active conversation, or None if no conversation is active.
    pub(crate) fn build_typing_request(&self, stop: bool) -> Option<SendRequest> {
        let conv_id = self.active_conversation.as_ref()?;
        let is_group = self
            .store
            .conversations
            .get(conv_id)
            .map(|c| c.is_group)
            .unwrap_or(false);
        Some(SendRequest::Typing {
            recipient: conv_id.clone(),
            is_group,
            stop,
        })
    }

    /// Check if the typing indicator has timed out (5 seconds since last keypress).
    /// Returns a typing-stop SendRequest if so, and resets state.
    pub fn check_typing_timeout(&mut self) -> Option<SendRequest> {
        if self.typing.check_timeout() {
            self.build_typing_request(true)
        } else {
            None
        }
    }

    /// Clear terminal image placement state so images are retransmitted on the next frame.
    /// The expensive caches (native_image_cache, iterm2_crop_cache,
    /// sixel_cache) are preserved so switching back to a conversation doesn't
    /// re-decode images from disk. Call on conversation switch.
    pub fn clear_kitty_placements(&mut self) {
        self.image.kitty_transmitted.clear();
        self.image.kitty_pending_transmits.clear();
    }

    /// Full image state reset: clear both terminal placements and base64 caches.
    /// Call on terminal resize (cell dimensions change, so cached PNGs need re-encoding).
    ///
    /// Sixel caches survive resize: encoding depends on cell_px and configured
    /// preview dimensions, not the terminal's current cell grid. Only screen
    /// positions change, which ratatui recomputes automatically.
    pub fn clear_kitty_state(&mut self) {
        self.clear_kitty_placements();
        if self.image.image_protocol != ImageProtocol::Sixel {
            self.image.native_image_cache.clear();
            self.image.iterm2_crop_cache.clear();
        }
    }

    /// Reset typing state and queue a stop request if we were typing.
    /// Call this before switching conversations.
    pub(crate) fn reset_typing_with_stop(&mut self) {
        if self.typing.reset() {
            self.pending.typing_stop = self.build_typing_request(true);
        }
    }

    /// Trigger the lock screen. If no passphrase hash exists yet, transitions
    /// to `SetPassphrase` so the user can create one. Otherwise transitions
    /// to `LockEntry`. Clears the input buffer and any pending error.
    pub fn lock_now(&mut self) {
        let has_hash = crate::domain::load_hash(&self.lock.hash_path)
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())
            .is_some();
        self.lock.phase = if has_hash {
            crate::domain::LockPhase::LockEntry
        } else {
            crate::domain::LockPhase::SetPassphrase
        };
        self.lock.input_buffer.clear();
        self.lock.error = None;
        self.lock.old_passphrase_verified = false;
    }

    /// Process a keypress while the lock screen is showing. Drives the
    /// LockPhase state machine: typing fills the buffer, Enter submits,
    /// Backspace deletes, Esc clears the buffer (but does NOT unlock).
    /// Returns true if the key was consumed (always true when locked).
    pub fn handle_lock_key(&mut self, code: crossterm::event::KeyCode) -> bool {
        crate::handlers::keys::handle_lock_key(self, code)
    }

    /// Resolve an Enter press on the lock screen by advancing the phase
    /// based on what's currently being asked for. The state machine:
    ///
    /// - SetPassphrase: hash the entered string, save, transition to Unlocked.
    /// - LockEntry: verify against stored hash; on success transition to
    ///   Unlocked, on failure stay and set the error.
    /// - ChangePassphraseOld: verify; on success transition to
    ///   ChangePassphraseNew and set old_passphrase_verified, on failure
    ///   stay and set the error.
    /// - ChangePassphraseNew: hash + save the new passphrase, transition
    ///   to Unlocked, clear old_passphrase_verified.
    pub(crate) fn submit_lock_input(&mut self) {
        use crate::domain::{LockPhase, hash_passphrase, load_hash, save_hash, verify_passphrase};
        let entered = std::mem::take(&mut self.lock.input_buffer);
        match self.lock.phase {
            LockPhase::Unlocked => {} // defensive; should not happen
            LockPhase::SetPassphrase => {
                if entered.is_empty() {
                    self.lock.error = Some("Passphrase cannot be empty".to_string());
                    return;
                }
                match hash_passphrase(&entered) {
                    Ok(h) => {
                        if let Err(e) = save_hash(&self.lock.hash_path, &h) {
                            self.lock.error = Some(format!("Could not save hash: {e}"));
                            return;
                        }
                        self.lock.phase = LockPhase::Unlocked;
                        self.lock.error = None;
                        self.status_message = "Lock passphrase set".to_string();
                    }
                    Err(e) => {
                        self.lock.error = Some(format!("Hash failed: {e}"));
                    }
                }
            }
            LockPhase::LockEntry => {
                let stored = load_hash(&self.lock.hash_path)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                if verify_passphrase(&entered, &stored) {
                    self.lock.phase = LockPhase::Unlocked;
                    self.lock.error = None;
                } else {
                    self.lock.error = Some("Incorrect passphrase".to_string());
                }
            }
            LockPhase::ChangePassphraseOld => {
                let stored = load_hash(&self.lock.hash_path)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                if verify_passphrase(&entered, &stored) {
                    self.lock.old_passphrase_verified = true;
                    self.lock.phase = LockPhase::ChangePassphraseNew;
                    self.lock.error = None;
                } else {
                    self.lock.error = Some("Incorrect passphrase".to_string());
                }
            }
            LockPhase::ChangePassphraseNew => {
                if entered.is_empty() {
                    self.lock.error = Some("Passphrase cannot be empty".to_string());
                    return;
                }
                match hash_passphrase(&entered) {
                    Ok(h) => {
                        if let Err(e) = save_hash(&self.lock.hash_path, &h) {
                            self.lock.error = Some(format!("Could not save hash: {e}"));
                            return;
                        }
                        self.lock.phase = LockPhase::Unlocked;
                        self.lock.old_passphrase_verified = false;
                        self.status_message = "Lock passphrase changed".to_string();
                    }
                    Err(e) => {
                        self.lock.error = Some(format!("Hash failed: {e}"));
                    }
                }
            }
        }
    }

    /// Handle global keys that work in both Normal and Insert mode.
    /// Returns true if the key was consumed.
    pub fn handle_global_key(&mut self, modifiers: KeyModifiers, code: KeyCode) -> bool {
        crate::handlers::keys::handle_global_key(self, modifiers, code)
    }

    /// Handle overlay keys (help, contacts, settings, autocomplete).
    /// Returns `Some((recipient, body, is_group, local_ts_ms))` if an autocomplete
    /// command triggers a message send. Returns `None` otherwise.
    /// Returns `Ok(true)` if the key was consumed by an overlay.
    /// Returns the currently-active overlay, if any.
    ///
    /// All 23 overlays are now backed by `current_overlay`, so this is just
    /// a thin accessor. Both `has_overlay` and `handle_overlay_key` defer
    /// to it so dispatch and visibility stay in sync automatically.
    pub fn active_overlay(&self) -> Option<OverlayKind> {
        self.current_overlay
    }

    /// Open an overlay.
    ///
    /// Clobbers whichever overlay was previously active. Use `try_open_overlay`
    /// if you need to defer to an existing higher-priority overlay (e.g.
    /// auto-open paths called from `update_status` or `update_autocomplete`).
    pub fn open_overlay(&mut self, kind: OverlayKind) {
        self.current_overlay = Some(kind);
    }

    /// Clear `current_overlay`.
    pub fn close_overlay(&mut self) {
        self.current_overlay = None;
    }

    /// Open `kind` only if no other App-owned overlay is currently active
    /// (or if `kind` is already the active overlay). Use this for auto-open
    /// paths that must not clobber a higher-priority overlay - e.g.
    /// `update_autocomplete` firing on every keystroke, or `update_status`
    /// auto-opening MessageRequest on conversation switches.
    pub fn try_open_overlay(&mut self, kind: OverlayKind) {
        if self.current_overlay.is_none() || self.current_overlay == Some(kind) {
            self.open_overlay(kind);
        }
    }

    /// Returns true if the given overlay kind is currently active.
    pub fn is_overlay(&self, kind: OverlayKind) -> bool {
        self.current_overlay == Some(kind)
    }

    pub fn handle_overlay_key(&mut self, code: KeyCode) -> (bool, Option<SendRequest>) {
        crate::handlers::keys::handle_overlay_key(self, code)
    }

    /// Handle Normal mode key. Dispatches to scroll, edit, or action sub-handlers.
    pub fn handle_normal_key(
        &mut self,
        modifiers: KeyModifiers,
        code: KeyCode,
    ) -> Option<SendRequest> {
        crate::handlers::keys::handle_normal_key(self, modifiers, code)
    }

    /// Handle Insert mode key.
    /// Returns `Some(SendRequest)` if a message send or typing indicator should be dispatched.
    pub fn handle_insert_key(
        &mut self,
        modifiers: KeyModifiers,
        code: KeyCode,
    ) -> Option<SendRequest> {
        crate::handlers::keys::handle_insert_key(self, modifiers, code)
    }

    /// Handle an event from signal-cli
    pub fn handle_signal_event(&mut self, event: SignalEvent) {
        crate::handlers::signal::handle_signal_event(self, event);
    }

    /// Remove expired disappearing messages from memory and DB.
    /// Returns true if any messages were removed (caller should re-render).
    pub fn sweep_expired_messages(&mut self) -> bool {
        if self.expiring_msg_count == 0 {
            return false;
        }

        let now_ms = Utc::now().timestamp_millis();
        let mut removed_count: usize = 0;
        let expired = |m: &DisplayMessage| {
            m.expires_in_seconds > 0
                && m.expiration_start_ms > 0
                && m.expiration_start_ms + m.expires_in_seconds * 1000 < now_ms
        };

        // Split-borrow the store so the read markers can be adjusted while
        // iterating the conversations mutably.
        let crate::conversation_store::ConversationStore {
            conversations,
            last_read_index,
            ..
        } = &mut self.store;

        for (conv_id, conv) in conversations.iter_mut() {
            // Pre-removal indices of the messages about to expire, ascending.
            let removed_idxs: Vec<usize> = conv
                .messages
                .iter()
                .enumerate()
                .filter(|(_, m)| expired(m))
                .map(|(i, _)| i)
                .collect();
            if removed_idxs.is_empty() {
                continue;
            }
            // How many removals sit strictly below a pre-removal index.
            let removed_below = |idx: usize| removed_idxs.partition_point(|&r| r < idx);

            conv.messages.retain(|m| !expired(m));
            removed_count += removed_idxs.len();

            // last_read_index, scroll.focused_index, and saved positions are
            // positions into conv.messages; removals below them shift every
            // later index down (#483). load_more_messages does the mirror
            // adjustment on prepend.
            if let Some(read_idx) = last_read_index.get_mut(conv_id) {
                *read_idx = read_idx
                    .saturating_sub(removed_below(*read_idx))
                    .min(conv.messages.len());
            }
            if self.active_conversation.as_deref() == Some(conv_id) {
                if let Some(fi) = self.scroll.focused_index {
                    self.scroll.focused_index = if removed_idxs.binary_search(&fi).is_ok() {
                        // The focused message itself expired: drop focus
                        // rather than silently moving it to a neighbor (the
                        // delete-confirm overlay targets this index).
                        None
                    } else {
                        Some(fi - removed_below(fi))
                    };
                }
            } else if let Some(pos) = self.scroll.positions.get_mut(conv_id)
                && let Some(fi) = pos.1
            {
                pos.1 = if removed_idxs.binary_search(&fi).is_ok() {
                    None
                } else {
                    Some(fi - removed_below(fi))
                };
            }
        }

        self.expiring_msg_count = self.expiring_msg_count.saturating_sub(removed_count);

        // Clean up DB
        let removed = removed_count > 0;
        if let Ok(n) = self.db.delete_expired_messages(now_ms)
            && n > 0
        {
            return true;
        }

        removed
    }

    /// Active mute state for a conversation, or `None` if unmuted or the timed mute has expired.
    pub fn active_mute(&self, conv_id: &str, now: DateTime<Utc>) -> Option<&MuteState> {
        self.muted_conversations
            .get(conv_id)
            .filter(|s| s.is_active(now))
    }

    /// Check whether a conversation is currently muted at the given instant.
    pub fn is_muted_at(&self, conv_id: &str, now: DateTime<Utc>) -> bool {
        self.active_mute(conv_id, now).is_some()
    }

    /// Display name for a conversation, falling back to the id if unknown.
    pub(crate) fn conversation_name<'a>(&'a self, conv_id: &'a str) -> &'a str {
        self.store
            .conversations
            .get(conv_id)
            .map(|c| c.name.as_str())
            .unwrap_or(conv_id)
    }

    /// Apply a mute change to both in-memory state and the database.
    pub(crate) fn apply_mute(&mut self, conv_id: &str, state: Option<MuteState>) {
        match state {
            None => {
                self.muted_conversations.remove(conv_id);
            }
            Some(s) => {
                self.muted_conversations.insert(conv_id.to_string(), s);
            }
        }
        db_warn(self.db.set_mute(conv_id, state), "set_mute");
    }

    /// Remove expired timed mutes from in-memory state and DB.
    pub fn sweep_expired_mutes(&mut self) {
        match self.db.clear_expired_mutes(Utc::now()) {
            Ok(cleared_ids) => {
                if cleared_ids.is_empty() {
                    return;
                }
                for id in &cleared_ids {
                    self.muted_conversations.remove(id);
                }
                let names: Vec<&str> = cleared_ids
                    .iter()
                    .map(|id| self.conversation_name(id))
                    .collect();
                self.status_message = format!("unmuted {}", names.join(", "));
            }
            Err(e) => {
                crate::debug_log::logf(format_args!("db clear_expired_mutes: {e}"));
            }
        }
    }

    /// Handle a key press in the delete confirmation overlay.
    /// Returns Some(SendRequest::RemoteDelete) if remote delete is requested.
    pub fn handle_delete_confirm_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        crate::handlers::keys::handle_delete_confirm_key(self, code)
    }

    /// Handle a key press in the delete-conversation confirmation overlay.
    pub fn handle_delete_conversation_confirm_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        crate::handlers::keys::handle_delete_conversation_confirm_key(self, code)
    }

    /// Remove the active conversation from local state, the database, and any
    /// cached side tables, then clear the composer.
    ///
    /// For unaccepted message requests this also returns a
    /// `MessageRequestResponse { response_type: "delete" }` so signal-cli can
    /// notify the server. For accepted conversations the deletion is purely
    /// local.
    pub(crate) fn delete_active_conversation(&mut self) -> Option<SendRequest> {
        let conv_id = match self.active_conversation.clone() {
            Some(id) => id,
            None => {
                self.status_message = "No active conversation to delete".to_string();
                return None;
            }
        };

        let (name, is_group, accepted) = match self.store.conversations.get(&conv_id) {
            Some(conv) => (conv.name.clone(), conv.is_group, conv.accepted),
            None => {
                self.status_message = "Active conversation no longer exists".to_string();
                self.active_conversation = None;
                return None;
            }
        };

        self.store.conversations.remove(&conv_id);
        self.store.conversation_order.retain(|id| id != &conv_id);
        self.store.last_read_index.remove(&conv_id);
        self.store.has_more_messages.remove(&conv_id);
        self.store.groups.remove(&conv_id);
        self.scroll.positions.remove(&conv_id);
        self.muted_conversations.remove(&conv_id);
        self.blocked_conversations.remove(&conv_id);
        self.db_warn_visible(self.db.delete_conversation(&conv_id), "delete_conversation");

        self.active_conversation = None;
        self.scroll.offset = 0;
        self.scroll.focused_index = None;
        self.pending_attachment = None;
        self.reply_target = None;
        self.editing_message = None;
        self.reset_typing_with_stop();
        self.clear_kitty_placements();
        if self.is_overlay(OverlayKind::SidebarFilter) {
            self.refresh_sidebar_filter();
        }

        self.status_message = format!("Deleted conversation \"{name}\"");

        if !accepted {
            Some(SendRequest::MessageRequestResponse {
                recipient: conv_id,
                is_group,
                response_type: "delete".to_string(),
            })
        } else {
            None
        }
    }

    /// Handle a key press while the pin duration picker overlay is open.
    pub fn handle_pin_duration_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        crate::handlers::keys::handle_pin_duration_key(self, code)
    }

    /// Handle keys in the profile editor overlay.
    pub fn handle_profile_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        crate::handlers::keys::handle_profile_key(self, code)
    }

    /// Handle a key press while the poll vote overlay is open.
    pub fn handle_poll_vote_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        crate::handlers::keys::handle_poll_vote_key(self, code)
    }

    /// Prepare outgoing mentions: replace @Name with U+FFFC and compute UTF-16 offsets.
    /// Returns (wire_body, mentions_for_rpc).
    pub(crate) fn prepare_outgoing_mentions(&self, text: &str) -> (String, Vec<(usize, String)>) {
        if self.autocomplete.pending_mentions.is_empty() {
            return (text.to_string(), Vec::new());
        }

        let mut wire = text.to_string();
        let mut mentions: Vec<(usize, String)> = Vec::new();

        // Process mentions in reverse order of their position in the string
        // to avoid offset invalidation
        let mut found: Vec<(usize, usize, String)> = Vec::new(); // (byte_start, byte_end, uuid)
        for (name, uuid) in &self.autocomplete.pending_mentions {
            let pattern = format!("@{name}");
            if let Some(uuid) = uuid
                && let Some(pos) = wire.find(&pattern)
            {
                found.push((pos, pos + pattern.len(), uuid.clone()));
            }
        }
        found.sort_by_key(|b| std::cmp::Reverse(b.0)); // reverse order

        for (byte_start, byte_end, uuid) in &found {
            // Compute UTF-16 offset before replacement
            let utf16_offset = wire[..*byte_start].encode_utf16().count();
            wire.replace_range(*byte_start..*byte_end, "\u{FFFC}");
            mentions.push((utf16_offset, uuid.clone()));
        }

        // Re-sort mentions by UTF-16 offset ascending for the RPC
        mentions.sort_by_key(|(off, _)| *off);

        (wire, mentions)
    }

    /// Handle a line of user input; returns Some((conv_id, body, is_group, local_ts_ms)) if we need to send a message
    pub fn handle_input(&mut self) -> Option<SendRequest> {
        crate::handlers::input::handle_input(self)
    }

    /// Update autocomplete candidates based on the current input buffer.
    /// Called after every input change in Insert mode.
    pub fn update_autocomplete(&mut self) {
        let buf = &self.input.buffer;

        // Try command autocomplete first: starts with '/' and no space yet
        if buf.starts_with('/') && !buf.contains(' ') {
            let prefix = buf.to_lowercase();
            let mut candidates = Vec::new();
            for (i, cmd) in COMMANDS.iter().enumerate() {
                if cmd.name.starts_with(&prefix)
                    || (!cmd.alias.is_empty() && cmd.alias.starts_with(&prefix))
                {
                    candidates.push(i);
                }
            }

            if !candidates.is_empty() {
                self.try_open_overlay(OverlayKind::Autocomplete);
                self.autocomplete.mode = AutocompleteMode::Command;
                self.autocomplete.command_candidates = candidates;
                if self.autocomplete.index >= self.autocomplete.command_candidates.len() {
                    self.autocomplete.index = 0;
                }
                return;
            }
        }

        // Try /join autocomplete: starts with "/join " or "/j "
        let join_prefix = if buf.starts_with("/join ") {
            Some("/join ".len())
        } else if buf.starts_with("/j ") {
            Some("/j ".len())
        } else {
            None
        };
        if let Some(prefix_len) = join_prefix {
            let filter_lower = buf[prefix_len..].to_lowercase();
            let mut candidates: Vec<(String, String)> = Vec::new();

            // Collect contacts from contact_names
            for (key, name) in &self.store.contact_names {
                // Non-phone keys are group ids or uuid-keyed username-only
                // contacts; the latter complete as @handle (#612)
                let (display, value) = if key.starts_with('+') {
                    (format!("{name} ({key})"), key.clone())
                } else if let Some(handle) = self.store.usernames.get(key) {
                    (format!("{name} (@{handle})"), format!("@{handle}"))
                } else {
                    continue;
                };
                if filter_lower.is_empty()
                    || name.to_lowercase().contains(&filter_lower)
                    || value.to_lowercase().contains(&filter_lower)
                {
                    candidates.push((display, value));
                }
            }

            // Collect groups
            for group in self.store.groups.values() {
                let display = format!("#{}", group.name);
                if filter_lower.is_empty() || group.name.to_lowercase().contains(&filter_lower) {
                    candidates.push((display, group.id.clone()));
                }
            }

            // Also include existing conversations not yet covered
            for conv_id in &self.store.conversation_order {
                if let Some(conv) = self.store.conversations.get(conv_id) {
                    let already_listed = candidates.iter().any(|(_, val)| val == conv_id);
                    if !already_listed {
                        let display = if conv.is_group {
                            format!("#{}", conv.name)
                        } else {
                            format!("{} ({})", conv.name, conv_id)
                        };
                        if filter_lower.is_empty()
                            || conv.name.to_lowercase().contains(&filter_lower)
                            || conv_id.to_lowercase().contains(&filter_lower)
                        {
                            candidates.push((display, conv_id.clone()));
                        }
                    }
                }
            }

            candidates.sort_by_key(|a| a.0.to_lowercase());

            if !candidates.is_empty() {
                self.try_open_overlay(OverlayKind::Autocomplete);
                self.autocomplete.mode = AutocompleteMode::Join;
                self.autocomplete.join_candidates = candidates;
                if self.autocomplete.index >= self.autocomplete.join_candidates.len() {
                    self.autocomplete.index = 0;
                }
                return;
            }
        }

        // Try @mention autocomplete
        if let Some(ref conv_id) = self.active_conversation
            && let Some(conv) = self.store.conversations.get(conv_id)
            && let Some(trigger_pos) = self.find_mention_trigger()
        {
            let after_at = &self.input.buffer[trigger_pos + 1..self.input.cursor];
            let filter_lower = after_at.to_lowercase();

            let mut candidates: Vec<(String, String, Option<String>)> = Vec::new();
            if conv.is_group {
                // Group: offer all group members
                if let Some(group) = self.store.groups.get(conv_id) {
                    for member_phone in &group.members {
                        let name = self
                            .store
                            .contact_names
                            .get(member_phone)
                            .cloned()
                            .unwrap_or_else(|| member_phone.clone());
                        let uuid = self.store.number_to_uuid.get(member_phone).cloned();
                        if filter_lower.is_empty()
                            || name.to_lowercase().contains(&filter_lower)
                            || member_phone.contains(&filter_lower)
                        {
                            candidates.push((member_phone.clone(), name, uuid));
                        }
                    }
                }
            } else {
                // 1:1 chat: offer the contact as a mention candidate
                let name = self
                    .store
                    .contact_names
                    .get(conv_id)
                    .cloned()
                    .unwrap_or_else(|| conv_id.clone());
                let uuid = self.store.number_to_uuid.get(conv_id).cloned();
                if filter_lower.is_empty()
                    || name.to_lowercase().contains(&filter_lower)
                    || conv_id.contains(&filter_lower)
                {
                    candidates.push((conv_id.clone(), name, uuid));
                }
            }
            candidates.sort_by_key(|a| a.1.to_lowercase());

            if !candidates.is_empty() {
                self.try_open_overlay(OverlayKind::Autocomplete);
                self.autocomplete.mode = AutocompleteMode::Mention;
                self.autocomplete.mention_candidates = candidates;
                self.autocomplete.mention_trigger_pos = trigger_pos;
                if self.autocomplete.index >= self.autocomplete.mention_candidates.len() {
                    self.autocomplete.index = 0;
                }
                return;
            }
        }

        // No autocomplete match
        self.autocomplete.clear();
        if self.is_overlay(OverlayKind::Autocomplete) {
            self.close_overlay();
        }
    }

    /// Find the byte position of the `@` trigger for mention autocomplete.
    /// Returns Some(pos) if `@` is found before cursor, at start or after whitespace,
    /// with no spaces between `@` and cursor.
    fn find_mention_trigger(&self) -> Option<usize> {
        let before_cursor = &self.input.buffer[..self.input.cursor];
        // Find rightmost '@' before cursor
        let at_pos = before_cursor.rfind('@')?;
        // '@' must be at start or preceded by whitespace
        if at_pos > 0 {
            let prev_char = before_cursor[..at_pos].chars().next_back()?;
            if !prev_char.is_whitespace() {
                return None;
            }
        }
        // No spaces between '@' and cursor
        let after_at = &before_cursor[at_pos + 1..];
        if after_at.contains(' ') {
            return None;
        }
        Some(at_pos)
    }

    /// Handle basic cursor/editing keys (Backspace, Delete, Left, Right, Home, End, Char).
    /// Returns true if the key was handled.
    /// Navigate up through input history (older entries).
    pub fn history_up(&mut self) {
        if self.input.history.is_empty() {
            return;
        }
        match self.input.history_index {
            None => {
                self.input.history_draft = self.input.buffer.clone();
                self.input.history_index = Some(self.input.history.len() - 1);
            }
            Some(idx) if idx > 0 => {
                self.input.history_index = Some(idx - 1);
            }
            _ => return,
        }
        self.input.buffer = self.input.history[self.input.history_index.unwrap()].clone();
        self.input.cursor = self.input.buffer.len();
    }

    /// Navigate down through input history (newer entries).
    pub fn history_down(&mut self) {
        let idx = match self.input.history_index {
            Some(idx) => idx,
            None => return,
        };
        if idx < self.input.history.len() - 1 {
            self.input.history_index = Some(idx + 1);
            self.input.buffer = self.input.history[idx + 1].clone();
        } else {
            self.input.buffer = self.input.history_draft.clone();
            self.input.history_index = None;
        }
        self.input.cursor = self.input.buffer.len();
    }

    pub fn apply_input_edit(&mut self, key_code: KeyCode) -> bool {
        match key_code {
            KeyCode::Backspace => {
                if self.input.cursor > 0 {
                    self.input.cursor = prev_char_pos(&self.input.buffer, self.input.cursor);
                    self.input.buffer.remove(self.input.cursor);
                } else if self.pending_attachment.is_some() {
                    self.pending_attachment = None;
                }
                true
            }
            KeyCode::Delete => {
                if self.input.cursor < self.input.buffer.len() {
                    self.input.buffer.remove(self.input.cursor);
                }
                true
            }
            KeyCode::Left => {
                self.input.cursor = prev_char_pos(&self.input.buffer, self.input.cursor);
                true
            }
            KeyCode::Right => {
                self.input.cursor = next_char_pos(&self.input.buffer, self.input.cursor);
                true
            }
            KeyCode::Home => {
                self.input.cursor = self.current_line_start();
                true
            }
            KeyCode::End => {
                self.input.cursor = self.current_line_end();
                true
            }
            KeyCode::Up => {
                let (line, col) = self.cursor_line_col();
                if line > 0 {
                    let lines: Vec<&str> = self.input.buffer.split('\n').collect();
                    let target_line = lines[line - 1];
                    let target_chars = target_line.chars().count();
                    let target_col: usize = target_line
                        .chars()
                        .take(col.min(target_chars))
                        .map(|c| c.len_utf8())
                        .sum();
                    let offset: usize = lines.iter().take(line - 1).map(|l| l.len() + 1).sum();
                    self.input.cursor = offset + target_col;
                } else {
                    self.history_up();
                }
                true
            }
            KeyCode::Down => {
                let (line, col) = self.cursor_line_col();
                let total_lines = self.input_line_count();
                if line < total_lines - 1 {
                    let lines: Vec<&str> = self.input.buffer.split('\n').collect();
                    let target_line = lines[line + 1];
                    let target_chars = target_line.chars().count();
                    let target_col: usize = target_line
                        .chars()
                        .take(col.min(target_chars))
                        .map(|c| c.len_utf8())
                        .sum();
                    let offset: usize = lines.iter().take(line + 1).map(|l| l.len() + 1).sum();
                    self.input.cursor = offset + target_col;
                } else {
                    self.history_down();
                }
                true
            }
            KeyCode::Char(c) => {
                self.input.buffer.insert(self.input.cursor, c);
                self.input.cursor += c.len_utf8();
                true
            }
            _ => false,
        }
    }

    /// Returns the number of lines in the input buffer.
    pub fn input_line_count(&self) -> usize {
        self.input.buffer.matches('\n').count() + 1
    }

    /// Returns (line_index, column) of the cursor within the input buffer.
    /// Column is measured in characters (not bytes) for correct display positioning.
    pub fn cursor_line_col(&self) -> (usize, usize) {
        let before = &self.input.buffer[..self.input.cursor];
        let line = before.matches('\n').count();
        let line_start = match before.rfind('\n') {
            Some(pos) => pos + 1,
            None => 0,
        };
        let col = before[line_start..].chars().count();
        (line, col)
    }

    /// Returns the byte offset of the start of the current line.
    pub(crate) fn current_line_start(&self) -> usize {
        self.input.buffer[..self.input.cursor]
            .rfind('\n')
            .map(|p| p + 1)
            .unwrap_or(0)
    }

    /// Returns the byte offset of the end of the current line (before the newline or buffer end).
    pub(crate) fn current_line_end(&self) -> usize {
        self.input.buffer[self.input.cursor..]
            .find('\n')
            .map(|p| self.input.cursor + p)
            .unwrap_or(self.input.buffer.len())
    }

    /// Delete the word before the cursor (Ctrl+W behavior).
    pub(crate) fn delete_word_back(&mut self) {
        if self.input.cursor == 0 {
            return;
        }
        let buf = &self.input.buffer;
        let mut pos = self.input.cursor;
        // Skip whitespace before cursor
        while pos > 0 {
            let prev = buf[..pos].chars().next_back().unwrap();
            if !prev.is_whitespace() {
                break;
            }
            pos -= prev.len_utf8();
        }
        // Skip word chars
        while pos > 0 {
            let prev = buf[..pos].chars().next_back().unwrap();
            if prev.is_whitespace() {
                break;
            }
            pos -= prev.len_utf8();
        }
        self.input.buffer.drain(pos..self.input.cursor);
        self.input.cursor = pos;
    }

    /// Handle a bracketed paste event (Ctrl+V or terminal paste).
    /// Inserts the entire pasted string at once, avoiding per-character overhead.
    pub fn handle_paste(&mut self, text: String) -> Option<SendRequest> {
        if self.lock.is_locked() {
            return None;
        }
        if self.mode != InputMode::Insert || self.has_overlay() {
            return None;
        }
        // Normalize line endings and insert pasted text at cursor position
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        self.input.buffer.insert_str(self.input.cursor, &text);
        self.input.cursor += text.len();
        // Single autocomplete + typing indicator update
        self.update_autocomplete();
        self.typing.last_keypress = Some(Instant::now());
        if !self.typing.sent
            && !self.input.buffer.is_empty()
            && !self.input.buffer.starts_with('/')
            && self
                .active_conversation
                .as_ref()
                .is_some_and(|id| !self.blocked_conversations.contains(id))
        {
            self.typing.sent = true;
            return self.build_typing_request(false);
        }
        None
    }

    /// Handle text content from clipboard: file path detection or plain text insert.
    /// Insert clipboard text into the input buffer (trimmed). Returns early with a status message
    /// if the text is empty. File paths are treated as plain text — use `/attach` to attach files.
    fn handle_paste_text(&mut self, text: &str) -> Option<SendRequest> {
        let text = text.trim();
        if text.is_empty() {
            self.status_message = "Clipboard is empty".to_string();
            return None;
        }
        self.handle_paste(text.to_string())
    }

    /// Save clipboard image data to a temp PNG file and stage it as an attachment.
    fn handle_clipboard_image(&mut self, img_data: arboard::ImageData) -> Option<SendRequest> {
        use image::{ImageBuffer, RgbaImage};

        let width = img_data.width as u32;
        let height = img_data.height as u32;

        let img: RgbaImage = match ImageBuffer::from_raw(width, height, img_data.bytes.into_owned())
        {
            Some(img) => img,
            None => {
                self.status_message = "Failed to decode clipboard image".to_string();
                return None;
            }
        };

        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S%.3f");
        let filename = format!("clipboard_{timestamp}.png");
        let path = self.paste_temp_path.join(&filename);

        if let Err(e) = std::fs::create_dir_all(&self.paste_temp_path) {
            self.status_message = format!("Cannot create paste directory: {e}");
            return None;
        }

        if let Err(e) = img.save(&path) {
            self.status_message = format!("Failed to save clipboard image: {e}");
            return None;
        }

        self.pending_attachment = Some(path);
        self.status_message = format!("Pasted image: {filename}");
        None
    }

    /// Handle the `/paste` command: read clipboard and act on contents.
    /// Image data → temp PNG → pending_attachment. Text → input buffer.
    /// Note: the full clipboard-read path is not unit-tested because `arboard::Clipboard`
    /// requires a display/compositor and cannot be mocked. The individual handlers
    /// (`handle_clipboard_image`, `handle_paste_text`) are tested directly instead.
    pub(crate) fn handle_paste_command(&mut self) -> Option<SendRequest> {
        if self.lock.is_locked() {
            return None;
        }
        if self.active_conversation.is_none() {
            self.status_message = "No active conversation".to_string();
            return None;
        }

        let mut clipboard = match arboard::Clipboard::new() {
            Ok(c) => c,
            Err(e) => {
                self.status_message = format!("Clipboard error: {e}");
                return None;
            }
        };

        // Try image first (screenshots add both image and file path to clipboard — prefer image)
        if let Ok(img_data) = clipboard.get_image() {
            return self.handle_clipboard_image(img_data);
        }

        // Try text — inserts into input buffer
        if let Ok(text) = clipboard.get_text() {
            return self.handle_paste_text(&text);
        }

        self.status_message = "Clipboard is empty or unsupported format".to_string();
        None
    }

    /// Accept the currently selected autocomplete candidate.
    pub fn apply_autocomplete(&mut self) {
        match self.autocomplete.mode {
            AutocompleteMode::Command => {
                if let Some(&cmd_idx) = self
                    .autocomplete
                    .command_candidates
                    .get(self.autocomplete.index)
                {
                    let cmd = &COMMANDS[cmd_idx];
                    if cmd.args.is_empty() {
                        self.input.buffer = cmd.name.to_string();
                    } else {
                        self.input.buffer = format!("{} ", cmd.name);
                    }
                    self.input.cursor = self.input.buffer.len();
                    self.close_overlay();
                    self.autocomplete.command_candidates.clear();
                    self.autocomplete.index = 0;
                }
            }
            AutocompleteMode::Mention => {
                if let Some((_phone, name, uuid)) = self
                    .autocomplete
                    .mention_candidates
                    .get(self.autocomplete.index)
                    .cloned()
                {
                    // Replace @partial with @FullName followed by a space
                    let replacement = format!("@{name} ");
                    let before = &self.input.buffer[..self.autocomplete.mention_trigger_pos];
                    let after = &self.input.buffer[self.input.cursor..];
                    self.input.buffer = format!("{before}{replacement}{after}");
                    self.input.cursor = self.autocomplete.mention_trigger_pos + replacement.len();
                    // Record for outgoing mention
                    self.autocomplete.pending_mentions.push((name, uuid));
                    self.close_overlay();
                    self.autocomplete.mention_candidates.clear();
                    self.autocomplete.index = 0;
                }
            }
            AutocompleteMode::Join => {
                if let Some((_display, value)) = self
                    .autocomplete
                    .join_candidates
                    .get(self.autocomplete.index)
                    .cloned()
                {
                    self.input.buffer = format!("/join {value}");
                    self.input.cursor = self.input.buffer.len();
                    self.close_overlay();
                    self.autocomplete.join_candidates.clear();
                    self.autocomplete.index = 0;
                }
            }
        }
    }

    pub(crate) fn save_scroll_position(&mut self) {
        if let Some(ref id) = self.active_conversation {
            self.scroll
                .positions
                .insert(id.clone(), (self.scroll.offset, self.scroll.focused_index));
        }
    }

    fn restore_scroll_position(&mut self, conv_id: &str) {
        if let Some(&(offset, focus)) = self.scroll.positions.get(conv_id) {
            self.scroll.offset = offset;
            self.scroll.focused_index = focus;
        } else {
            self.scroll.offset = 0;
            self.scroll.focused_index = None;
        }
    }

    /// Shared preamble for every conversation switch: persists read/scroll
    /// state and clears all composer state aimed at the outgoing
    /// conversation. `reply_target` and `editing_message` carry
    /// (timestamp, conv_id) pairs targeting the old conversation; if they
    /// survive a switch, the next Enter sends the new text as an edit or
    /// quoted reply in the wrong chat (#481).
    fn leave_active_conversation(&mut self) {
        self.mark_read();
        self.save_scroll_position();
        self.pending_attachment = None;
        self.reply_target = None;
        self.editing_message = None;
        self.reset_typing_with_stop();
        self.input.reset_for_conv_switch();
        self.sync.pin = None;
        self.scroll.window_extra = 0;
        self.scroll.can_extend_in_memory = false;
        self.clear_kitty_placements();
    }

    /// Focus an existing conversation: queue read receipts, clear unread,
    /// restore scroll. Callers ensure the conversation exists.
    fn activate_conversation(&mut self, id: &str) {
        let read_from = self.store.last_read_index.get(id).copied().unwrap_or(0);
        self.queue_read_receipts_for_conv(id, read_from);
        self.active_conversation = Some(id.to_string());
        if let Some(conv) = self.store.conversations.get_mut(id) {
            conv.unread = 0;
        }
        self.restore_scroll_position(id);

        self.update_status();
    }

    /// Create (if needed) and focus a 1:1 conversation for a known contact
    /// key, preferring the contact's display name over `fallback_name`.
    fn open_contact_conversation(&mut self, key: &str, fallback_name: &str) {
        let name = self
            .store
            .contact_names
            .get(key)
            .cloned()
            .unwrap_or_else(|| fallback_name.to_string());
        self.store
            .get_or_create_conversation(key, &name, false, &self.db);
        self.activate_conversation(key);
    }

    /// Switch to (or create) the conversation named by `target`.
    ///
    /// An unknown `@handle` needs a getUserStatus round-trip: the request is
    /// queued on `pending.trigger_sends` (drained by the main loop, so every
    /// call site dispatches it uniformly) and the response is correlated via
    /// `pending.username_resolve` (#612).
    pub(crate) fn join_conversation(&mut self, target: &str) {
        self.leave_active_conversation();
        // Stale from the previous conversation; the next render sets it. Prevents
        // evicting the new conversation's image_lines before it has drawn (#492).
        self.scroll.render_window_start = 0;

        // Username target: @handle or u:handle (#612)
        if let Some(handle) = target
            .strip_prefix('@')
            .or_else(|| target.strip_prefix("u:"))
        {
            let handle_lower = handle.to_lowercase();
            if let Some(key) = self.store.username_to_id.get(&handle_lower).cloned() {
                // Known contact: open (or create) its conversation under the
                // contact key (phone number, or uuid for username-only contacts).
                self.open_contact_conversation(&key, &format!("@{handle}"));
                return;
            }
            // A conversation literally named "@handle" (e.g. restored from
            // the DB before listContacts repopulates the username maps) is a
            // local match — don't burn a network round-trip on it.
            let found_id = self
                .store
                .conversations
                .iter()
                .find(|(_, conv)| conv.name.eq_ignore_ascii_case(target))
                .map(|(id, _)| id.clone());
            if let Some(id) = found_id {
                self.activate_conversation(&id);
                return;
            }
            // Unknown handle: a full username (name.discriminator) can be
            // resolved to a uuid via getUserStatus; a bare nickname cannot.
            if handle_lower.contains('.') {
                self.pending.username_resolve = Some(handle_lower.clone());
                self.pending
                    .trigger_sends
                    .push(SendRequest::ResolveUsername {
                        username: handle_lower.clone(),
                    });
                self.status_message = format!("Resolving @{handle_lower}...");
                return;
            }
            self.status_message =
                format!("Unknown username @{handle} - use the full handle (name.123)");
            return;
        }

        // Try exact match first
        if self.store.conversations.contains_key(target) {
            self.activate_conversation(target);
            return;
        }

        // Try matching by name (case-insensitive)
        let target_lower = target.to_lowercase();
        let found_id = self
            .store
            .conversations
            .iter()
            .find(|(_, conv)| conv.name.to_lowercase().contains(&target_lower))
            .map(|(id, _)| id.clone());

        if let Some(id) = found_id {
            self.activate_conversation(&id);
            return;
        }

        // Known 1:1 contact key without a conversation yet (e.g. a
        // username-only contact selected in the contacts overlay, keyed by
        // uuid). Group ids also live in contact_names — those must not be
        // resurrected as 1:1 conversations (#612).
        if self.store.contact_names.contains_key(target) && !self.store.groups.contains_key(target)
        {
            self.open_contact_conversation(target, target);
            return;
        }

        // Create a new 1:1 conversation if target looks like a phone number
        if target.starts_with('+') {
            self.store
                .get_or_create_conversation(target, target, false, &self.db);
            self.active_conversation = Some(target.to_string());
            self.scroll.offset = 0;
            self.scroll.focused_index = None;
            self.update_status();
        } else {
            self.status_message = format!("Conversation not found: {target}");
        }
    }

    /// Capture the viewport pin anchor on the first sync message arrival
    /// for the active conversation. The pin records (a) the timestamp of
    /// the message currently at the bottom of the conversation -- the one
    /// the user was looking at when sync began -- and (b) the user's
    /// `scroll.offset` at that moment. The renderer uses both to keep
    /// the pinned message at its original screen position regardless of
    /// how many lines the incoming sync messages add.
    ///
    /// Skipped when:
    /// - sync is not active (no need to pin)
    /// - the user has manually scrolled (their explicit choice wins)
    /// - the pin is already set (only first arrival captures)
    /// - the message is for a non-active conversation (pin is per-active)
    /// - the conversation has no prior messages (nothing to anchor to)
    pub(crate) fn maybe_capture_sync_pin(&mut self, conv_id: &str) {
        if !self.sync.active || self.sync.user_scrolled || self.sync.pin.is_some() {
            return;
        }
        if self.active_conversation.as_deref() != Some(conv_id) {
            return;
        }
        if let Some(conv) = self.store.conversations.get(conv_id)
            && let Some(last) = conv.messages.last()
        {
            self.sync.pin = Some((last.timestamp, self.scroll.offset));
        }
    }

    /// Whether the startup spinner should force a redraw on this tick.
    ///
    /// The spinner's *counter* advances at 80ms cadence whenever `loading`
    /// is true (see main.rs and #426), but the *redraw* it triggers is gated
    /// here. During the initial sync burst (`sync.active`), drain_events
    /// throttles redraws to 500ms so the UI stays responsive; forcing a
    /// redraw on every spinner tick (12.5fps) would bypass that throttle.
    /// Returning false during sync defers spinner rendering to the next
    /// throttled redraw, which still picks up the advanced counter value
    /// -- so the spinner animates at ~2fps during sync, 12.5fps after.
    pub fn should_tick_spinner(&self) -> bool {
        self.loading && !self.sync.active
    }

    pub fn next_conversation(&mut self) {
        if self.store.conversation_order.is_empty() {
            return;
        }
        self.clear_sidebar_filter();
        self.leave_active_conversation();
        let idx = self
            .active_conversation
            .as_ref()
            .and_then(|id| self.store.conversation_order.iter().position(|x| x == id))
            .map(|i| (i + 1) % self.store.conversation_order.len())
            .unwrap_or(0);
        let new_id = self.store.conversation_order[idx].clone();
        let read_from = self
            .store
            .last_read_index
            .get(&new_id)
            .copied()
            .unwrap_or(0);
        self.queue_read_receipts_for_conv(&new_id, read_from);
        self.active_conversation = Some(new_id.clone());
        if let Some(conv) = self.store.conversations.get_mut(&new_id) {
            conv.unread = 0;
        }
        self.restore_scroll_position(&new_id);

        self.update_status();
    }

    pub fn prev_conversation(&mut self) {
        if self.store.conversation_order.is_empty() {
            return;
        }
        self.clear_sidebar_filter();
        self.leave_active_conversation();
        let len = self.store.conversation_order.len();
        let idx = self
            .active_conversation
            .as_ref()
            .and_then(|id| self.store.conversation_order.iter().position(|x| x == id))
            .map(|i| if i == 0 { len - 1 } else { i - 1 })
            .unwrap_or(0);
        let new_id = self.store.conversation_order[idx].clone();
        let read_from = self
            .store
            .last_read_index
            .get(&new_id)
            .copied()
            .unwrap_or(0);
        self.queue_read_receipts_for_conv(&new_id, read_from);
        self.active_conversation = Some(new_id.clone());
        if let Some(conv) = self.store.conversations.get_mut(&new_id) {
            conv.unread = 0;
        }
        self.restore_scroll_position(&new_id);

        self.update_status();
    }

    pub(crate) fn update_status(&mut self) {
        if let Some(ref id) = self.active_conversation {
            if let Some(conv) = self.store.conversations.get(id) {
                let prefix = if conv.is_group { "#" } else { "" };
                self.status_message = format!("connected | {}{}", prefix, conv.name);
            }
            // Show message request overlay for unaccepted conversations.
            //
            // The pre-refactor model set `show_message_request` as an independent
            // bool that coexisted with other overlays; dispatch priority decided
            // which was visible. Naively calling `open_overlay(MessageRequest)`
            // here would clobber any higher-priority App-owned overlay the user
            // had open (e.g. closing Settings mid-edit when Tab switches to an
            // unaccepted conversation). Only claim the slot when no other
            // App-owned overlay is active.
            let should_show = self
                .active_conversation
                .as_ref()
                .and_then(|id| self.store.conversations.get(id))
                .is_some_and(|c| !c.accepted);
            if should_show {
                self.try_open_overlay(OverlayKind::MessageRequest);
            } else if self.is_overlay(OverlayKind::MessageRequest) {
                self.close_overlay();
            }
        } else {
            self.status_message = "connected | no conversation selected".to_string();
            if self.is_overlay(OverlayKind::MessageRequest) {
                self.close_overlay();
            }
        }
    }

    pub fn set_connected(&mut self) {
        self.connected = true;
        self.status_message = "connected | no conversation selected".to_string();
    }

    /// Get the message at the current scroll position.
    /// Returns the message at the bottom of the visible viewport.
    /// scroll.offset=0 means the newest message; higher values go older.
    pub fn selected_message(&self) -> Option<&DisplayMessage> {
        let conv_id = self.active_conversation.as_ref()?;
        let conv = self.store.conversations.get(conv_id)?;
        let index = self
            .scroll
            .focused_index
            .unwrap_or_else(|| conv.messages.len().saturating_sub(1));
        conv.messages.get(index)
    }

    /// Jump to the next or previous non-system message.
    /// `older` = true means go toward older messages (K), false means newer (J).
    pub(crate) fn jump_to_adjacent_message(&mut self, older: bool) {
        let conv_id = match self.active_conversation.as_ref() {
            Some(id) => id.clone(),
            None => return,
        };
        let conv = match self.store.conversations.get(&conv_id) {
            Some(c) => c,
            None => return,
        };
        let total = conv.messages.len();
        if total == 0 {
            return;
        }

        // Bootstrap: if no message is focused yet, pick the last non-system message
        // and enter scroll mode so the highlight becomes visible.
        let current = match self.scroll.focused_index {
            Some(i) => i,
            None => {
                let start = (0..total).rev().find(|&i| !conv.messages[i].is_system);
                if let Some(s) = start {
                    self.scroll.focused_index = Some(s);
                    if self.scroll.offset == 0 {
                        self.scroll.offset = 1;
                    }
                }
                return;
            }
        };

        let target = if older {
            (0..current).rev().find(|&i| !conv.messages[i].is_system)
        } else {
            ((current + 1)..total).find(|&i| !conv.messages[i].is_system)
        };

        if let Some(t) = target {
            self.scroll.focused_index = Some(t);
            // scroll.offset is adjusted by the renderer to keep the focused message visible
        }
    }

    /// Copy the selected message text to the system clipboard.
    /// If `full_line` is true, copies "[HH:MM] <sender> body"; otherwise just the body.
    pub fn copy_selected_message(&mut self, full_line: bool) {
        let text = match self.selected_message() {
            Some(msg) if msg.is_system => Some(msg.body.clone()),
            Some(msg) => {
                if full_line {
                    Some(format!(
                        "[{}] <{}> {}",
                        msg.format_time(),
                        msg.sender,
                        msg.body
                    ))
                } else {
                    Some(msg.body.clone())
                }
            }
            None => None,
        };

        let Some(text) = text else {
            self.status_message = "No message to copy".to_string();
            return;
        };

        match arboard::Clipboard::new() {
            Ok(mut clipboard) => match clipboard.set_text(&text) {
                Ok(()) => {
                    self.status_message = "Copied to clipboard".to_string();
                    if self.notifications.clipboard_clear_seconds > 0 {
                        self.notifications.clipboard_set_at = Some(std::time::Instant::now());
                    }
                }
                Err(e) => {
                    self.status_message = format!("Clipboard error: {e}");
                }
            },
            Err(e) => {
                self.status_message = format!("Clipboard error: {e}");
            }
        }
    }

    /// Clear the clipboard if auto-clear timer has expired.
    pub fn check_clipboard_clear(&mut self) {
        if let Some(set_at) = self.notifications.clipboard_set_at
            && set_at.elapsed().as_secs() >= self.notifications.clipboard_clear_seconds
        {
            self.notifications.clipboard_set_at = None;
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                let _ = clipboard.set_text("");
            }
        }
    }

    /// Delete any paste temp files whose 10s delay has elapsed.
    /// Called each tick from the main event loop.
    pub fn cleanup_paste_files(&mut self) {
        self.pending_paste_cleanups
            .retain(|_rpc_id, (path, delete_after)| {
                if Instant::now() >= *delete_after {
                    let _ = std::fs::remove_file(path);
                    false
                } else {
                    true
                }
            });
    }

    // --- Mouse support ---

    /// Returns true if any overlay is currently visible.
    ///
    /// Derived from `active_overlay` so it can never drift from dispatch.
    pub fn has_overlay(&self) -> bool {
        self.active_overlay().is_some()
    }

    /// Handle a mouse event. Returns an optional SendRequest (currently unused but future-proof).
    pub fn handle_mouse_event(&mut self, event: MouseEvent) -> Option<SendRequest> {
        crate::handlers::keys::handle_mouse_event(self, event)
    }

    pub(crate) fn open_url(&mut self, url: &str) {
        // Only allow http/https URLs to prevent local file access via file:// etc.
        if !url.starts_with("http://") && !url.starts_with("https://") {
            self.status_message = "Only http/https URLs can be opened".to_string();
            return;
        }
        if let Err(e) = open::that(url) {
            self.status_message = format!("Failed to open URL: {e}");
        }
    }

    fn open_file(&mut self, uri: &str) {
        let path = file_uri_to_path(uri);
        match classify_file_open(&path, &self.media.download_dir) {
            OpenFileDecision::NotFound => {
                self.status_message = format!("File not found: {path}");
            }
            OpenFileDecision::OutsideDownloadDir => {
                self.status_message =
                    "Refusing to open a file outside the download directory".to_string();
            }
            OpenFileDecision::UnsafeType => {
                self.status_message =
                    format!("Not opening this file type automatically; saved at {path}");
            }
            OpenFileDecision::Allow(canon) => {
                let filename = canon
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| path.clone());
                // Voice/audio: play inline via a detected CLI player (#199)
                // instead of handing off to the OS default app. Falls back to
                // open::that when no player is installed. Pressing `o` on the
                // message that is already playing stops it instead (#618).
                if crate::audio::is_audio_path(&canon)
                    && self.media.playing.as_ref().is_some_and(|p| p.path == canon)
                {
                    self.stop_voice_playback();
                    self.status_message = format!("Stopped {filename}");
                } else if crate::audio::is_audio_path(&canon)
                    && let Some(player) =
                        crate::audio::detect_player(self.media.audio_player.as_deref())
                {
                    // Only one voice plays at a time.
                    self.stop_voice_playback();
                    match crate::audio::play(&canon, &player) {
                        Ok(child) => {
                            let duration = crate::audio::ogg_opus_duration(&canon);
                            self.media.playing = Some(crate::domain::PlayingVoice {
                                child,
                                started: Instant::now(),
                                duration,
                                label: filename.clone(),
                                path: canon.clone(),
                            });
                            self.status_message = format!("Playing {filename}");
                        }
                        Err(e) => self.status_message = format!("Failed to play {filename}: {e}"),
                    }
                } else {
                    match open::that(&canon) {
                        Ok(()) => self.status_message = format!("Opened {filename}"),
                        Err(e) => self.status_message = format!("Failed to open: {e}"),
                    }
                }
            }
        }
    }

    /// Export the active conversation's messages to a file in the given
    /// format (plain text, Markdown, or JSON). Rendering lives in
    /// `crate::export`; this owns file naming, sanitizing, and the write.
    pub(crate) fn export_chat_history(
        &mut self,
        format: crate::export::ExportFormat,
        limit: Option<usize>,
    ) {
        let conv_id = match self.active_conversation.as_ref() {
            Some(id) => id.clone(),
            None => {
                self.status_message = "No active conversation to export".to_string();
                return;
            }
        };
        let conv = match self.store.conversations.get(&conv_id) {
            Some(c) => c,
            None => return,
        };

        let messages = &conv.messages;
        let export_msgs: &[DisplayMessage] = match limit {
            Some(n) => &messages[messages.len().saturating_sub(n)..],
            None => messages,
        };

        if export_msgs.is_empty() {
            self.status_message = "No messages to export".to_string();
            return;
        }

        let safe_name: String = conv
            .name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let now = chrono::Local::now();
        let filename = format!(
            "siggy-export-{safe_name}-{}.{}",
            now.format("%Y-%m-%d"),
            format.extension()
        );

        let output = crate::export::render(format, &conv.name, export_msgs, now);

        // Strip control characters from text/Markdown exports (message bodies
        // and names are remote-controlled) so the saved file cannot inject
        // terminal escapes when viewed with cat/less (#504). Structural
        // newlines and tabs are preserved. JSON needs no stripping: serde
        // escapes control characters inside strings.
        let output = if format == crate::export::ExportFormat::Json {
            output
        } else {
            crate::debug_log::strip_control_chars(&output, true)
        };

        // Write to download dir or home
        let dir = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let path = dir.join(&filename);

        match std::fs::write(&path, &output) {
            Ok(()) => {
                self.status_message = format!(
                    "Exported {} messages to {}",
                    export_msgs.len(),
                    path.display()
                );
            }
            Err(e) => {
                self.status_message = format!("Export failed: {e}");
            }
        }
    }
}

/// Simple point-in-rect hit test for mouse coordinates.
pub(crate) fn is_in_rect(col: u16, row: u16, rect: Rect) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}

/// Extract a local file path from a file:/// URI. On Unix the third slash is the
/// root path separator, so it must be preserved; on Windows it's just the scheme.
fn file_uri_to_path(uri: &str) -> String {
    let uri = uri.trim();
    if let Some(rest) = uri.strip_prefix("file:///") {
        #[cfg(windows)]
        {
            rest.to_string()
        }
        #[cfg(not(windows))]
        {
            format!("/{rest}")
        }
    } else if let Some(rest) = uri.strip_prefix("file://") {
        rest.to_string()
    } else {
        uri.to_string()
    }
}

/// File extensions safe to hand to the OS shell-open handler. Anything not on
/// this list (executables, scripts, shortcuts, unknown types) is shown but not
/// opened, because attachment filenames are chosen by the remote sender.
const SAFE_OPEN_EXTENSIONS: &[&str] = &[
    // images
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "heic", "avif", // video
    "mp4", "mov", "webm", "mkv", "avi", "m4v", // audio
    "mp3", "ogg", "oga", "m4a", "wav", "flac", "opus", "aac", // documents
    "pdf", "txt", "md", "log", "csv", "vcf",
];

/// Outcome of vetting a local file path extracted from a message body.
#[derive(Debug, PartialEq, Eq)]
enum OpenFileDecision {
    /// Safe to open: canonical path, inside the download dir, allowlisted type.
    Allow(PathBuf),
    NotFound,
    OutsideDownloadDir,
    UnsafeType,
}

/// Vet a file path before shell-opening it. Message bodies are remote-
/// controlled text, so the path must canonicalize inside `download_dir` and
/// carry an allowlisted extension; the OS open handler would otherwise
/// execute attacker-named files (.hta, .lnk, .desktop) on a keypress.
fn classify_file_open(path: &str, download_dir: &Path) -> OpenFileDecision {
    let Ok(canon) = Path::new(path).canonicalize() else {
        return OpenFileDecision::NotFound;
    };
    let Ok(canon_dir) = download_dir.canonicalize() else {
        // No usable download dir (empty/missing): never open anything.
        return OpenFileDecision::OutsideDownloadDir;
    };
    if !canon.starts_with(&canon_dir) {
        return OpenFileDecision::OutsideDownloadDir;
    }
    let ext = canon
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some(e) if SAFE_OPEN_EXTENSIONS.contains(&e) => OpenFileDecision::Allow(canon),
        _ => OpenFileDecision::UnsafeType,
    }
}

/// Extract the first `file:///` URI from a message body.
/// Stops at whitespace or `)` to handle `[image: name](file:///path)` format.
/// Anything extracted here is untrusted; `open_file` vets it via
/// `classify_file_open` before handing it to the OS.
fn extract_file_uri(body: &str) -> Option<String> {
    let pos = body.find("file:///")?;
    let rest = &body[pos..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == ')')
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// Extract the first `https://` or `http://` URL from a message body.
/// Stops at whitespace or `)`.
fn extract_http_url(body: &str) -> Option<String> {
    let mut best: Option<(usize, &str)> = None;
    for scheme in &["https://", "http://"] {
        if let Some(pos) = body.find(scheme)
            && (best.is_none() || pos < best.unwrap().0)
        {
            best = Some((pos, scheme));
        }
    }
    let (pos, _) = best?;
    let rest = &body[pos..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == ')')
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

impl App {
    /// Populate the app with demo conversations for `--demo` mode and snapshot tests.
    /// `base_date` is used for deterministic timestamps instead of `Utc::now()`.
    pub(crate) fn populate_demo_data(&mut self, base_date: chrono::NaiveDate) {
        crate::demo::populate_demo_data(self, base_date)
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
