//! Central application state and event handling.
//!
//! [`App`] owns conversations, input buffer, mode (Normal/Insert), and every
//! overlay state struct. [`App::handle_signal_event`] is the single entry point
//! for all backend events from `signal::client`. Conversations are keyed by
//! phone number (1:1) or group ID; the [`ConversationStore`] sub-struct holds
//! the persistent map and ordered sidebar list.

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::text::Line;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

pub use crate::autocomplete::AutocompleteMode;
use crate::autocomplete::AutocompleteState;
pub use crate::conversation_store::{Conversation, DisplayMessage, Quote};
use crate::conversation_store::{ConversationStore, db_warn, short_name};
use crate::db::Database;
use crate::domain::{
    ActionMenuState, ContactsOverlayState, EmojiPickerAction, EmojiPickerSource, EmojiPickerState,
    FilePickerState, ForwardOverlayState, GroupMenuOverlayState, ImageState, InputState,
    KeybindingsOverlayState, MouseState, NotificationState, PendingState, PinDurationOverlayState,
    PollVoteOverlayState, ProfileOverlayState, ReactionState, ScrollState, SearchAction,
    SearchState, SettingsProfileOverlayState, ThemePickerState, TypingState, VerifyOverlayState,
};
use crate::image_render;
use crate::image_render::ImageProtocol;
use crate::input::COMMANDS;
use crate::keybindings::{self, BindingMode, KeyAction, KeyBindings};
use crate::list_overlay::{self, ListKeyAction, classify_list_key};
use crate::mute::MuteState;
use crate::signal::types::{
    Contact, Group, IdentityInfo, Mention, MessageStatus, PollData, PollOption, PollVote, Reaction,
    SignalEvent, SignalMessage, StyleType, TrustLevel,
};
use crate::theme::{self, Theme};

/// Sentinel lifetime for paste temp files awaiting send confirmation from signal-cli.
/// If signal-cli never confirms, the file is deleted after this many seconds.
pub const PASTE_CLEANUP_SENTINEL_SECS: u64 = 3600;

/// How long after send confirmation to wait before deleting a paste temp file.
const PASTE_CLEANUP_DELAY_SECS: u64 = 10;

/// Find the byte position one character forward from `pos` in `buf`.
fn next_char_pos(buf: &str, pos: usize) -> usize {
    if pos >= buf.len() {
        return buf.len();
    }
    pos + buf[pos..].chars().next().map_or(1, |c| c.len_utf8())
}

/// Find the byte position one character backward from `pos` in `buf`.
fn prev_char_pos(buf: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    pos - buf[..pos].chars().next_back().map_or(1, |c| c.len_utf8())
}

/// Snap a byte position to the nearest valid char boundary at or before `pos`.
fn floor_char_boundary(buf: &str, pos: usize) -> usize {
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
#[derive(Default)]
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
        let insert_idx = {
            let conv = self.store.conversations.get_mut(conv_id)?;
            let pos = if ordered_insert {
                conv.messages
                    .partition_point(|m| m.timestamp_ms <= timestamp_ms)
            } else {
                conv.messages.len()
            };
            conv.messages.insert(pos, msg);
            pos
        };

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

/// Fire an OS-level desktop notification (runs on a blocking thread to avoid stalling async).
fn show_desktop_notification(
    sender: &str,
    body: &str,
    is_group: bool,
    group_name: Option<&str>,
    preview_level: &str,
) {
    let (title, preview) = match preview_level {
        "minimal" => ("New message".to_string(), String::new()),
        "sender" => {
            let t = if is_group {
                match group_name {
                    Some(gn) => format!("{} — {}", gn, sender),
                    None => sender.to_string(),
                }
            } else {
                sender.to_string()
            };
            (t, "New message".to_string())
        }
        _ => {
            // "full" or any unknown value — current behavior
            let t = if is_group {
                match group_name {
                    Some(gn) => format!("{} — {}", gn, sender),
                    None => sender.to_string(),
                }
            } else {
                sender.to_string()
            };
            (t, body.chars().take(100).collect())
        }
    };

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
}

/// An image visible on screen, for native protocol overlay rendering.
#[derive(PartialEq, Eq)]
pub struct VisibleImage {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    /// Total image height in cells (before viewport clipping).
    pub full_height: u16,
    /// Cells cropped from the top when the image is partially scrolled out.
    pub crop_top: u16,
    pub path: String,
}

/// Result from a background image render task.
pub struct ImageRenderResult {
    pub conv_id: String,
    pub timestamp_ms: i64,
    pub is_preview: bool,
    pub lines: Option<Vec<Line<'static>>>,
    pub image_path: Option<String>,
    /// Pre-encoded PNG for native_image_cache: (path, base64, pixel_w, pixel_h)
    pub pre_native_png: Option<(String, String, u32, u32)>,
    /// Pre-encoded full Sixel for sixel_cache: (path, sixel_string)
    pub pre_sixel: Option<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Insert,
}

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

/// An action available in the message action menu.
pub struct MenuAction {
    pub label: &'static str,
    pub key_hint: &'static str,
    pub nerd_icon: &'static str,
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
    /// Pending quit confirmation (unsent text in input buffer)
    pub quit_confirm: bool,
    /// Our own account number for identifying outgoing messages
    pub account: String,
    /// Resizable sidebar width (min 14, max 40)
    pub sidebar_width: u16,
    /// Display sidebar on the right side instead of left
    pub sidebar_on_right: bool,
    /// Current filter text for sidebar
    pub sidebar_filter: String,
    /// Filtered conversation IDs matching the filter
    pub sidebar_filtered: Vec<String>,
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
    /// Cursor position in settings list
    pub settings_index: usize,
    /// Cursor position in customize sub-menu
    pub customize_index: usize,
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
    /// Mouse enabled state when settings overlay opened (for deferred toggle)
    pub settings_mouse_snapshot: bool,
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

/// A request from the UI to the main loop to send something.
pub enum SendRequest {
    Message {
        recipient: String,
        body: String,
        is_group: bool,
        local_ts_ms: i64,
        mentions: Vec<(usize, String)>,
        attachment: Option<PathBuf>,
        quote_timestamp: Option<i64>,
        quote_author: Option<String>,
        quote_body: Option<String>,
    },
    Reaction {
        conv_id: String,
        emoji: String,
        is_group: bool,
        target_author: String,
        target_timestamp: i64,
        remove: bool,
    },
    Edit {
        recipient: String,
        body: String,
        is_group: bool,
        edit_timestamp: i64,
        local_ts_ms: i64,
        mentions: Vec<(usize, String)>,
        quote_timestamp: Option<i64>,
        quote_author: Option<String>,
        quote_body: Option<String>,
    },
    RemoteDelete {
        recipient: String,
        is_group: bool,
        target_timestamp: i64,
    },
    Typing {
        recipient: String,
        is_group: bool,
        stop: bool,
    },
    ReadReceipt {
        recipient: String,
        timestamps: Vec<i64>,
    },
    UpdateExpiration {
        conv_id: String,
        is_group: bool,
        seconds: i64,
    },
    CreateGroup {
        name: String,
    },
    AddGroupMembers {
        group_id: String,
        members: Vec<String>,
    },
    RemoveGroupMembers {
        group_id: String,
        members: Vec<String>,
    },
    RenameGroup {
        group_id: String,
        name: String,
    },
    LeaveGroup {
        group_id: String,
    },
    MessageRequestResponse {
        recipient: String,
        is_group: bool,
        response_type: String,
    },
    Block {
        recipient: String,
        is_group: bool,
    },
    Unblock {
        recipient: String,
        is_group: bool,
    },
    Pin {
        recipient: String,
        is_group: bool,
        target_author: String,
        target_timestamp: i64,
        pin_duration: i64,
    },
    Unpin {
        recipient: String,
        is_group: bool,
        target_author: String,
        target_timestamp: i64,
    },
    PollCreate {
        recipient: String,
        is_group: bool,
        question: String,
        options: Vec<String>,
        allow_multiple: bool,
        local_ts_ms: i64,
    },
    PollVote {
        recipient: String,
        is_group: bool,
        poll_author: String,
        poll_timestamp: i64,
        option_indexes: Vec<i64>,
        vote_count: i64,
    },
    PollTerminate {
        recipient: String,
        is_group: bool,
        poll_timestamp: i64,
    },
    ListIdentities,
    TrustIdentity {
        recipient: String,
        safety_number: String,
    },
    UpdateProfile {
        given_name: String,
        family_name: String,
        about: String,
        about_emoji: String,
    },
}

/// A single settings toggle entry: label, getter, setter, and optional config persistence.
pub struct SettingDef {
    pub label: &'static str,
    pub hint: &'static str,
    get: fn(&App) -> bool,
    set: fn(&mut App, bool),
    save: Option<fn(&mut crate::config::Config, bool)>,
    on_toggle: Option<fn(&mut App)>,
}

/// Section boundary indices within the SETTINGS array.
pub const SETTINGS_SECTION_DISPLAY: usize = 3;
pub const SETTINGS_SECTION_MESSAGES: usize = 9;
pub const SETTINGS_SECTION_INTERFACE: usize = 12;

/// Visual order of settings items (logical indices into the combined toggle+special list).
/// Toggle indices 0..SETTINGS.len() map to SETTINGS entries.
/// Special indices: SETTINGS.len() = preview, +1 = image mode, +2 = customize.
/// Navigation walks this array so j/k follows the visual layout.
pub const SETTINGS_VISUAL_ORDER: &[usize] = &[
    // Notifications
    0, 1, 2, 15, // DM, Group, Desktop, Notification preview
    // Display
    3, 4, 5, 6, 7, 8, 16, // Link previews .. Emoji to text, Image mode
    // Messages
    9, 10, 11, // Show reactions, Verbose reactions, Send read receipts
    // Interface
    12, 13, 14, 17, // Sidebar visible, Mouse, Sidebar on right, Customize...
];

pub const SETTINGS: &[SettingDef] = &[
    // — Notifications (0–2) —
    SettingDef {
        label: "Direct message notifications",
        hint: "Play a sound for incoming direct messages",
        get: |a| a.notifications.notify_direct,
        set: |a, v| a.notifications.notify_direct = v,
        save: Some(|c, v| c.notify_direct = v),
        on_toggle: None,
    },
    SettingDef {
        label: "Group message notifications",
        hint: "Play a sound for incoming group messages",
        get: |a| a.notifications.notify_group,
        set: |a, v| a.notifications.notify_group = v,
        save: Some(|c, v| c.notify_group = v),
        on_toggle: None,
    },
    SettingDef {
        label: "Desktop notifications",
        hint: "Show system notifications for new messages",
        get: |a| a.notifications.desktop_notifications,
        set: |a, v| a.notifications.desktop_notifications = v,
        save: Some(|c, v| c.desktop_notifications = v),
        on_toggle: None,
    },
    // — Display (3–8) —
    SettingDef {
        label: "Link previews",
        hint: "Show title and thumbnail for URLs",
        get: |a| a.image.show_link_previews,
        set: |a, v| a.image.show_link_previews = v,
        save: Some(|c, v| c.show_link_previews = v),
        on_toggle: None,
    },
    SettingDef {
        label: "Date separators",
        hint: "Show date lines between messages from different days",
        get: |a| a.date_separators,
        set: |a, v| a.date_separators = v,
        save: Some(|c, v| c.date_separators = v),
        on_toggle: None,
    },
    SettingDef {
        label: "Read receipts",
        hint: "Show delivery and read status on messages",
        get: |a| a.show_receipts,
        set: |a, v| a.show_receipts = v,
        save: Some(|c, v| c.show_receipts = v),
        on_toggle: None,
    },
    SettingDef {
        label: "Receipt colors",
        hint: "Colorize receipt indicators",
        get: |a| a.color_receipts,
        set: |a, v| a.color_receipts = v,
        save: Some(|c, v| c.color_receipts = v),
        on_toggle: None,
    },
    SettingDef {
        label: "Nerd Font icons",
        hint: "Use Nerd Font glyphs (requires a Nerd Font)",
        get: |a| a.nerd_fonts,
        set: |a, v| a.nerd_fonts = v,
        save: Some(|c, v| c.nerd_fonts = v),
        on_toggle: None,
    },
    SettingDef {
        label: "Emoji to text",
        hint: "Convert emoji to text emoticons/shortcodes",
        get: |a| a.reactions.emoji_to_text,
        set: |a, v| a.reactions.emoji_to_text = v,
        save: Some(|c, v| c.emoji_to_text = v),
        on_toggle: None,
    },
    // — Messages (9–11) —
    SettingDef {
        label: "Show reactions",
        hint: "Show emoji reactions on messages",
        get: |a| a.reactions.show_reactions,
        set: |a, v| a.reactions.show_reactions = v,
        save: Some(|c, v| c.show_reactions = v),
        on_toggle: None,
    },
    SettingDef {
        label: "Verbose reactions",
        hint: "Show names instead of just emoji counts",
        get: |a| a.reactions.verbose,
        set: |a, v| a.reactions.verbose = v,
        save: Some(|c, v| c.reaction_verbose = v),
        on_toggle: None,
    },
    SettingDef {
        label: "Send read receipts",
        hint: "Let contacts know when you read messages",
        get: |a| a.send_read_receipts,
        set: |a, v| a.send_read_receipts = v,
        save: Some(|c, v| c.send_read_receipts = v),
        on_toggle: None,
    },
    // — Interface (12–14) —
    SettingDef {
        label: "Sidebar visible",
        hint: "Show the conversation list sidebar",
        get: |a| a.sidebar_visible,
        set: |a, v| a.sidebar_visible = v,
        save: None, // runtime-only, not persisted
        on_toggle: None,
    },
    SettingDef {
        label: "Mouse support",
        hint: "Enable mouse click and scroll support",
        get: |a| a.mouse.enabled,
        set: |a, v| a.mouse.enabled = v,
        save: Some(|c, v| c.mouse_enabled = v),
        on_toggle: Some(|a| {
            a.mouse.pending_toggle = Some(a.mouse.enabled);
        }),
    },
    SettingDef {
        label: "Sidebar on right",
        hint: "Move the sidebar to the right side",
        get: |a| a.sidebar_on_right,
        set: |a, v| a.sidebar_on_right = v,
        save: Some(|c, v| c.sidebar_on_right = v),
        on_toggle: None,
    },
];

impl App {
    pub fn toggle_setting(&mut self, index: usize) {
        if let Some(def) = SETTINGS.get(index) {
            let cur = (def.get)(self);
            (def.set)(self, !cur);
            if let Some(hook) = def.on_toggle {
                hook(self);
            }
        }
    }

    pub fn setting_value(&self, index: usize) -> bool {
        SETTINGS.get(index).is_some_and(|def| (def.get)(self))
    }

    /// Persist current settings to the config file.
    fn save_settings(&self) {
        if self.is_demo {
            return;
        }
        let mut config = crate::config::Config::load(None).unwrap_or_default();
        config.account = self.account.clone();
        config.theme = self.theme.name.clone();
        config.keybinding_profile = self.keybindings.profile_name.clone();
        config.settings_profile = self.settings_profiles.name.clone();
        config.notification_preview = self.notifications.notification_preview.clone();
        config.image_mode = self.image.image_mode.clone();
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
                    self.image
                        .native_image_cache
                        .entry(path)
                        .or_insert((b64, pw, ph));
                }
                if let Some((path, sixel)) = result.pre_sixel {
                    self.image.sixel_cache.entry(path).or_insert(sixel);
                }
                drained = true;
            }
        }

        if self.image.image_mode == "none" {
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
        let end = len
            .saturating_sub(self.scroll.offset.saturating_sub(5))
            .min(len);
        let start = end.saturating_sub(60);

        // Collect work items to avoid borrow conflicts: (timestamp, path, max_width, is_preview)
        let mut work: Vec<(i64, String, u32, bool)> = Vec::new();
        for msg in &conv.messages[start..end] {
            if self.image.image_render_in_flight.len() + work.len() >= 4 {
                break;
            }
            if msg.body.starts_with("[image:")
                && msg.image_lines.is_none()
                && let Some(ref p) = msg.image_path
            {
                let key = (id.clone(), msg.timestamp_ms, false);
                if !self.image.image_render_in_flight.contains(&key) {
                    work.push((msg.timestamp_ms, p.clone(), 40, false));
                }
            }
            if self.image.show_link_previews
                && msg.preview_image_lines.is_none()
                && let Some(ref preview) = msg.preview
                && let Some(ref p) = preview.image_path
            {
                let key = (id.clone(), msg.timestamp_ms, true);
                if !self.image.image_render_in_flight.contains(&key) {
                    work.push((msg.timestamp_ms, p.clone(), 30, true));
                }
            }
        }

        // Spawn background render tasks
        let is_native = self.image.image_mode == "native";
        let is_sixel = self.image.image_protocol == image_render::ImageProtocol::Sixel;
        let cell_px = self.image.cell_px;
        for (ts, path, max_width, is_preview) in work {
            self.image
                .image_render_in_flight
                .insert((id.clone(), ts, is_preview));
            let tx = self.image.image_render_tx.clone();
            let cid = id.clone();
            tokio::task::spawn_blocking(move || {
                let lines = image_render::render_image(Path::new(&path), max_width);

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
        let preview_index = SETTINGS.len();
        let image_mode_index = SETTINGS.len() + 1;
        let customize_index = SETTINGS.len() + 2;

        // Find current position in visual order
        let visual_pos = SETTINGS_VISUAL_ORDER
            .iter()
            .position(|&i| i == self.settings_index)
            .unwrap_or(0);

        match code {
            KeyCode::Char('j') | KeyCode::Down if visual_pos + 1 < SETTINGS_VISUAL_ORDER.len() => {
                self.settings_index = SETTINGS_VISUAL_ORDER[visual_pos + 1];
            }
            KeyCode::Char('k') | KeyCode::Up if visual_pos > 0 => {
                self.settings_index = SETTINGS_VISUAL_ORDER[visual_pos - 1];
            }
            KeyCode::Char(' ') | KeyCode::Enter | KeyCode::Tab => {
                if self.settings_index == preview_index {
                    self.notifications.notification_preview =
                        match self.notifications.notification_preview.as_str() {
                            "full" => "sender".to_string(),
                            "sender" => "minimal".to_string(),
                            _ => "full".to_string(),
                        };
                } else if self.settings_index == image_mode_index {
                    self.image.image_mode = match self.image.image_mode.as_str() {
                        "native" => "halfblock".to_string(),
                        "halfblock" => "none".to_string(),
                        _ => "native".to_string(),
                    };
                } else if self.settings_index == customize_index {
                    self.open_overlay(OverlayKind::Customize);
                    self.customize_index = 0;
                } else {
                    self.toggle_setting(self.settings_index);
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.close_overlay();
                self.save_settings();
                self.fire_deferred_settings_hooks();
            }
            _ => {}
        }
    }

    /// Handle a key press in the Customize sub-menu (Theme, Keybindings, Profile).
    pub fn handle_customize_key(&mut self, code: KeyCode) {
        const ITEMS: usize = 3; // Theme, Keybindings, Profile
        match code {
            KeyCode::Char('j') | KeyCode::Down if self.customize_index + 1 < ITEMS => {
                self.customize_index += 1;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.customize_index = self.customize_index.saturating_sub(1);
            }
            KeyCode::Char(' ') | KeyCode::Enter | KeyCode::Tab => {
                self.close_overlay();
                self.save_settings();
                match self.customize_index {
                    0 => {
                        self.open_overlay(OverlayKind::ThemePicker);
                        self.theme_picker.index = self
                            .theme_picker
                            .available_themes
                            .iter()
                            .position(|t| t.name == self.theme.name)
                            .unwrap_or(0);
                    }
                    1 => {
                        self.open_overlay(OverlayKind::Keybindings);
                        self.keybindings_overlay.index = 0;
                    }
                    2 => {
                        self.open_settings_profile_manager();
                    }
                    _ => {}
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.close_overlay();
            }
            _ => {}
        }
    }

    /// Apply a profile without firing expensive hooks (image re-rendering).
    /// Hooks fire when the overlay closes (settings or profile manager Esc handler).
    fn apply_settings_profile_deferred(
        &mut self,
        profile: &crate::settings_profile::SettingsProfile,
    ) {
        profile.apply_to(self);
        self.settings_profiles.name = profile.name.clone();
    }

    /// Fire on_toggle hooks only for settings that changed since the overlay opened.
    fn fire_deferred_settings_hooks(&mut self) {
        if self.mouse.enabled != self.settings_mouse_snapshot {
            self.mouse.pending_toggle = Some(self.mouse.enabled);
        }
    }

    /// Open the settings profile manager overlay.
    fn open_settings_profile_manager(&mut self) {
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
        // Save-as text input mode
        if self.settings_profiles.save_as {
            match code {
                KeyCode::Enter => {
                    let name = self.settings_profiles.save_as_input.trim().to_string();
                    if name.is_empty() {
                        self.status_message = "Profile name cannot be empty".to_string();
                    } else if crate::settings_profile::is_builtin(&name) {
                        self.status_message = "Cannot overwrite built-in profile".to_string();
                    } else {
                        let profile =
                            crate::settings_profile::SettingsProfile::from_app(self, name.clone());
                        match crate::settings_profile::save_custom_profile(&profile) {
                            Ok(()) => {
                                self.settings_profiles.name = name;
                                self.settings_profiles.available =
                                    crate::settings_profile::all_settings_profiles();
                                self.settings_profiles.index = self
                                    .settings_profiles
                                    .available
                                    .iter()
                                    .position(|p| p.name == self.settings_profiles.name)
                                    .unwrap_or(0);
                                self.save_settings();
                                self.status_message = "Profile saved".to_string();
                            }
                            Err(e) => {
                                self.status_message = format!("Save failed: {e}");
                            }
                        }
                        self.settings_profiles.save_as = false;
                    }
                }
                KeyCode::Esc => {
                    self.settings_profiles.save_as = false;
                }
                KeyCode::Backspace => {
                    self.settings_profiles.save_as_input.pop();
                }
                KeyCode::Char(c) if self.settings_profiles.save_as_input.len() < 30 => {
                    self.settings_profiles.save_as_input.push(c);
                }
                _ => {}
            }
            return;
        }

        // List navigation mode
        match code {
            KeyCode::Char('j') | KeyCode::Down
                if self.settings_profiles.index
                    < self.settings_profiles.available.len().saturating_sub(1) =>
            {
                self.settings_profiles.index += 1;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.settings_profiles.index = self.settings_profiles.index.saturating_sub(1);
            }
            KeyCode::Enter => {
                // Load the selected profile (stay open for preview)
                if let Some(profile) = self
                    .settings_profiles
                    .available
                    .get(self.settings_profiles.index)
                    .cloned()
                {
                    self.apply_settings_profile_deferred(&profile);
                    self.save_settings();
                    self.status_message = format!("Loaded profile: {}", profile.name);
                }
            }
            KeyCode::Char('s') => {
                // Save over current custom profile (only if custom and settings differ)
                if let Some(profile) = self
                    .settings_profiles
                    .available
                    .get(self.settings_profiles.index)
                {
                    if crate::settings_profile::is_builtin(&profile.name) {
                        return;
                    }
                    if profile.matches_app(self) {
                        return;
                    }
                    let updated = crate::settings_profile::SettingsProfile::from_app(
                        self,
                        profile.name.clone(),
                    );
                    match crate::settings_profile::save_custom_profile(&updated) {
                        Ok(()) => {
                            self.settings_profiles.name = updated.name.clone();
                            self.settings_profiles.available =
                                crate::settings_profile::all_settings_profiles();
                            self.settings_profiles.index = self
                                .settings_profiles
                                .available
                                .iter()
                                .position(|p| p.name == self.settings_profiles.name)
                                .unwrap_or(0);
                            self.save_settings();
                            self.status_message = "Profile saved".to_string();
                        }
                        Err(e) => {
                            self.status_message = format!("Save failed: {e}");
                        }
                    }
                }
            }
            KeyCode::Char('S') => {
                // Save-as: open name input
                let has_changes = !self
                    .settings_profiles
                    .available
                    .iter()
                    .any(|p| p.name == self.settings_profiles.name && p.matches_app(self));
                if has_changes {
                    self.settings_profiles.save_as = true;
                    self.settings_profiles.save_as_input.clear();
                }
            }
            KeyCode::Char('d') => {
                // Delete custom profile
                if let Some(profile) = self
                    .settings_profiles
                    .available
                    .get(self.settings_profiles.index)
                {
                    if crate::settings_profile::is_builtin(&profile.name) {
                        return;
                    }
                    let name = profile.name.clone();
                    match crate::settings_profile::delete_custom_profile(&name) {
                        Ok(()) => {
                            if self.settings_profiles.name == name {
                                self.settings_profiles.name = "Default".to_string();
                            }
                            self.settings_profiles.available =
                                crate::settings_profile::all_settings_profiles();
                            if self.settings_profiles.index
                                >= self.settings_profiles.available.len()
                            {
                                self.settings_profiles.index =
                                    self.settings_profiles.available.len().saturating_sub(1);
                            }
                            self.save_settings();
                            self.status_message = format!("Deleted profile: {name}");
                        }
                        Err(e) => {
                            self.status_message = format!("Delete failed: {e}");
                        }
                    }
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.close_overlay();
                self.fire_deferred_settings_hooks();
            }
            _ => {}
        }
    }

    /// Handle a key press while the theme picker overlay is open.
    pub fn handle_theme_key(&mut self, code: KeyCode) {
        // Theme-specific: space selects, q closes
        let code = match code {
            KeyCode::Char(' ') => KeyCode::Enter,
            KeyCode::Char('q') => KeyCode::Esc,
            other => other,
        };
        match classify_list_key(code, false) {
            ListKeyAction::Down
                if self.theme_picker.index
                    < self.theme_picker.available_themes.len().saturating_sub(1) =>
            {
                self.theme_picker.index += 1;
            }
            ListKeyAction::Up => {
                self.theme_picker.index = self.theme_picker.index.saturating_sub(1);
            }
            ListKeyAction::Select => {
                if let Some(selected) = self
                    .theme_picker
                    .available_themes
                    .get(self.theme_picker.index)
                {
                    self.theme = selected.clone();
                    self.save_settings();
                }
                self.close_overlay();
            }
            ListKeyAction::Close => {
                self.close_overlay();
            }
            _ => {}
        }
    }

    /// Handle a key press while the keybindings overlay is open.
    pub fn handle_keybindings_key(&mut self, code: KeyCode) {
        if self.keybindings_overlay.profile_picker {
            match code {
                KeyCode::Char('j') | KeyCode::Down
                    if self.keybindings_overlay.profile_index
                        < self
                            .keybindings_overlay
                            .available_profiles
                            .len()
                            .saturating_sub(1) =>
                {
                    self.keybindings_overlay.profile_index += 1;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.keybindings_overlay.profile_index =
                        self.keybindings_overlay.profile_index.saturating_sub(1);
                }
                KeyCode::Char(' ') | KeyCode::Enter => {
                    if let Some(name) = self
                        .keybindings_overlay
                        .available_profiles
                        .get(self.keybindings_overlay.profile_index)
                    {
                        let mut kb = keybindings::find_profile(name);
                        let overrides = keybindings::load_overrides();
                        kb.apply_overrides(&overrides);
                        self.keybindings = kb;
                        self.save_settings();
                    }
                    self.keybindings_overlay.profile_picker = false;
                }
                KeyCode::Esc => {
                    self.keybindings_overlay.profile_picker = false;
                }
                _ => {}
            }
            return;
        }

        if let Some((displaced_action, _combo)) = self.keybindings_overlay.conflict.take() {
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    // Accept: the displaced action loses its binding
                    self.status_message = format!(
                        "{} is now unbound",
                        keybindings::action_label(displaced_action)
                    );
                }
                _ => {
                    // Undo the rebind — restore both
                    let (mode, action) =
                        self.keybindings_overlay_item(self.keybindings_overlay.index);
                    if let Some(action) = action {
                        self.keybindings.reset_action(mode, action);
                        self.keybindings.reset_action(mode, displaced_action);
                    }
                    self.status_message.clear();
                }
            }
            return;
        }

        let total = self.keybindings_overlay_total();
        match code {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.keybindings_overlay.index < total.saturating_sub(1) {
                    self.keybindings_overlay.index += 1;
                }
                // Skip section headers
                while self.keybindings_overlay.index < total
                    && self
                        .keybindings_overlay_item(self.keybindings_overlay.index)
                        .1
                        .is_none()
                {
                    self.keybindings_overlay.index += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.keybindings_overlay.index = self.keybindings_overlay.index.saturating_sub(1);
                // Skip section headers (index 0 is the profile row — always selectable)
                while self.keybindings_overlay.index > 0
                    && self
                        .keybindings_overlay_item(self.keybindings_overlay.index)
                        .1
                        .is_none()
                {
                    self.keybindings_overlay.index =
                        self.keybindings_overlay.index.saturating_sub(1);
                }
            }
            KeyCode::Enter => {
                if self.keybindings_overlay.index == 0 {
                    // Profile row → open profile picker
                    self.keybindings_overlay.profile_picker = true;
                    self.keybindings_overlay.profile_index = self
                        .keybindings_overlay
                        .available_profiles
                        .iter()
                        .position(|n| *n == self.keybindings.profile_name)
                        .unwrap_or(0);
                } else {
                    let (_, action) = self.keybindings_overlay_item(self.keybindings_overlay.index);
                    if action.is_some() {
                        self.keybindings_overlay.capturing = true;
                        self.status_message = "Press a key combo...".to_string();
                    }
                }
            }
            KeyCode::Backspace => {
                // Reset to profile default
                let (mode, action) = self.keybindings_overlay_item(self.keybindings_overlay.index);
                if let Some(action) = action {
                    self.keybindings.reset_action(mode, action);
                    self.status_message = format!("Reset {}", keybindings::action_label(action));
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.close_overlay();
                self.save_settings();
            }
            _ => {}
        }
    }

    /// Handle keybinding capture: intercepts ALL keys when capturing a new binding.
    pub fn handle_keybinding_capture(&mut self, modifiers: KeyModifiers, code: KeyCode) {
        if code == KeyCode::Esc && modifiers == KeyModifiers::NONE {
            self.keybindings_overlay.capturing = false;
            self.status_message.clear();
            return;
        }

        let (mode, action) = self.keybindings_overlay_item(self.keybindings_overlay.index);
        let Some(action) = action else {
            self.keybindings_overlay.capturing = false;
            return;
        };

        // Strip SHIFT for Char keys — case is encoded in the character itself
        let modifiers = if matches!(code, KeyCode::Char(_)) {
            modifiers - KeyModifiers::SHIFT
        } else {
            modifiers
        };
        let combo = keybindings::KeyCombo { modifiers, code };
        let displaced = self.keybindings.rebind(mode, action, combo.clone());
        self.keybindings_overlay.capturing = false;

        if let Some(displaced_action) = displaced
            && displaced_action != action
        {
            self.status_message = format!(
                "'{}' was bound to {}. Accept? (y/n)",
                keybindings::format_key_combo(&combo),
                keybindings::action_label(displaced_action)
            );
            self.keybindings_overlay.conflict = Some((displaced_action, combo));
            return;
        }
        self.status_message = format!(
            "{} → {}",
            keybindings::action_label(action),
            keybindings::format_key_combo(&combo)
        );
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
    pub fn refresh_contacts_filter(&mut self) {
        let filter_lower = self.contacts_overlay.filter.to_lowercase();
        let mut contacts: Vec<(String, String)> = self
            .store
            .contact_names
            .iter()
            .filter(|(_, name)| !name.is_empty())
            .filter(|(number, name)| {
                if filter_lower.is_empty() {
                    return true;
                }
                name.to_lowercase().contains(&filter_lower)
                    || number.to_lowercase().contains(&filter_lower)
            })
            .map(|(number, name)| (number.clone(), name.clone()))
            .collect();
        contacts.sort_by_key(|a| a.1.to_lowercase());
        self.contacts_overlay.filtered = contacts;
        list_overlay::clamp_index(
            &mut self.contacts_overlay.index,
            self.contacts_overlay.filtered.len(),
        );
    }

    /// Build the list of available group menu actions (context-dependent).
    pub fn group_menu_items(&self) -> Vec<MenuAction> {
        let is_group = self
            .active_conversation
            .as_ref()
            .and_then(|id| self.store.conversations.get(id))
            .is_some_and(|c| c.is_group);
        if is_group {
            vec![
                MenuAction {
                    label: "Members",
                    key_hint: "m",
                    nerd_icon: "\u{f0849}",
                },
                MenuAction {
                    label: "Add member",
                    key_hint: "a",
                    nerd_icon: "\u{f0234}",
                },
                MenuAction {
                    label: "Remove member",
                    key_hint: "r",
                    nerd_icon: "\u{f0235}",
                },
                MenuAction {
                    label: "Rename",
                    key_hint: "n",
                    nerd_icon: "\u{f03eb}",
                },
                MenuAction {
                    label: "Leave",
                    key_hint: "l",
                    nerd_icon: "\u{f0a79}",
                },
            ]
        } else {
            vec![MenuAction {
                label: "Create group",
                key_hint: "c",
                nerd_icon: "\u{f0234}",
            }]
        }
    }

    /// Build filtered contacts list for the "Add member" picker (excludes existing group members).
    pub fn refresh_group_add_filter(&mut self) {
        let filter_lower = self.group_menu.filter.to_lowercase();
        let existing_members: HashSet<&str> = self
            .active_conversation
            .as_ref()
            .and_then(|id| self.store.groups.get(id))
            .map(|g| g.members.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();
        let mut contacts: Vec<(String, String)> = self
            .store
            .contact_names
            .iter()
            .filter(|(_, name)| !name.is_empty())
            .filter(|(number, _)| !existing_members.contains(number.as_str()))
            .filter(|(number, name)| {
                if filter_lower.is_empty() {
                    return true;
                }
                name.to_lowercase().contains(&filter_lower)
                    || number.to_lowercase().contains(&filter_lower)
            })
            .map(|(number, name)| (number.clone(), name.clone()))
            .collect();
        contacts.sort_by_key(|a| a.1.to_lowercase());
        self.group_menu.filtered = contacts;
        if self.group_menu.filtered.is_empty() {
            self.group_menu.index = 0;
        } else if self.group_menu.index >= self.group_menu.filtered.len() {
            self.group_menu.index = self.group_menu.filtered.len() - 1;
        }
    }

    /// Build filtered member list for the "Remove member" picker (excludes self).
    pub fn refresh_group_remove_filter(&mut self) {
        let filter_lower = self.group_menu.filter.to_lowercase();
        let members: Vec<String> = self
            .active_conversation
            .as_ref()
            .and_then(|id| self.store.groups.get(id))
            .map(|g| g.members.clone())
            .unwrap_or_default();
        let mut result: Vec<(String, String)> = members
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
            .filter(|(phone, name)| {
                if filter_lower.is_empty() {
                    return true;
                }
                name.to_lowercase().contains(&filter_lower)
                    || phone.to_lowercase().contains(&filter_lower)
            })
            .collect();
        result.sort_by_key(|a| a.1.to_lowercase());
        self.group_menu.filtered = result;
        if self.group_menu.filtered.is_empty() {
            self.group_menu.index = 0;
        } else if self.group_menu.index >= self.group_menu.filtered.len() {
            self.group_menu.index = self.group_menu.filtered.len() - 1;
        }
    }

    /// Handle a key press while the group management menu is open.
    pub fn handle_group_menu_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        let state = self.group_menu.state.clone()?;
        match state {
            GroupMenuState::Menu => {
                let items = self.group_menu_items();
                let item_count = items.len();
                match code {
                    KeyCode::Char('j') | KeyCode::Down
                        if self.group_menu.index < item_count.saturating_sub(1) =>
                    {
                        self.group_menu.index += 1;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.group_menu.index = self.group_menu.index.saturating_sub(1);
                    }
                    KeyCode::Enter => {
                        if let Some(action) = items.get(self.group_menu.index) {
                            self.transition_group_menu(action.key_hint);
                        }
                    }
                    KeyCode::Char(c) => {
                        let hint = match c {
                            'm' => "m",
                            'a' => "a",
                            'r' => "r",
                            'n' => "n",
                            'l' => "l",
                            'c' => "c",
                            _ => "",
                        };
                        if !hint.is_empty() && items.iter().any(|a| a.key_hint == hint) {
                            self.transition_group_menu(hint);
                        }
                    }
                    KeyCode::Esc => {
                        self.group_menu.state = None;
                        self.close_overlay();
                    }
                    _ => {}
                }
                None
            }
            GroupMenuState::Members => {
                let member_count = self.group_menu.filtered.len();
                match code {
                    KeyCode::Char('j') | KeyCode::Down
                        if self.group_menu.index < member_count.saturating_sub(1) =>
                    {
                        self.group_menu.index += 1;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.group_menu.index = self.group_menu.index.saturating_sub(1);
                    }
                    KeyCode::Esc => {
                        self.open_overlay(OverlayKind::GroupMenu);
                        self.group_menu.state = Some(GroupMenuState::Menu);
                        self.group_menu.index = 0;
                    }
                    _ => {}
                }
                None
            }
            GroupMenuState::AddMember => {
                match code {
                    KeyCode::Char('j') | KeyCode::Down
                        if !self.group_menu.filtered.is_empty()
                            && self.group_menu.index < self.group_menu.filtered.len() - 1 =>
                    {
                        self.group_menu.index += 1;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.group_menu.index = self.group_menu.index.saturating_sub(1);
                    }
                    KeyCode::Enter => {
                        if let Some((phone, _)) =
                            self.group_menu.filtered.get(self.group_menu.index)
                        {
                            let phone = phone.clone();
                            let group_id = self.active_conversation.clone()?;
                            self.group_menu.state = None;
                            self.close_overlay();
                            self.group_menu.filter.clear();
                            return Some(SendRequest::AddGroupMembers {
                                group_id,
                                members: vec![phone],
                            });
                        }
                    }
                    KeyCode::Esc => {
                        self.open_overlay(OverlayKind::GroupMenu);
                        self.group_menu.state = Some(GroupMenuState::Menu);
                        self.group_menu.index = 0;
                        self.group_menu.filter.clear();
                    }
                    KeyCode::Backspace => {
                        self.group_menu.filter.pop();
                        self.group_menu.index = 0;
                        self.refresh_group_add_filter();
                    }
                    KeyCode::Char(c) if c != 'j' && c != 'k' => {
                        self.group_menu.filter.push(c);
                        self.group_menu.index = 0;
                        self.refresh_group_add_filter();
                    }
                    _ => {}
                }
                None
            }
            GroupMenuState::RemoveMember => {
                match code {
                    KeyCode::Char('j') | KeyCode::Down
                        if !self.group_menu.filtered.is_empty()
                            && self.group_menu.index < self.group_menu.filtered.len() - 1 =>
                    {
                        self.group_menu.index += 1;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.group_menu.index = self.group_menu.index.saturating_sub(1);
                    }
                    KeyCode::Enter => {
                        if let Some((phone, _)) =
                            self.group_menu.filtered.get(self.group_menu.index)
                        {
                            let phone = phone.clone();
                            let group_id = self.active_conversation.clone()?;
                            self.group_menu.state = None;
                            self.close_overlay();
                            self.group_menu.filter.clear();
                            return Some(SendRequest::RemoveGroupMembers {
                                group_id,
                                members: vec![phone],
                            });
                        }
                    }
                    KeyCode::Esc => {
                        self.open_overlay(OverlayKind::GroupMenu);
                        self.group_menu.state = Some(GroupMenuState::Menu);
                        self.group_menu.index = 0;
                        self.group_menu.filter.clear();
                    }
                    KeyCode::Backspace => {
                        self.group_menu.filter.pop();
                        self.group_menu.index = 0;
                        self.refresh_group_remove_filter();
                    }
                    KeyCode::Char(c) if c != 'j' && c != 'k' => {
                        self.group_menu.filter.push(c);
                        self.group_menu.index = 0;
                        self.refresh_group_remove_filter();
                    }
                    _ => {}
                }
                None
            }
            GroupMenuState::Rename => {
                match code {
                    KeyCode::Enter => {
                        let name = self.group_menu.input.trim().to_string();
                        if !name.is_empty() {
                            let group_id = self.active_conversation.clone()?;
                            self.group_menu.state = None;
                            self.close_overlay();
                            self.group_menu.input.clear();
                            return Some(SendRequest::RenameGroup { group_id, name });
                        }
                    }
                    KeyCode::Esc => {
                        self.open_overlay(OverlayKind::GroupMenu);
                        self.group_menu.state = Some(GroupMenuState::Menu);
                        self.group_menu.index = 0;
                        self.group_menu.input.clear();
                    }
                    KeyCode::Backspace => {
                        self.group_menu.input.pop();
                    }
                    KeyCode::Char(c) => {
                        self.group_menu.input.push(c);
                    }
                    _ => {}
                }
                None
            }
            GroupMenuState::Create => {
                match code {
                    KeyCode::Enter => {
                        let name = self.group_menu.input.trim().to_string();
                        if !name.is_empty() {
                            self.group_menu.state = None;
                            self.close_overlay();
                            self.group_menu.input.clear();
                            return Some(SendRequest::CreateGroup { name });
                        }
                    }
                    KeyCode::Esc => {
                        self.group_menu.state = None;
                        self.close_overlay();
                        self.group_menu.input.clear();
                    }
                    KeyCode::Backspace => {
                        self.group_menu.input.pop();
                    }
                    KeyCode::Char(c) => {
                        self.group_menu.input.push(c);
                    }
                    _ => {}
                }
                None
            }
            GroupMenuState::LeaveConfirm => {
                match code {
                    KeyCode::Char('y') => {
                        let group_id = self.active_conversation.clone()?;
                        self.group_menu.state = None;
                        self.close_overlay();
                        return Some(SendRequest::LeaveGroup { group_id });
                    }
                    KeyCode::Char('n') | KeyCode::Esc => {
                        self.open_overlay(OverlayKind::GroupMenu);
                        self.group_menu.state = Some(GroupMenuState::Menu);
                        self.group_menu.index = 0;
                    }
                    _ => {}
                }
                None
            }
        }
    }

    /// Transition from the top-level group menu to a sub-state.
    fn transition_group_menu(&mut self, hint: &str) {
        self.group_menu.index = 0;
        self.group_menu.filter.clear();
        self.group_menu.input.clear();
        match hint {
            "m" => {
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
            "a" => {
                self.refresh_group_add_filter();
                self.open_overlay(OverlayKind::GroupMenu);
                self.group_menu.state = Some(GroupMenuState::AddMember);
            }
            "r" => {
                self.refresh_group_remove_filter();
                self.open_overlay(OverlayKind::GroupMenu);
                self.group_menu.state = Some(GroupMenuState::RemoveMember);
            }
            "n" => {
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
            "l" => {
                self.open_overlay(OverlayKind::GroupMenu);
                self.group_menu.state = Some(GroupMenuState::LeaveConfirm);
            }
            "c" => {
                self.open_overlay(OverlayKind::GroupMenu);
                self.group_menu.state = Some(GroupMenuState::Create);
            }
            _ => {}
        }
    }

    /// Handle a key press while the reaction picker overlay is open.
    fn handle_message_request_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        let conv_id = match self.active_conversation.clone() {
            Some(id) => id,
            None => {
                self.close_overlay();
                return None;
            }
        };
        match code {
            KeyCode::Char('a') => {
                let is_group = self
                    .store
                    .conversations
                    .get(&conv_id)
                    .map(|c| c.is_group)
                    .unwrap_or(false);
                if let Some(conv) = self.store.conversations.get_mut(&conv_id) {
                    conv.accepted = true;
                }
                self.db_warn_visible(self.db.update_accepted(&conv_id, true), "update_accepted");
                self.close_overlay();
                Some(SendRequest::MessageRequestResponse {
                    recipient: conv_id,
                    is_group,
                    response_type: "accept".to_string(),
                })
            }
            KeyCode::Char('d') => {
                self.close_overlay();
                self.delete_active_conversation()
            }
            KeyCode::Esc => {
                self.close_overlay();
                self.active_conversation = None;
                None
            }
            _ => None,
        }
    }

    fn handle_reaction_picker_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        match code {
            KeyCode::Char('h') | KeyCode::Left => {
                self.reactions.picker_index = self.reactions.picker_index.saturating_sub(1);
                None
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if self.reactions.picker_index < QUICK_REACTIONS.len() - 1 {
                    self.reactions.picker_index += 1;
                }
                None
            }
            KeyCode::Char(c @ '1'..='8') => {
                let idx = (c as u8 - b'1') as usize;
                if idx < QUICK_REACTIONS.len() {
                    self.reactions.picker_index = idx;
                    self.close_overlay();
                    self.prepare_reaction_send()
                } else {
                    None
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.close_overlay();
                self.prepare_reaction_send()
            }
            KeyCode::Char('e') | KeyCode::Char('/') => {
                // Open full emoji picker from reaction context
                self.emoji_picker.open(EmojiPickerSource::Reaction, None);
                self.open_overlay(OverlayKind::EmojiPicker);
                None
            }
            KeyCode::Esc => {
                self.close_overlay();
                None
            }
            _ => None,
        }
    }

    /// Build a SendRequest::Reaction from the current picker selection and focused message.
    /// If the user already reacted with the same emoji, removes it instead (toggle behavior).
    fn prepare_reaction_send(&mut self) -> Option<SendRequest> {
        let emoji = QUICK_REACTIONS
            .get(self.reactions.picker_index)?
            .to_string();
        self.prepare_reaction_send_emoji(&emoji)
    }

    /// Build a SendRequest::Reaction for an arbitrary emoji string.
    /// If the user already reacted with the same emoji, removes it instead (toggle behavior).
    fn prepare_reaction_send_emoji(&mut self, emoji: &str) -> Option<SendRequest> {
        let conv_id = self.active_conversation.clone()?;
        let conv = self.store.conversations.get(&conv_id)?;
        let is_group = conv.is_group;

        let index = self
            .scroll
            .focused_index
            .unwrap_or_else(|| conv.messages.len().saturating_sub(1));
        let msg = conv.messages.get(index)?;

        let target_timestamp = msg.timestamp_ms;
        let target_author = if msg.sender == "you" {
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
            .any(|r| r.sender == "you" && r.emoji == emoji);

        // Optimistic local update
        if let Some(conv) = self.store.conversations.get_mut(&conv_id)
            && let Some(msg) = conv.messages.get_mut(index)
        {
            if is_remove {
                msg.reactions
                    .retain(|r| !(r.sender == "you" && r.emoji == emoji));
            } else {
                // One reaction per user — replace or push
                if let Some(existing) = msg.reactions.iter_mut().find(|r| r.sender == "you") {
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
    pub fn action_menu_items(&self) -> Vec<MenuAction> {
        let msg = match self.selected_message() {
            Some(m) => m,
            None => return Vec::new(),
        };
        let mut items = Vec::new();
        if !msg.is_system && !msg.is_deleted {
            items.push(MenuAction {
                label: "Reply",
                key_hint: "q",
                nerd_icon: "\u{f045a}",
            });
        }
        if msg.sender == "you" && !msg.is_system && !msg.is_deleted {
            items.push(MenuAction {
                label: "Edit",
                key_hint: "e",
                nerd_icon: "\u{f03eb}",
            });
        }
        if !msg.is_system {
            items.push(MenuAction {
                label: "React",
                key_hint: "r",
                nerd_icon: "\u{f0785}",
            });
        }
        if !msg.is_system && !msg.is_deleted {
            items.push(MenuAction {
                label: "Forward",
                key_hint: "f",
                nerd_icon: "\u{f04d6}",
            });
        }
        items.push(MenuAction {
            label: "Copy",
            key_hint: "y",
            nerd_icon: "\u{f018f}",
        });
        if !msg.is_system && !msg.is_deleted {
            items.push(MenuAction {
                label: "Delete",
                key_hint: "d",
                nerd_icon: "\u{f0a79}",
            });
        }
        if !msg.is_system && !msg.is_deleted {
            items.push(MenuAction {
                label: if msg.is_pinned { "Unpin" } else { "Pin" },
                key_hint: "p",
                nerd_icon: "\u{f0403}",
            });
        }
        if let Some(ref poll) = msg.poll_data {
            if !poll.closed {
                items.push(MenuAction {
                    label: "Vote",
                    key_hint: "v",
                    nerd_icon: "\u{f0e73}",
                });
            }
            if msg.sender == "you" && !poll.closed {
                items.push(MenuAction {
                    label: "End Poll",
                    key_hint: "x",
                    nerd_icon: "\u{f073a}",
                });
            }
        }
        if !msg.is_deleted {
            if extract_file_uri(&msg.body).is_some() {
                items.push(MenuAction {
                    label: "Open attachment",
                    key_hint: "o",
                    nerd_icon: "\u{f15b5}",
                });
            }
            if extract_http_url(&msg.body).is_some() {
                items.push(MenuAction {
                    label: "Open link",
                    key_hint: "l",
                    nerd_icon: "\u{f0337}",
                });
            }
        }
        items
    }

    /// Handle a key press while the action menu overlay is open.
    pub fn handle_action_menu_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        let item_count = self.action_menu_items().len();
        if item_count == 0 {
            self.close_overlay();
            return None;
        }
        match classify_list_key(code, false) {
            ListKeyAction::Down => {
                if self.action_menu.index < item_count - 1 {
                    self.action_menu.index += 1;
                }
                None
            }
            ListKeyAction::Up => {
                self.action_menu.index = self.action_menu.index.saturating_sub(1);
                None
            }
            ListKeyAction::Select => {
                let items = self.action_menu_items();
                if let Some(action) = items.get(self.action_menu.index) {
                    let hint = action.key_hint;
                    self.close_overlay();
                    self.execute_action_by_hint(hint)
                } else {
                    self.close_overlay();
                    None
                }
            }
            ListKeyAction::Close => {
                self.close_overlay();
                None
            }
            ListKeyAction::None => {
                // Action menu shortcut keys
                if let KeyCode::Char(c) = code {
                    let hint = match c {
                        'q' => "q",
                        'e' => "e",
                        'r' => "r",
                        'f' => "f",
                        'y' => "y",
                        'd' => "d",
                        'p' => "p",
                        'v' => "v",
                        'x' => "x",
                        'o' => "o",
                        'l' => "l",
                        _ => return None,
                    };
                    // Only execute if this action is available in the menu
                    let items = self.action_menu_items();
                    if items.iter().any(|a| a.key_hint == hint) {
                        self.close_overlay();
                        self.execute_action_by_hint(hint)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Execute an action by its key hint character. Reuses the same logic as
    /// the direct Normal-mode keybinds.
    fn execute_action_by_hint(&mut self, hint: &str) -> Option<SendRequest> {
        match hint {
            "q" => {
                // Reply — same as Normal 'q'
                if let Some(msg) = self.selected_message()
                    && !msg.is_system
                    && !msg.is_deleted
                {
                    let author_phone = msg.sender_id.clone();
                    let snippet: String = if msg.body.chars().count() > 50 {
                        format!("{}…", msg.body.chars().take(50).collect::<String>())
                    } else {
                        msg.body.clone()
                    };
                    let ts = msg.timestamp_ms;
                    let phone = if author_phone.is_empty() || author_phone == "you" {
                        self.account.clone()
                    } else {
                        author_phone
                    };
                    self.reply_target = Some((phone, snippet, ts));
                    self.mode = InputMode::Insert;
                }
                None
            }
            "e" => {
                // Edit — same as Normal 'e'
                if let Some(msg) = self.selected_message()
                    && msg.sender == "you"
                    && !msg.is_deleted
                    && !msg.is_system
                {
                    let ts = msg.timestamp_ms;
                    let body = msg.body.clone();
                    if let Some(ref conv_id) = self.active_conversation {
                        let conv_id = conv_id.clone();
                        self.editing_message = Some((ts, conv_id));
                        self.input.buffer = body;
                        self.input.cursor = self.input.buffer.len();
                        self.mode = InputMode::Insert;
                    }
                }
                None
            }
            "r" => {
                // React — open reaction picker
                if self.selected_message().is_some_and(|m| !m.is_system) {
                    self.open_overlay(OverlayKind::ReactionPicker);
                    self.reactions.picker_index = 0;
                }
                None
            }
            "f" => {
                // Forward — open conversation picker
                if let Some(msg) = self.selected_message()
                    && !msg.is_system
                    && !msg.is_deleted
                {
                    self.forward.body = msg.body.clone();
                    self.open_forward_picker();
                }
                None
            }
            "y" => {
                // Copy
                self.copy_selected_message(false);
                None
            }
            "d" => {
                // Delete — open delete confirm
                if let Some(msg) = self.selected_message()
                    && !msg.is_system
                    && !msg.is_deleted
                {
                    self.open_overlay(OverlayKind::DeleteConfirm);
                }
                None
            }
            "p" => {
                // Pin/Unpin
                self.execute_pin_toggle()
            }
            "v" => {
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
                    let poll_author = if msg.sender_id.is_empty() || msg.sender_id == "you" {
                        self.account.clone()
                    } else {
                        msg.sender_id.clone()
                    };
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
            "x" => {
                // End poll
                if let Some(msg) = self.selected_message()
                    && msg.sender == "you"
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
            "o" => {
                // Open attachment
                if let Some(msg) = self.selected_message()
                    && let Some(uri) = extract_file_uri(&msg.body)
                {
                    self.open_file(&uri);
                }
                None
            }
            "l" => {
                // Open link
                if let Some(msg) = self.selected_message()
                    && let Some(url) = extract_http_url(&msg.body)
                {
                    self.open_url(&url);
                }
                None
            }
            _ => None,
        }
    }

    /// Handle a key press while the contacts overlay is open.
    pub fn handle_verify_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        match code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.verify.confirming = false;
                if !self.verify.identities.is_empty()
                    && self.verify.index < self.verify.identities.len() - 1
                {
                    self.verify.index += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.verify.confirming = false;
                if self.verify.index > 0 {
                    self.verify.index -= 1;
                }
            }
            KeyCode::Char('v') | KeyCode::Enter => {
                if let Some(id) = self.verify.identities.get(self.verify.index) {
                    if id.safety_number.is_empty() {
                        self.status_message =
                            "Safety number not available — cannot verify".to_string();
                        return None;
                    }
                    if self.verify.confirming {
                        // Second press: actually trust with the specific safety number
                        if let Some(ref number) = id.number {
                            let recipient = number.clone();
                            let safety_number = id.safety_number.clone();
                            self.verify.confirming = false;
                            return Some(SendRequest::TrustIdentity {
                                recipient,
                                safety_number,
                            });
                        }
                    } else {
                        // First press: ask for confirmation
                        self.verify.confirming = true;
                    }
                }
            }
            KeyCode::Esc => {
                self.verify.confirming = false;
                self.close_overlay();
            }
            _ => {
                self.verify.confirming = false;
            }
        }
        None
    }

    fn open_forward_picker(&mut self) {
        self.open_overlay(OverlayKind::Forward);
        self.forward.index = 0;
        self.forward.filter.clear();
        self.update_forward_filter();
    }

    fn update_forward_filter(&mut self) {
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
        match classify_list_key(code, true) {
            ListKeyAction::Down => {
                if !self.forward.filtered.is_empty()
                    && self.forward.index < self.forward.filtered.len() - 1
                {
                    self.forward.index += 1;
                }
            }
            ListKeyAction::Up => {
                self.forward.index = self.forward.index.saturating_sub(1);
            }
            ListKeyAction::Select => {
                if let Some((conv_id, name)) =
                    self.forward.filtered.get(self.forward.index).cloned()
                {
                    let is_group = self
                        .store
                        .conversations
                        .get(&conv_id)
                        .map(|c| c.is_group)
                        .unwrap_or(false);
                    let body = format!("[Forwarded]\n{}", self.forward.body);
                    let local_ts_ms = chrono::Utc::now().timestamp_millis();
                    self.close_overlay();
                    self.status_message = format!("Forwarded to {name}");
                    self.store.move_conversation_to_top(&conv_id);
                    return Some(SendRequest::Message {
                        recipient: conv_id,
                        body,
                        is_group,
                        local_ts_ms,
                        mentions: Vec::new(),
                        attachment: None,
                        quote_timestamp: None,
                        quote_author: None,
                        quote_body: None,
                    });
                }
            }
            ListKeyAction::Close => {
                self.close_overlay();
            }
            ListKeyAction::FilterPush(c) => {
                if !c.is_control() {
                    self.forward.filter.push(c);
                    self.update_forward_filter();
                }
            }
            ListKeyAction::FilterPop => {
                self.forward.filter.pop();
                self.update_forward_filter();
            }
            ListKeyAction::None => {}
        }
        None
    }

    pub fn handle_contacts_key(&mut self, code: KeyCode) {
        match classify_list_key(code, true) {
            ListKeyAction::Down => {
                if !self.contacts_overlay.filtered.is_empty()
                    && self.contacts_overlay.index < self.contacts_overlay.filtered.len() - 1
                {
                    self.contacts_overlay.index += 1;
                }
            }
            ListKeyAction::Up => {
                self.contacts_overlay.index = self.contacts_overlay.index.saturating_sub(1);
            }
            ListKeyAction::Select => {
                if let Some((number, _)) = self
                    .contacts_overlay
                    .filtered
                    .get(self.contacts_overlay.index)
                {
                    let number = number.clone();
                    self.close_overlay();
                    self.contacts_overlay.filter.clear();
                    self.join_conversation(&number);
                }
            }
            ListKeyAction::Close => {
                self.close_overlay();
                self.contacts_overlay.filter.clear();
            }
            ListKeyAction::FilterPush(c) => {
                self.contacts_overlay.filter.push(c);
                self.refresh_contacts_filter();
            }
            ListKeyAction::FilterPop => {
                self.contacts_overlay.filter.pop();
                self.refresh_contacts_filter();
            }
            ListKeyAction::None => {}
        }
    }

    /// Handle a key press while the search overlay is open.
    pub fn handle_search_key(&mut self, code: KeyCode) {
        let active = self.active_conversation.as_deref().map(str::to_owned);
        let action = self.search.handle_key(code, active.as_deref(), &self.db);
        self.dispatch_search_action(action);
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
    fn jump_to_quote(&mut self) {
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
    fn jump_back(&mut self) {
        if let Some((offset, index)) = self.scroll.jump_stack.pop() {
            self.scroll.offset = offset;
            self.scroll.focused_index = index;
        }
    }

    /// Jump to the next/previous search result in the active conversation.
    fn jump_to_search_result(&mut self, forward: bool) {
        let active = self.active_conversation.as_deref();
        let action = self.search.jump_to_result(forward, active);
        self.dispatch_search_action(action);
    }

    /// Dispatch a `SearchAction` returned by `SearchState` methods.
    fn dispatch_search_action(&mut self, action: SearchAction) {
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
        match self.file_picker.handle_key(code) {
            crate::domain::FilePickerOutcome::Continue => {}
            crate::domain::FilePickerOutcome::Selected(path) => {
                self.pending_attachment = Some(path);
                self.close_overlay();
            }
            crate::domain::FilePickerOutcome::Cancelled => {
                self.close_overlay();
            }
        }
    }

    /// Handle a key press while the autocomplete popup is visible.
    /// Returns `Some(SendRequest)` when the user submits a command
    /// that requires sending a message. Returns `None` otherwise.
    pub fn handle_autocomplete_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        let list_len = self.autocomplete.len();
        match code {
            KeyCode::Up => {
                if list_len > 0 {
                    self.autocomplete.index = if self.autocomplete.index == 0 {
                        list_len - 1
                    } else {
                        self.autocomplete.index - 1
                    };
                }
            }
            KeyCode::Down => {
                if list_len > 0 {
                    self.autocomplete.index = (self.autocomplete.index + 1) % list_len;
                }
            }
            KeyCode::Tab => {
                self.apply_autocomplete();
            }
            KeyCode::Esc => {
                self.autocomplete.clear();
                if self.is_overlay(OverlayKind::Autocomplete) {
                    self.close_overlay();
                }
            }
            KeyCode::Enter => {
                if self.autocomplete.mode == AutocompleteMode::Mention {
                    self.apply_autocomplete();
                    // Don't submit on Enter for mentions — just complete
                } else {
                    // Command and Join: apply + submit
                    self.apply_autocomplete();
                    return self.handle_input();
                }
            }
            _ => {
                self.apply_input_edit(code);
                self.update_autocomplete();
            }
        }
        None
    }

    pub fn new(account: String, db: Database) -> Self {
        let (image_render_tx, image_render_rx) = mpsc::channel();
        Self {
            store: ConversationStore::new(),
            active_conversation: None,
            input: InputState::default(),
            sidebar_visible: true,
            scroll: ScrollState::default(),
            status_message: "connecting...".to_string(),
            should_quit: false,
            quit_confirm: false,
            account,
            sidebar_width: 22,
            sidebar_on_right: false,
            sidebar_filter: String::new(),
            sidebar_filtered: Vec::new(),
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
            settings_index: 0,
            customize_index: 0,
            contacts_overlay: ContactsOverlayState::default(),
            verify: VerifyOverlayState::default(),
            identity_trust: HashMap::new(),
            image: ImageState::new(image_render_tx, image_render_rx),
            prev_active_conversation: None,
            incognito: false,
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
            settings_mouse_snapshot: true,
            sync: SyncState::new(),
            current_overlay: None,
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

            // Promote stale Sending messages to Sent — if they're in the DB, the
            // send completed but the app exited before the RPC response arrived.
            for msg in &mut conv.messages {
                if msg.status == Some(MessageStatus::Sending) {
                    msg.status = Some(MessageStatus::Sent);
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

        // Post-process: promote stale Sending → Sent, resolve image paths
        let mut processed: Vec<DisplayMessage> = new_msgs
            .into_iter()
            .map(|mut msg| {
                if msg.status == Some(MessageStatus::Sending) {
                    msg.status = Some(MessageStatus::Sent);
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
        let query = self.sidebar_filter.to_lowercase();
        self.sidebar_filtered = self
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
    fn clear_sidebar_filter(&mut self) {
        if self.is_overlay(OverlayKind::SidebarFilter) {
            self.close_overlay();
        }
        self.sidebar_filter.clear();
        self.sidebar_filtered.clear();
    }

    /// Handle a key press while sidebar filter is active.
    fn handle_sidebar_filter_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => {
                self.clear_sidebar_filter();
            }
            KeyCode::Enter => {
                // Select the first matching conversation
                let target = if self.sidebar_filtered.is_empty() {
                    None
                } else {
                    Some(self.sidebar_filtered[0].clone())
                };
                self.clear_sidebar_filter();
                if let Some(conv_id) = target {
                    self.join_conversation(&conv_id);
                }
            }
            KeyCode::Char(c) => {
                self.sidebar_filter.push(c);
                self.refresh_sidebar_filter();
            }
            KeyCode::Backspace => {
                self.sidebar_filter.pop();
                if self.sidebar_filter.is_empty() {
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
                show_desktop_notification("siggy", &body, false, None, "full");
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

    /// Queue a read receipt for a single incoming message (when it arrives in the active conversation).
    fn queue_single_read_receipt(&mut self, sender_id: &str, timestamp_ms: i64) {
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
    fn build_typing_request(&self, stop: bool) -> Option<SendRequest> {
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
    /// Sixel caches survive resize: encoding depends on cell_px (from config,
    /// not terminal size) and halfblock dimensions (fixed at max_width=40).
    /// Only screen positions change, which ratatui recomputes automatically.
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

    /// Handle global keys that work in both Normal and Insert mode.
    /// Returns true if the key was consumed.
    pub fn handle_global_key(&mut self, modifiers: KeyModifiers, code: KeyCode) -> bool {
        let action = self
            .keybindings
            .resolve(modifiers, code, BindingMode::Global);
        if self.quit_confirm && !matches!(action, Some(KeyAction::Quit)) {
            self.quit_confirm = false;
            self.update_status();
        }
        match action {
            Some(KeyAction::Quit) => {
                if self.input.buffer.is_empty() || self.quit_confirm {
                    self.should_quit = true;
                } else {
                    self.quit_confirm = true;
                }
                true
            }
            Some(KeyAction::NextConversation) if !self.is_overlay(OverlayKind::Autocomplete) => {
                self.next_conversation();
                true
            }
            Some(KeyAction::PrevConversation) => {
                self.prev_conversation();
                true
            }
            Some(KeyAction::ResizeSidebarLeft) => {
                self.resize_sidebar(-2);
                true
            }
            Some(KeyAction::ResizeSidebarRight) => {
                self.resize_sidebar(2);
                true
            }
            Some(KeyAction::PageScrollUp) => {
                self.sync.user_scrolled = true;
                self.scroll.offset = self.scroll.offset.saturating_add(5);
                self.scroll.focused_index = None;
                true
            }
            Some(KeyAction::PageScrollDown) => {
                self.sync.user_scrolled = true;
                self.scroll.offset = self.scroll.offset.saturating_sub(5);
                self.scroll.focused_index = None;
                true
            }
            Some(KeyAction::SidebarSearch) => {
                self.sidebar_visible = true;
                self.open_overlay(OverlayKind::SidebarFilter);
                self.sidebar_filter.clear();
                self.sidebar_filtered.clear();
                true
            }
            _ => false,
        }
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
        let Some(kind) = self.active_overlay() else {
            return (false, None);
        };
        match kind {
            OverlayKind::SidebarFilter => {
                self.handle_sidebar_filter_key(code);
                (true, None)
            }
            OverlayKind::PollVote => {
                let send = self.handle_poll_vote_key(code);
                (true, send)
            }
            OverlayKind::PinDuration => {
                let send = self.handle_pin_duration_key(code);
                (true, send)
            }
            OverlayKind::ActionMenu => {
                let send = self.handle_action_menu_key(code);
                (true, send)
            }
            OverlayKind::DeleteConfirm => {
                let send = self.handle_delete_confirm_key(code);
                (true, send)
            }
            OverlayKind::DeleteConversationConfirm => {
                let send = self.handle_delete_conversation_confirm_key(code);
                (true, send)
            }
            OverlayKind::FilePicker => {
                self.handle_file_browser_key(code);
                (true, None)
            }
            OverlayKind::EmojiPicker => match self.emoji_picker.handle_key(code) {
                EmojiPickerAction::Select(emoji) => {
                    let source = self.emoji_picker.source;
                    self.emoji_picker.close();
                    match source {
                        EmojiPickerSource::Input => {
                            self.input.buffer.insert_str(self.input.cursor, &emoji);
                            self.input.cursor += emoji.len();
                            (true, None)
                        }
                        EmojiPickerSource::Reaction => {
                            let send = self.prepare_reaction_send_emoji(&emoji);
                            (true, send)
                        }
                    }
                }
                EmojiPickerAction::Close => {
                    let was_reaction = self.emoji_picker.source == EmojiPickerSource::Reaction;
                    self.emoji_picker.close();
                    if was_reaction {
                        self.open_overlay(OverlayKind::ReactionPicker);
                    }
                    (true, None)
                }
                EmojiPickerAction::None => (true, None),
            },
            OverlayKind::ReactionPicker => {
                let send = self.handle_reaction_picker_key(code);
                (true, send)
            }
            OverlayKind::MessageRequest => {
                let send = self.handle_message_request_key(code);
                (true, send)
            }
            OverlayKind::GroupMenu => {
                let send = self.handle_group_menu_key(code);
                (true, send)
            }
            OverlayKind::About => {
                self.close_overlay();
                (true, None)
            }
            OverlayKind::Profile => {
                let send = self.handle_profile_key(code);
                (true, send)
            }
            OverlayKind::Help => {
                self.close_overlay();
                (true, None)
            }
            OverlayKind::Verify => {
                let send = self.handle_verify_key(code);
                (true, send)
            }
            OverlayKind::Forward => {
                let send = self.handle_forward_key(code);
                (true, send)
            }
            OverlayKind::Contacts => {
                self.handle_contacts_key(code);
                (true, None)
            }
            OverlayKind::Search => {
                self.handle_search_key(code);
                (true, None)
            }
            OverlayKind::SettingsProfiles => {
                self.handle_settings_profile_manager_key(code);
                (true, None)
            }
            OverlayKind::ThemePicker => {
                self.handle_theme_key(code);
                (true, None)
            }
            OverlayKind::Keybindings => {
                self.handle_keybindings_key(code);
                (true, None)
            }
            OverlayKind::Customize => {
                self.handle_customize_key(code);
                (true, None)
            }
            OverlayKind::Settings => {
                self.handle_settings_key(code);
                (true, None)
            }
            OverlayKind::Autocomplete => {
                let send = self.handle_autocomplete_key(code);
                (true, send)
            }
        }
    }

    /// Handle Normal mode key. Dispatches to scroll, edit, or action sub-handlers.
    pub fn handle_normal_key(
        &mut self,
        modifiers: KeyModifiers,
        code: KeyCode,
    ) -> Option<SendRequest> {
        // Handle pending prefix key (gg, dd sequences)
        if let Some(prev) = self.pending_normal_key.take() {
            match (prev, code) {
                ('g', KeyCode::Char('g')) => {
                    // gg = scroll to top
                    if let Some(ref id) = self.active_conversation
                        && let Some(conv) = self.store.conversations.get(id)
                    {
                        self.scroll.offset = conv.messages.len();
                    }
                    self.scroll.focused_index = None;
                    return None;
                }
                ('d', KeyCode::Char('d')) => {
                    // dd = delete message
                    if let Some(msg) = self.selected_message()
                        && !msg.is_system
                        && !msg.is_deleted
                    {
                        self.open_overlay(OverlayKind::DeleteConfirm);
                    }
                    return None;
                }
                (_, KeyCode::Esc) => {
                    // Esc cancels pending prefix
                    return None;
                }
                _ => {
                    // Not a valid sequence -- fall through to process this key normally
                }
            }
        }

        match self
            .keybindings
            .resolve(modifiers, code, BindingMode::Normal)
        {
            // Scroll
            Some(KeyAction::ScrollDown) => {
                self.sync.user_scrolled = true;
                self.scroll.offset = self.scroll.offset.saturating_sub(1);
                self.scroll.focused_index = None;
                None
            }
            Some(KeyAction::ScrollUp) => {
                self.sync.user_scrolled = true;
                self.scroll.offset = self.scroll.offset.saturating_add(1);
                self.scroll.focused_index = None;
                None
            }
            Some(KeyAction::FocusNextMessage) => {
                self.sync.user_scrolled = true;
                self.jump_to_adjacent_message(false);
                None
            }
            Some(KeyAction::FocusPrevMessage) => {
                self.sync.user_scrolled = true;
                self.jump_to_adjacent_message(true);
                None
            }
            Some(KeyAction::HalfPageDown) => {
                self.sync.user_scrolled = true;
                self.scroll.offset = self.scroll.offset.saturating_sub(10);
                self.scroll.focused_index = None;
                None
            }
            Some(KeyAction::HalfPageUp) => {
                self.sync.user_scrolled = true;
                self.scroll.offset = self.scroll.offset.saturating_add(10);
                self.scroll.focused_index = None;
                None
            }
            Some(KeyAction::ScrollToBottom) => {
                self.sync.user_scrolled = true;
                self.scroll.offset = 0;
                self.scroll.focused_index = None;
                None
            }
            // Edit/mode-switch
            Some(KeyAction::InsertAtCursor) => {
                self.mode = InputMode::Insert;
                None
            }
            Some(KeyAction::InsertAfterCursor) => {
                self.input.cursor = next_char_pos(&self.input.buffer, self.input.cursor);
                self.mode = InputMode::Insert;
                None
            }
            Some(KeyAction::InsertLineStart) => {
                self.input.cursor = self.current_line_start();
                self.mode = InputMode::Insert;
                None
            }
            Some(KeyAction::InsertLineEnd) => {
                self.input.cursor = self.current_line_end();
                self.mode = InputMode::Insert;
                None
            }
            Some(KeyAction::OpenLineBelow) => {
                let line_end = self.current_line_end();
                self.input.cursor = line_end;
                self.input.buffer.insert(self.input.cursor, '\n');
                self.input.cursor += 1;
                self.mode = InputMode::Insert;
                None
            }
            Some(KeyAction::CursorLeft) => {
                self.input.cursor = prev_char_pos(&self.input.buffer, self.input.cursor);
                None
            }
            Some(KeyAction::CursorRight) => {
                self.input.cursor = next_char_pos(&self.input.buffer, self.input.cursor);
                None
            }
            Some(KeyAction::LineStart) => {
                self.input.cursor = self.current_line_start();
                None
            }
            Some(KeyAction::LineEnd) => {
                self.input.cursor = self.current_line_end();
                None
            }
            Some(KeyAction::WordForward) => {
                let buf = &self.input.buffer;
                let mut pos = self.input.cursor;
                while pos < buf.len() {
                    let c = buf[pos..].chars().next().unwrap();
                    if c.is_whitespace() {
                        break;
                    }
                    pos += c.len_utf8();
                }
                while pos < buf.len() {
                    let c = buf[pos..].chars().next().unwrap();
                    if !c.is_whitespace() {
                        break;
                    }
                    pos += c.len_utf8();
                }
                self.input.cursor = pos;
                None
            }
            Some(KeyAction::WordBack) => {
                let buf = &self.input.buffer;
                let mut pos = self.input.cursor;
                while pos > 0 {
                    let prev = buf[..pos].chars().next_back().unwrap();
                    if !prev.is_whitespace() {
                        break;
                    }
                    pos -= prev.len_utf8();
                }
                while pos > 0 {
                    let prev = buf[..pos].chars().next_back().unwrap();
                    if prev.is_whitespace() {
                        break;
                    }
                    pos -= prev.len_utf8();
                }
                self.input.cursor = pos;
                None
            }
            Some(KeyAction::DeleteChar) => {
                if self.input.cursor < self.input.buffer.len() {
                    self.input.buffer.remove(self.input.cursor);
                    if self.input.cursor > 0 && self.input.cursor >= self.input.buffer.len() {
                        self.input.cursor =
                            prev_char_pos(&self.input.buffer, self.input.buffer.len());
                    }
                }
                None
            }
            Some(KeyAction::DeleteToEnd) => {
                let line_end = self.current_line_end();
                self.input.buffer.drain(self.input.cursor..line_end);
                None
            }
            Some(KeyAction::StartSearch) => {
                self.input.buffer = "/".to_string();
                self.input.cursor = 1;
                self.mode = InputMode::Insert;
                self.update_autocomplete();
                None
            }
            Some(KeyAction::SidebarSearch) => {
                self.sidebar_visible = true;
                self.open_overlay(OverlayKind::SidebarFilter);
                self.sidebar_filter.clear();
                self.sidebar_filtered.clear();
                None
            }
            Some(KeyAction::ClearInput) => {
                if !self.input.buffer.is_empty() {
                    self.input.buffer.clear();
                    self.input.cursor = 0;
                    self.autocomplete.pending_mentions.clear();
                }
                None
            }
            // Actions
            Some(KeyAction::CopyMessage) => {
                self.copy_selected_message(false);
                None
            }
            Some(KeyAction::CopyAllMessages) => {
                self.copy_selected_message(true);
                None
            }
            Some(KeyAction::React) => {
                if self.selected_message().is_some_and(|m| !m.is_system) {
                    self.open_overlay(OverlayKind::ReactionPicker);
                    self.reactions.picker_index = 0;
                }
                None
            }
            Some(KeyAction::Quote) => {
                if let Some(msg) = self.selected_message()
                    && !msg.is_system
                    && !msg.is_deleted
                {
                    let author_phone = msg.sender_id.clone();
                    let snippet: String = if msg.body.chars().count() > 50 {
                        format!("{}…", msg.body.chars().take(50).collect::<String>())
                    } else {
                        msg.body.clone()
                    };
                    let ts = msg.timestamp_ms;
                    let phone = if author_phone.is_empty() || author_phone == "you" {
                        self.account.clone()
                    } else {
                        author_phone
                    };
                    self.reply_target = Some((phone, snippet, ts));
                    self.mode = InputMode::Insert;
                }
                None
            }
            Some(KeyAction::EditMessage) => {
                if let Some(msg) = self.selected_message()
                    && msg.sender == "you"
                    && !msg.is_deleted
                    && !msg.is_system
                {
                    let ts = msg.timestamp_ms;
                    let body = msg.body.clone();
                    if let Some(ref conv_id) = self.active_conversation {
                        let conv_id = conv_id.clone();
                        self.editing_message = Some((ts, conv_id));
                        self.input.buffer = body;
                        self.input.cursor = self.input.buffer.len();
                        self.mode = InputMode::Insert;
                    }
                }
                None
            }
            Some(KeyAction::ForwardMessage) => {
                if let Some(msg) = self.selected_message()
                    && !msg.is_system
                    && !msg.is_deleted
                {
                    self.forward.body = msg.body.clone();
                    self.open_forward_picker();
                }
                None
            }
            Some(KeyAction::NextSearchResult) => {
                if !self.search.results.is_empty() {
                    self.jump_to_search_result(true);
                }
                None
            }
            Some(KeyAction::PrevSearchResult) => {
                if !self.search.results.is_empty() {
                    self.jump_to_search_result(false);
                }
                None
            }
            Some(KeyAction::OpenActionMenu) => {
                if self.selected_message().is_some_and(|m| !m.is_system) {
                    self.open_overlay(OverlayKind::ActionMenu);
                    self.action_menu.index = 0;
                }
                None
            }
            Some(KeyAction::PinMessage) => self.execute_pin_toggle(),
            Some(KeyAction::JumpToQuote) => {
                self.jump_to_quote();
                None
            }
            Some(KeyAction::JumpBack) => {
                self.jump_back();
                None
            }
            _ => {
                // Handle prefix keys that aren't in the binding map
                if let KeyCode::Char(c @ ('g' | 'd')) = code
                    && modifiers.is_empty()
                {
                    self.pending_normal_key = Some(c);
                }
                None
            }
        }
    }

    /// Handle Insert mode key.
    /// Returns `Some(SendRequest)` if a message send or typing indicator should be dispatched.
    pub fn handle_insert_key(
        &mut self,
        modifiers: KeyModifiers,
        code: KeyCode,
    ) -> Option<SendRequest> {
        match self
            .keybindings
            .resolve(modifiers, code, BindingMode::Insert)
        {
            Some(KeyAction::ExitInsert) => {
                self.mode = InputMode::Normal;
                self.pending_normal_key = None; // defensive reset
                self.close_overlay();
                self.reply_target = None;
                self.editing_message = None;
                if self.typing.reset() {
                    return self.build_typing_request(true);
                }
                None
            }
            Some(KeyAction::InsertNewline) => {
                self.input.buffer.insert(self.input.cursor, '\n');
                self.input.cursor += 1;
                self.close_overlay();
                self.typing.last_keypress = Some(Instant::now());
                if !self.typing.sent
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
            Some(KeyAction::SendMessage) => {
                let was_typing = self.typing.reset();
                let result = self.handle_input();
                if result.is_some() {
                    result
                } else if was_typing {
                    self.build_typing_request(true)
                } else {
                    None
                }
            }
            Some(KeyAction::DeleteWordBack) => {
                self.delete_word_back();
                None
            }
            // Actions that alternative profiles (Emacs/Minimal) may bind in Insert mode
            Some(KeyAction::ScrollDown) => {
                self.sync.user_scrolled = true;
                self.scroll.offset = self.scroll.offset.saturating_sub(1);
                self.scroll.focused_index = None;
                None
            }
            Some(KeyAction::ScrollUp) => {
                self.sync.user_scrolled = true;
                self.scroll.offset = self.scroll.offset.saturating_add(1);
                self.scroll.focused_index = None;
                None
            }
            Some(KeyAction::CursorLeft) => {
                self.input.cursor = prev_char_pos(&self.input.buffer, self.input.cursor);
                None
            }
            Some(KeyAction::CursorRight) => {
                self.input.cursor = next_char_pos(&self.input.buffer, self.input.cursor);
                None
            }
            Some(KeyAction::LineStart) => {
                self.input.cursor = self.current_line_start();
                None
            }
            Some(KeyAction::LineEnd) => {
                self.input.cursor = self.current_line_end();
                None
            }
            Some(KeyAction::DeleteChar) => {
                if self.input.cursor < self.input.buffer.len() {
                    self.input.buffer.remove(self.input.cursor);
                }
                None
            }
            Some(KeyAction::DeleteToEnd) => {
                let line_end = self.current_line_end();
                self.input.buffer.drain(self.input.cursor..line_end);
                None
            }
            Some(KeyAction::CopyMessage) => {
                self.copy_selected_message(false);
                None
            }
            Some(KeyAction::CopyAllMessages) => {
                self.copy_selected_message(true);
                None
            }
            Some(KeyAction::React) => {
                if self.selected_message().is_some_and(|m| !m.is_system) {
                    self.open_overlay(OverlayKind::ReactionPicker);
                    self.reactions.picker_index = 0;
                }
                None
            }
            Some(KeyAction::Quote) => {
                if let Some(msg) = self.selected_message()
                    && !msg.is_system
                    && !msg.is_deleted
                {
                    let author_phone = msg.sender_id.clone();
                    let snippet: String = if msg.body.chars().count() > 50 {
                        format!("{}…", msg.body.chars().take(50).collect::<String>())
                    } else {
                        msg.body.clone()
                    };
                    let ts = msg.timestamp_ms;
                    let phone = if author_phone.is_empty() || author_phone == "you" {
                        self.account.clone()
                    } else {
                        author_phone
                    };
                    self.reply_target = Some((phone, snippet, ts));
                }
                None
            }
            Some(KeyAction::EditMessage) => {
                if let Some(msg) = self.selected_message()
                    && msg.sender == "you"
                    && !msg.is_deleted
                    && !msg.is_system
                {
                    let ts = msg.timestamp_ms;
                    let body = msg.body.clone();
                    if let Some(ref conv_id) = self.active_conversation {
                        let conv_id = conv_id.clone();
                        self.editing_message = Some((ts, conv_id));
                        self.input.buffer = body;
                        self.input.cursor = self.input.buffer.len();
                    }
                }
                None
            }
            Some(KeyAction::ForwardMessage) => {
                if let Some(msg) = self.selected_message()
                    && !msg.is_system
                    && !msg.is_deleted
                {
                    self.forward.body = msg.body.clone();
                    self.open_forward_picker();
                }
                None
            }
            Some(KeyAction::DeleteMessage) => {
                if let Some(msg) = self.selected_message()
                    && !msg.is_system
                    && !msg.is_deleted
                {
                    self.open_overlay(OverlayKind::DeleteConfirm);
                }
                None
            }
            Some(KeyAction::NextSearchResult) => {
                if !self.search.results.is_empty() {
                    self.jump_to_search_result(true);
                }
                None
            }
            Some(KeyAction::PrevSearchResult) => {
                if !self.search.results.is_empty() {
                    self.jump_to_search_result(false);
                }
                None
            }
            Some(KeyAction::OpenActionMenu) => {
                if self.selected_message().is_some_and(|m| !m.is_system) {
                    self.open_overlay(OverlayKind::ActionMenu);
                    self.action_menu.index = 0;
                }
                None
            }
            Some(KeyAction::PinMessage) => self.execute_pin_toggle(),
            Some(KeyAction::JumpToQuote) => {
                self.jump_to_quote();
                None
            }
            Some(KeyAction::JumpBack) => {
                self.jump_back();
                None
            }
            _ => {
                let needs_ac_update = matches!(
                    code,
                    KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_)
                );
                self.apply_input_edit(code);
                if needs_ac_update {
                    self.update_autocomplete();
                }
                if matches!(
                    code,
                    KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete
                ) {
                    self.typing.last_keypress = Some(Instant::now());
                    if self.input.buffer.is_empty() && self.typing.sent {
                        self.typing.sent = false;
                        self.typing.last_keypress = None;
                        return self.build_typing_request(true);
                    }
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
                }
                None
            }
        }
    }

    /// Handle an event from signal-cli
    pub fn handle_signal_event(&mut self, event: SignalEvent) {
        match event {
            SignalEvent::MessageReceived(msg) => self.handle_message(msg),
            SignalEvent::ReceiptReceived {
                sender,
                receipt_type,
                timestamps,
            } => {
                self.handle_receipt(&sender, &receipt_type, &timestamps);
            }
            SignalEvent::SendTimestamp { rpc_id, server_ts } => {
                self.handle_send_timestamp(&rpc_id, server_ts);
            }
            SignalEvent::SendFailed { rpc_id } => {
                self.status_message = "send failed".to_string();
                self.handle_send_failed(&rpc_id);
            }
            SignalEvent::TypingIndicator {
                sender,
                sender_name,
                is_typing,
                group_id,
            } => {
                self.store
                    .remember_contact_name(&sender, sender_name.as_deref());
                // Key by group ID for group messages, sender phone for 1:1
                let conv_key = group_id.as_ref().unwrap_or(&sender).clone();
                if is_typing {
                    self.typing
                        .indicators
                        .entry(conv_key)
                        .or_default()
                        .insert(sender.clone(), Instant::now());
                } else if let Some(senders) = self.typing.indicators.get_mut(&conv_key) {
                    senders.remove(&sender);
                    if senders.is_empty() {
                        self.typing.indicators.remove(&conv_key);
                    }
                }
            }
            SignalEvent::ReactionReceived {
                conv_id,
                emoji,
                sender,
                sender_name,
                target_author,
                target_timestamp,
                is_remove,
            } => {
                self.store
                    .remember_contact_name(&sender, sender_name.as_deref());
                self.handle_reaction(
                    &conv_id,
                    &emoji,
                    &sender,
                    &target_author,
                    target_timestamp,
                    is_remove,
                );
            }
            SignalEvent::EditReceived {
                conv_id,
                sender,
                sender_name,
                target_timestamp,
                new_body,
                new_timestamp: _,
                is_outgoing: _,
            } => {
                self.store
                    .remember_contact_name(&sender, sender_name.as_deref());
                self.handle_edit_received(&conv_id, target_timestamp, &new_body);
            }
            SignalEvent::RemoteDeleteReceived {
                conv_id,
                sender: _,
                target_timestamp,
            } => {
                self.handle_remote_delete(&conv_id, target_timestamp);
            }
            SignalEvent::PinReceived {
                conv_id,
                sender,
                sender_name,
                target_author: _,
                target_timestamp,
            } => {
                self.store
                    .remember_contact_name(&sender, sender_name.as_deref());
                self.handle_pin_received(&conv_id, &sender, target_timestamp, true);
            }
            SignalEvent::UnpinReceived {
                conv_id,
                sender,
                sender_name,
                target_author: _,
                target_timestamp,
            } => {
                self.store
                    .remember_contact_name(&sender, sender_name.as_deref());
                self.handle_pin_received(&conv_id, &sender, target_timestamp, false);
            }
            SignalEvent::PollCreated {
                conv_id,
                timestamp,
                poll_data,
            } => {
                self.handle_poll_created(&conv_id, timestamp, poll_data);
            }
            SignalEvent::PollVoteReceived {
                conv_id,
                target_timestamp,
                voter,
                voter_name,
                option_indexes,
                vote_count,
            } => {
                self.store
                    .remember_contact_name(&voter, voter_name.as_deref());
                self.handle_poll_vote(
                    &conv_id,
                    target_timestamp,
                    &voter,
                    voter_name.as_deref(),
                    &option_indexes,
                    vote_count,
                );
            }
            SignalEvent::PollTerminated {
                conv_id,
                target_timestamp,
            } => {
                self.handle_poll_terminated(&conv_id, target_timestamp);
            }
            SignalEvent::SystemMessage {
                conv_id,
                body,
                timestamp,
                timestamp_ms,
            } => {
                self.handle_system_message(&conv_id, &body, timestamp, timestamp_ms);
            }
            SignalEvent::ExpirationTimerChanged {
                conv_id,
                seconds,
                body,
                timestamp,
                timestamp_ms,
            } => {
                // Update conversation timer
                let is_group = self
                    .store
                    .conversations
                    .get(&conv_id)
                    .map(|c| c.is_group)
                    .unwrap_or(false);
                let conv_name = self
                    .store
                    .contact_names
                    .get(&conv_id)
                    .cloned()
                    .unwrap_or_else(|| conv_id.to_string());
                self.store
                    .get_or_create_conversation(&conv_id, &conv_name, is_group, &self.db);
                if let Some(conv) = self.store.conversations.get_mut(&conv_id) {
                    conv.expiration_timer = seconds;
                }
                self.db_warn_visible(
                    self.db.update_expiration_timer(&conv_id, seconds),
                    "update_expiration_timer",
                );
                // Insert system message
                self.handle_system_message(&conv_id, &body, timestamp, timestamp_ms);
            }
            SignalEvent::ReadSyncReceived { read_messages } => {
                self.handle_read_sync(read_messages);
            }
            SignalEvent::ContactList(contacts) => self.handle_contact_list(contacts),
            SignalEvent::GroupList(groups) => self.handle_group_list(groups),
            SignalEvent::IdentityList(identities) => self.handle_identity_list(identities),
            SignalEvent::Error(ref err) => {
                crate::debug_log::logf(format_args!("signal event error: {err}"));
                self.status_message = format!("error: {err}");
            }
        }
    }

    fn handle_message(&mut self, msg: SignalMessage) {
        let conv_id = if let Some(ref gid) = msg.group_id {
            gid.clone()
        } else if msg.is_outgoing {
            // Outgoing 1:1 — conversation is keyed by recipient
            match msg.destination {
                Some(ref dest) => dest.clone(),
                None => return,
            }
        } else {
            msg.source.clone()
        };

        if self.store.move_conversation_to_top(&conv_id)
            && self.is_overlay(OverlayKind::SidebarFilter)
        {
            self.refresh_sidebar_filter();
        }

        // Track sync burst progress
        if self.sync.active {
            self.sync.message_count += 1;
            self.sync.last_message_time = Some(Instant::now());
            self.status_message =
                format!("Syncing... ({} messages received)", self.sync.message_count);
            // Pin the viewport against the message at the bottom of the
            // active conversation BEFORE we append the new sync message,
            // so subsequent renders can hold that message at its original
            // screen position. See #394.
            self.maybe_capture_sync_pin(&conv_id);
        }

        // Store source_name in contact lookup for future resolution (typing indicators, etc.)
        if !msg.is_outgoing {
            self.store
                .remember_contact_name(&msg.source, msg.source_name.as_deref());
            // Populate UUID->name for @mention resolution
            if let (Some(uuid), Some(name)) = (&msg.source_uuid, &msg.source_name)
                && !name.is_empty()
            {
                self.store
                    .uuid_to_name
                    .entry(uuid.clone())
                    .or_insert_with(|| name.clone());
            }
        }

        // Resolve conversation name: prefer message metadata, then contact lookup, then raw ID
        // For groups, source_name is the sender (not the group), so skip it
        let is_group = msg.group_id.is_some();
        let conv_name = msg
            .group_name
            .as_deref()
            .or(if is_group {
                None
            } else {
                msg.source_name.as_deref()
            })
            .unwrap_or_else(|| {
                self.store
                    .contact_names
                    .get(&conv_id)
                    .map(|s| s.as_str())
                    .unwrap_or(&conv_id)
            })
            .to_string();

        let sender_display = if msg.is_outgoing {
            "you".to_string()
        } else {
            msg.source_name
                .clone()
                .or_else(|| self.store.contact_names.get(&msg.source).cloned())
                .unwrap_or_else(|| short_name(&msg.source))
        };

        let sender_id = if msg.is_outgoing {
            self.account.clone()
        } else {
            msg.source.clone()
        };

        // Ensure conversation exists; detect message requests for new 1:1 from unknown senders
        let is_new = !self.store.conversations.contains_key(&conv_id);
        self.store
            .get_or_create_conversation(&conv_id, &conv_name, is_group, &self.db);
        if is_new
            && !msg.is_outgoing
            && !is_group
            && !self.store.contact_names.contains_key(&conv_id)
        {
            if let Some(conv) = self.store.conversations.get_mut(&conv_id) {
                conv.accepted = false;
            }
            self.db_warn_visible(self.db.update_accepted(&conv_id, false), "update_accepted");
        }

        let msg_ts_ms = msg.timestamp.timestamp_millis();
        // Outgoing synced messages already have a server timestamp; incoming messages have no status
        let msg_status = if msg.is_outgoing {
            Some(MessageStatus::Sent)
        } else {
            None
        };

        // Disappearing messages: extract expiration metadata
        let msg_expires_in = msg.expires_in_seconds;
        let msg_expiration_start = if msg_expires_in > 0 {
            // For received messages, start countdown now; for sent sync, use message timestamp
            if msg.is_outgoing {
                msg_ts_ms
            } else {
                Utc::now().timestamp_millis()
            }
        } else {
            0
        };

        // Keep conversation's expiration_timer in sync with incoming messages
        if let Some(conv) = self.store.conversations.get_mut(&conv_id)
            && conv.expiration_timer != msg_expires_in
        {
            conv.expiration_timer = msg_expires_in;
            db_warn(
                self.db.update_expiration_timer(&conv_id, msg_expires_in),
                "update_expiration_timer",
            );
        }

        // Resolve @mentions before the push closure borrows self mutably
        let resolved_body = msg
            .body
            .as_ref()
            .map(|body| self.store.resolve_mentions(body, &msg.mentions));

        // Resolve text styles (UTF-16 → byte offsets, accounting for mention replacements)
        let resolved_styles = resolved_body
            .as_ref()
            .map(|(resolved, _)| {
                self.store
                    .resolve_text_styles(resolved, &msg.text_styles, &msg.mentions)
            })
            .unwrap_or_default();

        // Resolve quote from wire format
        let msg_quote = msg.quote.as_ref().map(|(ts, author_phone, body)| {
            let author_display = self
                .store
                .contact_names
                .get(author_phone)
                .cloned()
                .unwrap_or_else(|| {
                    if *author_phone == self.account {
                        "you".to_string()
                    } else {
                        author_phone.clone()
                    }
                });
            (
                Quote {
                    author: author_display,
                    body: body.clone(),
                    timestamp_ms: *ts,
                    author_id: author_phone.clone(),
                },
                author_phone.clone(),
                body.clone(),
                *ts,
            )
        });
        let display_quote = msg_quote.as_ref().map(|(q, _, _, _)| q.clone());
        let wire_quote_author = msg_quote.as_ref().map(|(_, a, _, _)| a.clone());
        let wire_quote_body = msg_quote.as_ref().map(|(_, _, b, _)| b.clone());
        let wire_quote_ts = msg_quote.as_ref().map(|(_, _, _, t)| *t);

        // Helper: build a DisplayMessage in timestamp order and persist via on_message_added.
        let mut push_msg = |body: String,
                            image_lines: Option<Vec<Line<'static>>>,
                            image_path: Option<String>,
                            mention_ranges: Vec<(usize, usize)>,
                            style_ranges: Vec<(usize, usize, StyleType)>,
                            quote: Option<Quote>,
                            body_raw: Option<String>,
                            mentions: Vec<Mention>| {
            // Check for buffered poll data from a race condition (poll event arrived first)
            let deferred_poll = self
                .poll_vote
                .pending_polls
                .remove(&(conv_id.clone(), msg_ts_ms));
            let display = DisplayMessage {
                sender: sender_display.clone(),
                timestamp: msg.timestamp,
                body,
                is_system: false,
                image_lines,
                image_path,
                status: msg_status,
                timestamp_ms: msg_ts_ms,
                reactions: Vec::new(),
                mention_ranges,
                style_ranges,
                body_raw,
                mentions,
                quote,
                is_edited: false,
                is_deleted: false,
                is_pinned: false,
                sender_id: sender_id.clone(),
                expires_in_seconds: msg_expires_in,
                expiration_start_ms: msg_expiration_start,
                poll_data: deferred_poll,
                poll_votes: Vec::new(),
                preview: None,
                preview_image_lines: None,
                preview_image_path: None,
            };
            self.on_message_added(
                &conv_id,
                display,
                WireQuote {
                    author: wire_quote_author.clone(),
                    body: wire_quote_body.clone(),
                    timestamp: wire_quote_ts,
                },
                true,
            );
        };

        // Add text body (with resolved @mentions and text styles)
        let had_mentions = !msg.mentions.is_empty();
        if let Some((resolved, ranges)) = resolved_body {
            let raw_body_for_msg = if had_mentions { msg.body.clone() } else { None };
            let mentions_for_msg = if had_mentions {
                msg.mentions.clone()
            } else {
                Vec::new()
            };
            push_msg(
                resolved,
                None,
                None,
                ranges,
                resolved_styles,
                display_quote,
                raw_body_for_msg,
                mentions_for_msg,
            );
        }

        // Add attachment notices
        for att in &msg.attachments {
            let label = att.filename.as_deref().unwrap_or(&att.content_type);
            let is_image = matches!(
                att.content_type.as_str(),
                "image/jpeg" | "image/png" | "image/gif" | "image/webp"
            );

            let path_info = att
                .local_path
                .as_deref()
                .map(|p| format!("({})", path_to_file_uri(p)))
                .unwrap_or_default();

            if is_image {
                let rendered = att
                    .local_path
                    .as_deref()
                    .and_then(|p| image_render::render_image(Path::new(p), 40));
                push_msg(
                    format!("[image: {label}]{path_info}"),
                    rendered,
                    att.local_path.clone(),
                    Vec::new(),
                    Vec::new(),
                    None,
                    None,
                    Vec::new(),
                );
            } else {
                push_msg(
                    format!("[attachment: {label}]{path_info}"),
                    None,
                    None,
                    Vec::new(),
                    Vec::new(),
                    None,
                    None,
                    Vec::new(),
                );
            }
        }

        // Persist raw body + mentions so the display body can be re-resolved
        // later when the contact list or group list fills in unknown UUIDs.
        if had_mentions && let Some(ref raw) = msg.body {
            db_warn(
                self.db
                    .upsert_message_mentions(&conv_id, msg_ts_ms, raw, &msg.mentions),
                "upsert_message_mentions",
            );
        }

        // Attach first link preview to the body message (not attachment messages)
        if let Some(preview) = msg.previews.into_iter().next() {
            if let Some(conv) = self.store.conversations.get_mut(&conv_id)
                && let Some(dm) = conv
                    .messages
                    .iter_mut()
                    .rev()
                    .find(|m| m.timestamp_ms == msg_ts_ms && !m.body.starts_with('['))
            {
                let (img_lines, img_path) =
                    if self.image.show_link_previews && self.image.image_mode != "none" {
                        if let Some(ref p) = preview.image_path {
                            (
                                image_render::render_image(Path::new(p), 30),
                                Some(p.clone()),
                            )
                        } else {
                            (None, None)
                        }
                    } else {
                        (None, None)
                    };
                dm.preview = Some(preview.clone());
                dm.preview_image_lines = img_lines;
                dm.preview_image_path = img_path;
            }
            db_warn(
                self.db.upsert_link_preview(&conv_id, msg_ts_ms, &preview),
                "upsert_link_preview",
            );
        }

        let is_active = self
            .active_conversation
            .as_ref()
            .map(|a| a == &conv_id)
            .unwrap_or(false);

        if !is_active && !msg.is_outgoing {
            if let Some(c) = self.store.conversations.get_mut(&conv_id) {
                c.unread += 1;
            }
            let conv_accepted = self
                .store
                .conversations
                .get(&conv_id)
                .map(|c| c.accepted)
                .unwrap_or(true);
            let is_muted = self.is_muted_at(&conv_id, Utc::now());
            let not_muted_or_blocked =
                conv_accepted && !is_muted && !self.blocked_conversations.contains(&conv_id);
            let type_enabled = if is_group {
                self.notifications.notify_group
            } else {
                self.notifications.notify_direct
            };
            if self.sync.active {
                if type_enabled && not_muted_or_blocked {
                    *self
                        .sync
                        .suppressed_notifications
                        .entry(conv_id.clone())
                        .or_insert(0) += 1;
                }
            } else {
                if type_enabled && not_muted_or_blocked {
                    self.notifications.pending_bell = true;
                }
                if self.notifications.desktop_notifications && not_muted_or_blocked {
                    let notif_body = msg.body.as_deref().unwrap_or("");
                    let notif_group = if is_group {
                        self.store
                            .conversations
                            .get(&conv_id)
                            .map(|c| c.name.clone())
                    } else {
                        None
                    };
                    show_desktop_notification(
                        &sender_display,
                        notif_body,
                        is_group,
                        notif_group.as_deref(),
                        &self.notifications.notification_preview,
                    );
                }
            }
        }

        // Viewport stabilization happens render-side via SyncState::pin --
        // see App::maybe_capture_sync_pin and the chat_pane renderer.

        // Active conversation: send read receipt and advance read marker
        let conv_accepted = self
            .store
            .conversations
            .get(&conv_id)
            .map(|c| c.accepted)
            .unwrap_or(true);
        if is_active {
            if !self.sync.active {
                if !msg.is_outgoing
                    && conv_accepted
                    && !self.blocked_conversations.contains(&conv_id)
                {
                    self.queue_single_read_receipt(&sender_id, msg_ts_ms);
                }
                if let Some(conv) = self.store.conversations.get(&conv_id) {
                    self.store
                        .last_read_index
                        .insert(conv_id.clone(), conv.messages.len());
                }
            }
            if let Ok(Some(rowid)) = self.db.last_message_rowid(&conv_id) {
                db_warn(
                    self.db.save_read_marker(&conv_id, rowid),
                    "save_read_marker",
                );
            }
        }
    }

    fn handle_system_message(
        &mut self,
        conv_id: &str,
        body: &str,
        timestamp: DateTime<Utc>,
        timestamp_ms: i64,
    ) {
        let is_group = self
            .store
            .conversations
            .get(conv_id)
            .map(|c| c.is_group)
            .unwrap_or(false);
        let conv_name = self
            .store
            .contact_names
            .get(conv_id)
            .cloned()
            .unwrap_or_else(|| conv_id.to_string());
        self.store
            .get_or_create_conversation(conv_id, &conv_name, is_group, &self.db);
        let msg = DisplayMessage {
            sender: String::new(),
            timestamp,
            body: body.to_string(),
            is_system: true,
            image_lines: None,
            image_path: None,
            status: None,
            timestamp_ms,
            reactions: Vec::new(),
            mention_ranges: Vec::new(),
            style_ranges: Vec::new(),
            body_raw: None,
            mentions: Vec::new(),
            quote: None,
            is_edited: false,
            is_deleted: false,
            is_pinned: false,
            sender_id: String::new(),
            expires_in_seconds: 0,
            expiration_start_ms: 0,
            poll_data: None,
            poll_votes: Vec::new(),
            preview: None,
            preview_image_lines: None,
            preview_image_path: None,
        };
        self.on_message_added(conv_id, msg, WireQuote::default(), true);
    }

    /// Remove expired disappearing messages from memory and DB.
    /// Returns true if any messages were removed (caller should re-render).
    pub fn sweep_expired_messages(&mut self) -> bool {
        if self.expiring_msg_count == 0 {
            return false;
        }

        let now_ms = Utc::now().timestamp_millis();
        let mut removed_count: usize = 0;

        for conv in self.store.conversations.values_mut() {
            let before = conv.messages.len();
            conv.messages.retain(|m| {
                if m.expires_in_seconds > 0 && m.expiration_start_ms > 0 {
                    let expiry = m.expiration_start_ms + m.expires_in_seconds * 1000;
                    expiry >= now_ms
                } else {
                    true
                }
            });
            removed_count += before - conv.messages.len();
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

    fn handle_reaction(
        &mut self,
        conv_id: &str,
        emoji: &str,
        sender: &str,
        target_author: &str,
        target_timestamp: i64,
        is_remove: bool,
    ) {
        // Find the message in memory and update reactions.
        // Pre-resolve names to avoid borrow conflict with self.store.conversations.
        let account = &self.account;
        let target_display = self.store.contact_names.get(target_author).cloned();
        // Resolve sender phone number to display name for rendering
        let is_self = sender == self.account;
        let sender_display = if is_self {
            "you".to_string()
        } else {
            self.store
                .contact_names
                .get(sender)
                .cloned()
                .unwrap_or_else(|| sender.to_string())
        };
        if let Some(conv) = self.store.conversations.get_mut(conv_id) {
            let found = conv.find_msg_idx(target_timestamp).and_then(|idx| {
                let m = &conv.messages[idx];
                let matches = if m.sender == "you" {
                    target_author == account.as_str()
                } else {
                    m.sender == target_author
                        || target_display.as_deref() == Some(m.sender.as_str())
                };
                if matches { Some(idx) } else { None }
            });
            if let Some(msg) = found.map(|idx| &mut conv.messages[idx]) {
                if is_remove {
                    // Match by display name or "you" (for own reactions from other devices)
                    msg.reactions.retain(|r| r.sender != sender_display);
                } else {
                    // One reaction per user — replace or push
                    if let Some(existing) = msg
                        .reactions
                        .iter_mut()
                        .find(|r| r.sender == sender_display)
                    {
                        existing.emoji = emoji.to_string();
                    } else {
                        msg.reactions.push(Reaction {
                            emoji: emoji.to_string(),
                            sender: sender_display,
                        });
                    }
                }
            }
        }

        // Persist to DB regardless of whether message is in memory
        if is_remove {
            self.db_warn_visible(
                self.db
                    .remove_reaction(conv_id, target_timestamp, target_author, sender),
                "remove_reaction",
            );
        } else {
            self.db_warn_visible(
                self.db
                    .upsert_reaction(conv_id, target_timestamp, target_author, sender, emoji),
                "upsert_reaction",
            );
        }
    }

    /// Handle a key press in the delete confirmation overlay.
    /// Returns Some(SendRequest::RemoteDelete) if remote delete is requested.
    pub fn handle_delete_confirm_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        match code {
            KeyCode::Char('y') => {
                self.close_overlay();
                let conv_id = self.active_conversation.clone()?;
                let conv = self.store.conversations.get(&conv_id)?;
                let is_group = conv.is_group;
                let index = self
                    .scroll
                    .focused_index
                    .unwrap_or_else(|| conv.messages.len().saturating_sub(1));
                let msg = conv.messages.get(index)?;
                let is_outgoing = msg.sender == "you";
                let target_timestamp = msg.timestamp_ms;

                // Apply local delete
                let conv = self.store.conversations.get_mut(&conv_id)?;
                let msg = conv.messages.get_mut(index)?;
                msg.is_deleted = true;
                msg.body = "[deleted]".to_string();
                msg.reactions.clear();
                self.db_warn_visible(
                    self.db.mark_message_deleted(&conv_id, target_timestamp),
                    "mark_message_deleted",
                );

                // Send remote delete only for outgoing messages
                if is_outgoing {
                    return Some(SendRequest::RemoteDelete {
                        recipient: conv_id,
                        is_group,
                        target_timestamp,
                    });
                }
                None
            }
            KeyCode::Char('l') => {
                // Local-only delete (for outgoing messages)
                self.close_overlay();
                let conv_id = self.active_conversation.clone()?;
                let conv = self.store.conversations.get(&conv_id)?;
                let index = self
                    .scroll
                    .focused_index
                    .unwrap_or_else(|| conv.messages.len().saturating_sub(1));
                let msg = conv.messages.get(index)?;
                let target_timestamp = msg.timestamp_ms;

                let conv = self.store.conversations.get_mut(&conv_id)?;
                let msg = conv.messages.get_mut(index)?;
                msg.is_deleted = true;
                msg.body = "[deleted]".to_string();
                msg.reactions.clear();
                self.db_warn_visible(
                    self.db.mark_message_deleted(&conv_id, target_timestamp),
                    "mark_message_deleted",
                );
                None
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                self.close_overlay();
                None
            }
            _ => None,
        }
    }

    /// Handle a key press in the delete-conversation confirmation overlay.
    pub fn handle_delete_conversation_confirm_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        match code {
            KeyCode::Char('y') => {
                self.close_overlay();
                self.delete_active_conversation()
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                self.close_overlay();
                None
            }
            _ => None,
        }
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

    fn handle_edit_received(&mut self, conv_id: &str, target_timestamp: i64, new_body: &str) {
        if let Some(conv) = self.store.conversations.get_mut(conv_id)
            && let Some(idx) = conv.find_msg_idx(target_timestamp)
        {
            conv.messages[idx].body = new_body.to_string();
            conv.messages[idx].is_edited = true;
        }
        self.db_warn_visible(
            self.db
                .update_message_body(conv_id, target_timestamp, new_body),
            "update_message_body",
        );
    }

    fn handle_remote_delete(&mut self, conv_id: &str, target_timestamp: i64) {
        if let Some(conv) = self.store.conversations.get_mut(conv_id)
            && let Some(idx) = conv.find_msg_idx(target_timestamp)
        {
            conv.messages[idx].is_deleted = true;
            conv.messages[idx].body = "[deleted]".to_string();
            conv.messages[idx].reactions.clear();
        }
        self.db_warn_visible(
            self.db.mark_message_deleted(conv_id, target_timestamp),
            "mark_message_deleted",
        );
    }

    fn handle_pin_received(
        &mut self,
        conv_id: &str,
        sender: &str,
        target_timestamp: i64,
        pinned: bool,
    ) {
        if let Some(conv) = self.store.conversations.get_mut(conv_id)
            && let Some(idx) = conv.find_msg_idx(target_timestamp)
        {
            conv.messages[idx].is_pinned = pinned;
        }
        self.db_warn_visible(
            self.db
                .set_message_pinned(conv_id, target_timestamp, pinned),
            "set_message_pinned",
        );
        // Insert system message — resolve sender to display name
        let sender_display = if sender == self.account {
            "you".to_string()
        } else {
            self.store
                .contact_names
                .get(sender)
                .cloned()
                .unwrap_or_else(|| sender.to_string())
        };
        let action = if pinned { "pinned" } else { "unpinned" };
        let body = format!("{sender_display} {action} a message");
        let now = Utc::now();
        let now_ms = now.timestamp_millis();
        self.handle_system_message(conv_id, &body, now, now_ms);
    }

    fn handle_poll_created(&mut self, conv_id: &str, timestamp: i64, poll_data: PollData) {
        // The poll arrives as a regular message too — find it and attach poll_data.
        // If the message hasn't arrived yet (race), buffer the poll data so
        // handle_message can attach it when the message arrives.
        if let Some(conv) = self.store.conversations.get_mut(conv_id) {
            if let Some(idx) = conv.find_msg_idx(timestamp) {
                conv.messages[idx].poll_data = Some(poll_data.clone());
            } else {
                self.poll_vote
                    .pending_polls
                    .insert((conv_id.to_string(), timestamp), poll_data.clone());
            }
        }
        self.db_warn_visible(
            self.db.upsert_poll_data(conv_id, timestamp, &poll_data),
            "upsert_poll_data",
        );
    }

    fn handle_poll_vote(
        &mut self,
        conv_id: &str,
        target_timestamp: i64,
        voter: &str,
        voter_name: Option<&str>,
        option_indexes: &[i64],
        vote_count: i64,
    ) {
        if let Some(conv) = self.store.conversations.get_mut(conv_id)
            && let Some(idx) = conv.find_msg_idx(target_timestamp)
        {
            let msg = &mut conv.messages[idx];
            // Upsert vote in memory
            if let Some(existing) = msg.poll_votes.iter_mut().find(|v| v.voter == voter) {
                existing.option_indexes = option_indexes.to_vec();
                existing.vote_count = vote_count;
                existing.voter_name = voter_name.map(|s| s.to_string());
            } else {
                msg.poll_votes.push(PollVote {
                    voter: voter.to_string(),
                    voter_name: voter_name.map(|s| s.to_string()),
                    option_indexes: option_indexes.to_vec(),
                    vote_count,
                });
            }
        }
        self.db_warn_visible(
            self.db.upsert_poll_vote(
                conv_id,
                target_timestamp,
                voter,
                voter_name,
                option_indexes,
                vote_count,
            ),
            "upsert_poll_vote",
        );
    }

    fn handle_poll_terminated(&mut self, conv_id: &str, target_timestamp: i64) {
        if let Some(conv) = self.store.conversations.get_mut(conv_id)
            && let Some(idx) = conv.find_msg_idx(target_timestamp)
            && let Some(ref mut poll) = conv.messages[idx].poll_data
        {
            poll.closed = true;
        }
        self.db_warn_visible(self.db.close_poll(conv_id, target_timestamp), "close_poll");
    }

    fn execute_pin_toggle(&mut self) -> Option<SendRequest> {
        let msg = self.selected_message()?;
        if msg.is_system || msg.is_deleted {
            return None;
        }
        let was_pinned = msg.is_pinned;
        let target_timestamp = msg.timestamp_ms;
        let author_phone = msg.sender_id.clone();
        let conv_id = self.active_conversation.clone()?;
        let is_group = self
            .store
            .conversations
            .get(&conv_id)
            .map(|c| c.is_group)
            .unwrap_or(false);

        let target_author = if author_phone.is_empty() || author_phone == "you" {
            self.account.clone()
        } else {
            author_phone
        };

        if was_pinned {
            // Unpin immediately — no duration needed
            if let Some(conv) = self.store.conversations.get_mut(&conv_id)
                && let Some(idx) = conv.find_msg_idx(target_timestamp)
            {
                conv.messages[idx].is_pinned = false;
            }
            self.db_warn_visible(
                self.db
                    .set_message_pinned(&conv_id, target_timestamp, false),
                "set_message_pinned",
            );
            self.scroll.offset = 0;
            self.scroll.focused_index = None;
            let body = "you unpinned a message";
            let now = Utc::now();
            let now_ms = now.timestamp_millis();
            self.handle_system_message(&conv_id, body, now, now_ms);
            Some(SendRequest::Unpin {
                recipient: conv_id,
                is_group,
                target_author,
                target_timestamp,
            })
        } else {
            // Open pin duration picker
            self.pin_duration.pending = Some(PinPending {
                conv_id,
                is_group,
                target_author,
                target_timestamp,
            });
            self.open_overlay(OverlayKind::PinDuration);
            self.pin_duration.index = 0;
            None
        }
    }

    /// Handle a key press while the pin duration picker overlay is open.
    pub fn handle_pin_duration_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        match classify_list_key(code, false) {
            ListKeyAction::Down => {
                if self.pin_duration.index < PIN_DURATIONS.len() - 1 {
                    self.pin_duration.index += 1;
                }
                None
            }
            ListKeyAction::Up => {
                self.pin_duration.index = self.pin_duration.index.saturating_sub(1);
                None
            }
            ListKeyAction::Select => {
                let duration = PIN_DURATIONS[self.pin_duration.index].0;
                self.close_overlay();
                let pending = self.pin_duration.pending.take()?;

                // Optimistically pin
                if let Some(conv) = self.store.conversations.get_mut(&pending.conv_id)
                    && let Some(idx) = conv.find_msg_idx(pending.target_timestamp)
                {
                    conv.messages[idx].is_pinned = true;
                }
                self.db_warn_visible(
                    self.db
                        .set_message_pinned(&pending.conv_id, pending.target_timestamp, true),
                    "set_message_pinned",
                );
                self.scroll.offset = 0;
                self.scroll.focused_index = None;
                let body = "you pinned a message";
                let now = Utc::now();
                let now_ms = now.timestamp_millis();
                self.handle_system_message(&pending.conv_id, body, now, now_ms);

                Some(SendRequest::Pin {
                    recipient: pending.conv_id,
                    is_group: pending.is_group,
                    target_author: pending.target_author,
                    target_timestamp: pending.target_timestamp,
                    pin_duration: duration,
                })
            }
            ListKeyAction::Close => {
                self.close_overlay();
                self.pin_duration.pending = None;
                None
            }
            _ => None,
        }
    }

    /// Handle keys in the profile editor overlay.
    pub fn handle_profile_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        const FIELD_COUNT: usize = 4;
        const SAVE_INDEX: usize = FIELD_COUNT;

        if self.profile.editing {
            // Editing a field
            match code {
                KeyCode::Esc => {
                    // Cancel edit, discard buffer
                    self.profile.editing = false;
                }
                KeyCode::Enter => {
                    // Confirm edit, write buffer back to field
                    self.profile.fields[self.profile.index] = self.profile.edit_buffer.clone();
                    self.profile.editing = false;
                }
                KeyCode::Backspace => {
                    self.profile.edit_buffer.pop();
                }
                KeyCode::Char(c) => {
                    self.profile.edit_buffer.push(c);
                }
                _ => {}
            }
            return None;
        }

        // Navigation mode
        match code {
            KeyCode::Char('j') | KeyCode::Down if self.profile.index < SAVE_INDEX => {
                self.profile.index += 1;
            }
            KeyCode::Char('k') | KeyCode::Up if self.profile.index > 0 => {
                self.profile.index -= 1;
            }
            KeyCode::Enter => {
                if self.profile.index < FIELD_COUNT {
                    // Start editing the selected field
                    self.profile.editing = true;
                    self.profile.edit_buffer = self.profile.fields[self.profile.index].clone();
                } else {
                    // Save button
                    let [given_name, family_name, about, about_emoji] = self.profile.fields.clone();
                    if given_name.trim().is_empty() {
                        self.status_message = "Given name is required".to_string();
                        return None;
                    }
                    self.close_overlay();
                    return Some(SendRequest::UpdateProfile {
                        given_name,
                        family_name,
                        about,
                        about_emoji,
                    });
                }
            }
            KeyCode::Esc => {
                self.close_overlay();
            }
            _ => {}
        }
        None
    }

    pub fn handle_poll_vote_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        let pending = self.poll_vote.pending.as_ref()?;
        let option_count = pending.options.len();
        match code {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.poll_vote.index < option_count.saturating_sub(1) {
                    self.poll_vote.index += 1;
                }
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.poll_vote.index = self.poll_vote.index.saturating_sub(1);
                None
            }
            KeyCode::Char(' ') => {
                let allow_multiple = pending.allow_multiple;
                if allow_multiple {
                    if let Some(sel) = self.poll_vote.selections.get_mut(self.poll_vote.index) {
                        *sel = !*sel;
                    }
                } else {
                    // Single select: clear all, select current
                    for sel in &mut self.poll_vote.selections {
                        *sel = false;
                    }
                    if let Some(sel) = self.poll_vote.selections.get_mut(self.poll_vote.index) {
                        *sel = true;
                    }
                }
                None
            }
            KeyCode::Enter => {
                let selected: Vec<i64> = self
                    .poll_vote
                    .selections
                    .iter()
                    .enumerate()
                    .filter(|&(_, &sel)| sel)
                    .map(|(i, _)| i as i64)
                    .collect();
                if selected.is_empty() {
                    return None;
                }
                let pending = self.poll_vote.pending.take()?;
                self.close_overlay();

                // Optimistic local vote
                let voter = self.account.clone();
                self.handle_poll_vote(
                    &pending.conv_id,
                    pending.poll_timestamp,
                    &voter,
                    None,
                    &selected,
                    1,
                );

                Some(SendRequest::PollVote {
                    recipient: pending.conv_id,
                    is_group: pending.is_group,
                    poll_author: pending.poll_author,
                    poll_timestamp: pending.poll_timestamp,
                    option_indexes: selected,
                    vote_count: 1,
                })
            }
            KeyCode::Esc => {
                self.close_overlay();
                self.poll_vote.pending = None;
                None
            }
            _ => None,
        }
    }

    fn handle_read_sync(&mut self, read_messages: Vec<(String, i64)>) {
        // Group entries by conversation: for 1:1, the sender phone IS the conv_id.
        // For groups, we need to scan existing conversations to find which group
        // contains a message with that timestamp from that sender.
        let mut max_ts_per_conv: HashMap<String, i64> = HashMap::new();

        for (sender, timestamp) in &read_messages {
            // First try direct match: sender is a 1:1 conversation
            if self.store.conversations.contains_key(sender.as_str()) {
                let entry = max_ts_per_conv.entry(sender.clone()).or_insert(0);
                *entry = (*entry).max(*timestamp);
                continue;
            }
            // Otherwise, scan group conversations for a message matching this timestamp
            let mut found = false;
            for (conv_id, conv) in &self.store.conversations {
                if !conv.is_group {
                    continue;
                }
                if conv.messages.iter().any(|m| m.timestamp_ms == *timestamp) {
                    let entry = max_ts_per_conv.entry(conv_id.clone()).or_insert(0);
                    *entry = (*entry).max(*timestamp);
                    found = true;
                    break;
                }
            }
            if !found {
                crate::debug_log::logf(format_args!(
                    "read_sync: no conversation found for sender={} ts={timestamp}",
                    crate::debug_log::mask_phone(sender)
                ));
            }
        }

        // For each conversation, advance the read marker
        for (conv_id, max_ts) in &max_ts_per_conv {
            let new_read_idx = if let Some(conv) = self.store.conversations.get(conv_id) {
                // partition_point gives the index of the first message with ts > max_ts
                conv.messages.partition_point(|m| m.timestamp_ms <= *max_ts)
            } else {
                continue;
            };

            // Only advance, never retreat
            let current = self
                .store
                .last_read_index
                .get(conv_id)
                .copied()
                .unwrap_or(0);
            if new_read_idx > current {
                self.store
                    .last_read_index
                    .insert(conv_id.clone(), new_read_idx);

                // Recompute unread from remaining messages after the read marker
                if let Some(conv) = self.store.conversations.get_mut(conv_id) {
                    let unread = conv.messages[new_read_idx..]
                        .iter()
                        .filter(|m| !m.is_system && m.status.is_none())
                        .count();
                    conv.unread = unread;
                }

                // Persist to DB
                if let Ok(Some(rowid)) = self.db.max_rowid_up_to_timestamp(conv_id, *max_ts) {
                    db_warn(
                        self.db.save_read_marker(conv_id, rowid),
                        "save_read_marker (read_sync)",
                    );
                }
            }
        }
    }

    fn handle_contact_list(&mut self, contacts: Vec<Contact>) {
        self.loading = false;
        self.startup_status.clear();
        for contact in contacts {
            // Store name in lookup for future message resolution
            if let Some(ref name) = contact.name
                && !name.is_empty()
            {
                self.store
                    .contact_names
                    .insert(contact.number.clone(), name.clone());
            }
            // Build UUID maps for @mention resolution
            if let Some(ref uuid) = contact.uuid {
                if let Some(ref name) = contact.name
                    && !name.is_empty()
                {
                    self.store.uuid_to_name.insert(uuid.clone(), name.clone());
                }
                self.store
                    .number_to_uuid
                    .insert(contact.number.clone(), uuid.clone());
            }
            // Update name on existing conversations only — don't create new ones
            if let Some(conv) = self.store.conversations.get_mut(&contact.number)
                && let Some(ref contact_name) = contact.name
                && !contact_name.is_empty()
                && conv.name != *contact_name
            {
                conv.name = contact_name.clone();
                db_warn(
                    self.db
                        .upsert_conversation(&contact.number, contact_name, false),
                    "upsert_conversation",
                );
            }
        }
        // Auto-accept unaccepted 1:1 conversations whose sender is now a known contact
        let to_accept: Vec<String> = self
            .store
            .conversations
            .iter()
            .filter(|(_, c)| {
                !c.accepted && !c.is_group && self.store.contact_names.contains_key(&c.id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in to_accept {
            if let Some(conv) = self.store.conversations.get_mut(&id) {
                conv.accepted = true;
                db_warn(self.db.update_accepted(&id, true), "update_accepted");
            }
        }

        // Re-resolve reaction senders: DB stores phone numbers but display
        // needs contact names (or "you" for own reactions).
        self.store.resolve_stored_names(&self.account);

        // Re-resolve @mention display bodies: messages that arrived before the
        // contact list may have fallen back to truncated UUIDs. (#283)
        self.store.rebuild_mention_display(&self.db);
    }

    fn handle_group_list(&mut self, groups: Vec<Group>) {
        for group in groups {
            // Store name in lookup for future message resolution
            if !group.name.is_empty() {
                self.store
                    .contact_names
                    .insert(group.id.clone(), group.name.clone());
            }
            // Store UUID↔phone mappings from group members
            for (phone, uuid) in &group.member_uuids {
                self.store
                    .number_to_uuid
                    .entry(phone.clone())
                    .or_insert_with(|| uuid.clone());
            }
            // Populate UUID->name from group members (phone->uuid + phone->name)
            for (phone, uuid) in &group.member_uuids {
                if let Some(name) = self.store.contact_names.get(phone)
                    && !name.is_empty()
                {
                    self.store
                        .uuid_to_name
                        .entry(uuid.clone())
                        .or_insert_with(|| name.clone());
                }
            }
            // Store group for @mention member lookup
            self.store.groups.insert(group.id.clone(), group.clone());
            // Groups are always "active" (you're a member), so create conversations
            let conv =
                self.store
                    .get_or_create_conversation(&group.id, &group.name, true, &self.db);
            if !group.name.is_empty() && conv.name != group.name {
                conv.name = group.name.clone();
                db_warn(
                    self.db.upsert_conversation(&group.id, &group.name, true),
                    "upsert_conversation",
                );
            }
        }
        // Re-resolve reaction senders with any new names from group members.
        self.store.resolve_stored_names(&self.account);

        // Re-resolve @mention display bodies: group member names may now fill
        // in UUIDs that weren't known at message-receipt time. (#283)
        self.store.rebuild_mention_display(&self.db);
    }

    fn handle_identity_list(&mut self, identities: Vec<IdentityInfo>) {
        // Populate the trust level cache
        self.identity_trust.clear();
        for id in &identities {
            if let Some(ref number) = id.number {
                self.identity_trust.insert(number.clone(), id.trust_level);
            }
        }
        // If verify overlay is open, refresh the displayed identities
        if self.is_overlay(OverlayKind::Verify)
            && let Some(ref conv_id) = self.active_conversation
        {
            let conv_id = conv_id.clone();
            let is_group = self
                .store
                .conversations
                .get(&conv_id)
                .map(|c| c.is_group)
                .unwrap_or(false);
            if is_group {
                if let Some(group) = self.store.groups.get(&conv_id) {
                    let members: HashSet<&str> = group.members.iter().map(|s| s.as_str()).collect();
                    self.verify.identities = identities
                        .iter()
                        .filter(|id| {
                            id.number
                                .as_ref()
                                .is_some_and(|n| members.contains(n.as_str()))
                        })
                        .cloned()
                        .collect();
                }
            } else {
                self.verify.identities = identities
                    .iter()
                    .filter(|id| id.number.as_deref() == Some(conv_id.as_str()))
                    .cloned()
                    .collect();
            }
            // Clamp index
            if !self.verify.identities.is_empty()
                && self.verify.index >= self.verify.identities.len()
            {
                self.verify.index = self.verify.identities.len() - 1;
            }
        }
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

    fn handle_send_timestamp(&mut self, rpc_id: &str, server_ts: i64) {
        // Schedule any paste temp file for deletion after the delay (signal-cli has confirmed send)
        if let Some((path, _)) = self.pending_paste_cleanups.remove(rpc_id) {
            self.pending_paste_cleanups.insert(
                rpc_id.to_string(),
                (
                    path,
                    Instant::now() + std::time::Duration::from_secs(PASTE_CLEANUP_DELAY_SECS),
                ),
            );
        }
        if let Some((conv_id, local_ts)) = self.pending.sends.remove(rpc_id) {
            crate::debug_log::logf(format_args!(
                "send confirmed: conv={} local_ts={local_ts} server_ts={server_ts}",
                crate::debug_log::mask_phone(&conv_id)
            ));
            let effective_ts = if server_ts != 0 { server_ts } else { local_ts };
            let mut found = false;
            if let Some(conv) = self.store.conversations.get_mut(&conv_id) {
                // Find the outgoing message with matching local timestamp
                if let Some(idx) = conv
                    .find_msg_idx(local_ts)
                    .filter(|&idx| conv.messages[idx].sender == "you")
                {
                    conv.messages[idx].timestamp_ms = effective_ts;
                    conv.messages[idx].status = Some(MessageStatus::Sent);
                    found = true;
                }
            }
            if found {
                // Update the DB row's timestamp_ms from local → server
                self.db_warn_visible(
                    self.db.update_message_timestamp_ms(
                        &conv_id,
                        local_ts,
                        effective_ts,
                        MessageStatus::Sent.to_i32(),
                    ),
                    "update_message_timestamp_ms",
                );
            }

            // Replay any buffered receipts that may have arrived before this SendTimestamp
            if !self.pending.receipts.is_empty() {
                let receipts = std::mem::take(&mut self.pending.receipts);
                for (sender, receipt_type, timestamps) in receipts {
                    self.handle_receipt(&sender, &receipt_type, &timestamps);
                }
            }
        }
    }

    fn handle_send_failed(&mut self, rpc_id: &str) {
        // Schedule any paste temp file for deletion after the delay (signal-cli has finished with it)
        if let Some((path, _)) = self.pending_paste_cleanups.remove(rpc_id) {
            self.pending_paste_cleanups.insert(
                rpc_id.to_string(),
                (
                    path,
                    Instant::now() + std::time::Duration::from_secs(PASTE_CLEANUP_DELAY_SECS),
                ),
            );
        }
        if let Some((conv_id, local_ts)) = self.pending.sends.remove(rpc_id) {
            let mut found = false;
            if let Some(conv) = self.store.conversations.get_mut(&conv_id)
                && let Some(idx) = conv
                    .find_msg_idx(local_ts)
                    .filter(|&idx| conv.messages[idx].sender == "you")
            {
                conv.messages[idx].status = Some(MessageStatus::Failed);
                found = true;
            }
            if found {
                self.db_warn_visible(
                    self.db.update_message_status(
                        &conv_id,
                        local_ts,
                        MessageStatus::Failed.to_i32(),
                    ),
                    "update_message_status",
                );
            }
        }
    }

    /// Try to upgrade an outgoing message's status in a single conversation.
    /// Returns true if a match was found for `ts`.
    fn try_upgrade_receipt(
        db: &Database,
        conv_id: &str,
        conv: &mut Conversation,
        ts: i64,
        new_status: MessageStatus,
    ) -> bool {
        if let Some(idx) = conv
            .find_msg_idx(ts)
            .filter(|&idx| conv.messages[idx].sender == "you")
        {
            if let Some(current) = conv.messages[idx].status
                && new_status > current
            {
                conv.messages[idx].status = Some(new_status);
                db_warn(
                    db.update_message_status(conv_id, ts, new_status.to_i32()),
                    "update_message_status",
                );
            }
            return true;
        }
        false
    }

    fn handle_receipt(&mut self, sender: &str, receipt_type: &str, timestamps: &[i64]) {
        let receipt_upper = receipt_type.to_uppercase();
        let new_status = match receipt_upper.as_str() {
            "DELIVERY" => MessageStatus::Delivered,
            "READ" => MessageStatus::Read,
            "VIEWED" => MessageStatus::Viewed,
            _ => return,
        };

        let mut matched_any = false;

        // Try matching in the 1:1 conversation keyed by the receipt sender
        let conv_id = sender.to_string();
        if let Some(conv) = self.store.conversations.get_mut(&conv_id) {
            for ts in timestamps {
                if Self::try_upgrade_receipt(&self.db, &conv_id, conv, *ts, new_status) {
                    matched_any = true;
                }
            }
        }

        // If no match in 1:1, scan all conversations (handles group receipts
        // where sender is a member but conv is keyed by group ID)
        if !matched_any {
            for ts in timestamps {
                for (cid, conv) in &mut self.store.conversations {
                    if Self::try_upgrade_receipt(&self.db, cid, conv, *ts, new_status) {
                        matched_any = true;
                        break;
                    }
                }
            }
        }

        // If still no match, the receipt may have arrived before the SendTimestamp
        // that assigns the server timestamp. Buffer it for replay.
        if !matched_any && !timestamps.is_empty() {
            crate::debug_log::logf(format_args!(
                "receipt: buffering {receipt_type} from {} (no matching ts)",
                crate::debug_log::mask_phone(sender)
            ));
            self.pending.receipts.push((
                sender.to_string(),
                receipt_type.to_string(),
                timestamps.to_vec(),
            ));
        } else if matched_any {
            crate::debug_log::logf(format_args!(
                "receipt: {receipt_type} from {} -> {new_status:?}",
                crate::debug_log::mask_phone(sender)
            ));
        }
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
            for (phone, name) in &self.store.contact_names {
                // Skip group IDs (they don't start with '+')
                if !phone.starts_with('+') {
                    continue;
                }
                let display = format!("{name} ({phone})");
                if filter_lower.is_empty()
                    || name.to_lowercase().contains(&filter_lower)
                    || phone.contains(&filter_lower)
                {
                    candidates.push((display, phone.clone()));
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
    fn current_line_start(&self) -> usize {
        self.input.buffer[..self.input.cursor]
            .rfind('\n')
            .map(|p| p + 1)
            .unwrap_or(0)
    }

    /// Returns the byte offset of the end of the current line (before the newline or buffer end).
    fn current_line_end(&self) -> usize {
        self.input.buffer[self.input.cursor..]
            .find('\n')
            .map(|p| self.input.cursor + p)
            .unwrap_or(self.input.buffer.len())
    }

    /// Delete the word before the cursor (Ctrl+W behavior).
    fn delete_word_back(&mut self) {
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

    pub(crate) fn join_conversation(&mut self, target: &str) {
        self.mark_read();
        self.save_scroll_position();
        self.pending_attachment = None;
        self.reset_typing_with_stop();
        self.input.reset_for_conv_switch();
        self.sync.pin = None;
        self.clear_kitty_placements();

        // Try exact match first
        if self.store.conversations.contains_key(target) {
            let read_from = self.store.last_read_index.get(target).copied().unwrap_or(0);
            self.queue_read_receipts_for_conv(target, read_from);
            self.active_conversation = Some(target.to_string());
            if let Some(conv) = self.store.conversations.get_mut(target) {
                conv.unread = 0;
            }
            self.restore_scroll_position(target);

            self.update_status();
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
            let read_from = self.store.last_read_index.get(&id).copied().unwrap_or(0);
            self.queue_read_receipts_for_conv(&id, read_from);
            self.active_conversation = Some(id.clone());
            if let Some(conv) = self.store.conversations.get_mut(&id) {
                conv.unread = 0;
            }
            self.restore_scroll_position(&id);

            self.update_status();
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

    /// Whether the startup spinner should advance this event-loop tick.
    ///
    /// Pauses during the initial sync burst (`sync.active`). The event loop
    /// polls every 50ms, and ticking the spinner unconditionally on every
    /// iteration sets `needs_redraw = true`, which bypasses the 500ms sync
    /// redraw throttle in `drain_events`. The status bar already shows
    /// "Syncing... (N messages received)" during sync, so the spinner adds
    /// no information; pausing it lets the throttle actually take effect.
    pub fn should_tick_spinner(&self) -> bool {
        self.loading && !self.sync.active
    }

    pub fn next_conversation(&mut self) {
        if self.store.conversation_order.is_empty() {
            return;
        }
        self.clear_sidebar_filter();
        self.mark_read();
        self.save_scroll_position();
        self.pending_attachment = None;
        self.reset_typing_with_stop();
        self.input.reset_for_conv_switch();
        self.sync.pin = None;
        self.clear_kitty_placements();
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
        self.mark_read();
        self.save_scroll_position();
        self.pending_attachment = None;
        self.reset_typing_with_stop();
        self.input.reset_for_conv_switch();
        self.sync.pin = None;
        self.clear_kitty_placements();
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
    fn jump_to_adjacent_message(&mut self, older: bool) {
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
        if !self.mouse.enabled {
            return None;
        }

        // When overlays are open, translate scroll to j/k navigation and Esc on outside click
        if self.has_overlay() {
            match event.kind {
                MouseEventKind::ScrollUp => self.handle_overlay_key(KeyCode::Char('k')),
                MouseEventKind::ScrollDown => self.handle_overlay_key(KeyCode::Char('j')),
                _ => (false, None),
            };
            return None;
        }

        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.handle_left_click(event.column, event.row);
            }
            MouseEventKind::ScrollUp
                if is_in_rect(event.column, event.row, self.mouse.messages_area) =>
            {
                self.sync.user_scrolled = true;
                self.scroll.offset = self.scroll.offset.saturating_add(3);
                self.scroll.focused_index = None;
            }
            MouseEventKind::ScrollDown
                if is_in_rect(event.column, event.row, self.mouse.messages_area) =>
            {
                self.sync.user_scrolled = true;
                self.scroll.offset = self.scroll.offset.saturating_sub(3);
                self.scroll.focused_index = None;
            }
            _ => {}
        }
        None
    }

    fn handle_left_click(&mut self, col: u16, row: u16) {
        // 1. Check link regions first (highest priority — links overlay everything)
        for link in &self.image.link_regions {
            if row == link.y && col >= link.x && col < link.x + link.width {
                let url = link.url.clone();
                self.open_url(&url);
                return;
            }
        }

        // 2. Sidebar click — switch conversation
        if let Some(inner) = self.mouse.sidebar_inner
            && is_in_rect(col, row, inner)
        {
            let index = (row - inner.y) as usize;
            let sidebar_list = if self.is_overlay(OverlayKind::SidebarFilter)
                && !self.sidebar_filtered.is_empty()
            {
                &self.sidebar_filtered
            } else {
                &self.store.conversation_order
            };
            if index < sidebar_list.len() {
                let conv_id = sidebar_list[index].clone();
                self.clear_sidebar_filter();
                self.join_conversation(&conv_id);
            }
            return;
        }

        // 3. Input area click — position cursor and enter Insert mode
        if is_in_rect(col, row, self.mouse.input_area) {
            self.mode = InputMode::Insert;
            // Content starts after left border (1) + prefix
            let content_start_col = self.mouse.input_area.x + 1 + self.mouse.input_prefix_len;
            if col >= content_start_col {
                let text_width = (self.mouse.input_area.width.saturating_sub(2)) as usize
                    - self.mouse.input_prefix_len as usize;
                let input_scroll = floor_char_boundary(
                    &self.input.buffer,
                    self.input.cursor.saturating_sub(text_width),
                );
                let target_col = (col - content_start_col) as usize;
                // Walk characters to find the byte offset for the target column
                let mut byte_pos = input_scroll;
                for (col_pos, ch) in self.input.buffer[input_scroll..].chars().enumerate() {
                    if col_pos >= target_col {
                        break;
                    }
                    byte_pos += ch.len_utf8();
                }
                self.input.cursor = byte_pos.min(self.input.buffer.len());
            } else {
                self.input.cursor = 0;
            }
        }
    }

    fn open_url(&mut self, url: &str) {
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
        if !std::path::Path::new(&path).exists() {
            self.status_message = format!("File not found: {path}");
            return;
        }
        let filename = std::path::Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&path);
        match open::that(&path) {
            Ok(()) => self.status_message = format!("Opened {filename}"),
            Err(e) => self.status_message = format!("Failed to open: {e}"),
        }
    }

    /// Export the active conversation's messages to a plain text file.
    pub(crate) fn export_chat_history(&mut self, limit: Option<usize>) {
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

        // Build plain text output
        let mut output = String::new();
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
        let date = chrono::Local::now().format("%Y-%m-%d");
        let filename = format!("siggy-export-{safe_name}-{date}.txt");

        output.push_str(&format!("Chat export: {}\n", conv.name));
        output.push_str(&format!(
            "Exported: {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M")
        ));
        output.push_str(&format!("Messages: {}\n", export_msgs.len()));
        output.push_str(&"-".repeat(60));
        output.push('\n');

        for msg in export_msgs {
            let time = msg
                .timestamp
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M");
            if msg.is_system {
                output.push_str(&format!("[{time}] * {}\n", msg.body));
            } else {
                let prefix = if msg.is_edited { "(edited) " } else { "" };
                output.push_str(&format!("[{time}] <{}> {prefix}{}\n", msg.sender, msg.body));
                if let Some(ref q) = msg.quote {
                    output.push_str(&format!("  > <{}> {}\n", q.author, q.body));
                }
            }
        }

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
fn is_in_rect(col: u16, row: u16, rect: Rect) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}

/// Convert a local file path to a file:/// URI (forward slashes, for terminal Ctrl+Click).
fn path_to_file_uri(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    if normalized.starts_with('/') {
        format!("file://{normalized}")
    } else {
        format!("file:///{normalized}")
    }
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

/// Extract the first `file:///` URI from a message body.
/// Stops at whitespace or `)` to handle `[image: name](file:///path)` format.
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
        use crate::signal::types::{
            Group, LinkPreview, MessageStatus, PollData, PollOption, PollVote, Reaction, StyleType,
        };
        use chrono::{Local, TimeZone};

        let today = base_date;
        // Build timestamps via the local timezone so that format_time() (which
        // converts to Local) always displays the intended hour:minute values,
        // regardless of which timezone the machine is in.
        let ts = |hour: u32, min: u32| -> chrono::DateTime<chrono::Utc> {
            let naive = today
                .and_hms_opt(hour, min, 0)
                .unwrap_or_else(|| today.and_hms_opt(12, 0, 0).unwrap());
            Local
                .from_local_datetime(&naive)
                .single()
                .expect("ambiguous or invalid local time in demo data")
                .with_timezone(&chrono::Utc)
        };

        let dm = |sender: &str, time: chrono::DateTime<Utc>, body: &str| -> DisplayMessage {
            let is_outgoing = sender == "you";
            DisplayMessage {
                sender: sender.to_string(),
                timestamp: time,
                body: body.to_string(),
                is_system: false,
                image_lines: None,
                image_path: None,
                status: if is_outgoing {
                    Some(MessageStatus::Sent)
                } else {
                    None
                },
                timestamp_ms: time.timestamp_millis(),
                reactions: Vec::new(),
                mention_ranges: Vec::new(),
                style_ranges: Vec::new(),
                body_raw: None,
                mentions: Vec::new(),
                quote: None,
                is_edited: false,
                is_deleted: false,
                is_pinned: false,
                sender_id: String::new(),
                expires_in_seconds: 0,
                expiration_start_ms: 0,
                poll_data: None,
                poll_votes: Vec::new(),
                preview: None,
                preview_image_lines: None,
                preview_image_path: None,
            }
        };

        // --- Alice: weekend plans (with quotes, edited msg, link preview, delivery statuses) ---
        let alice_id = "+15550001111".to_string();

        let mut alice_msgs = vec![
            dm("Alice", ts(8, 0), "Good morning! How's your day going?"),
            dm("you", ts(8, 5), "Just getting started, coffee in hand"),
            dm(
                "Alice",
                ts(8, 10),
                "Nice! I've been up since 6, went for a run",
            ),
            dm(
                "you",
                ts(8, 15),
                "Impressive. I can barely get out of bed before 7",
            ),
            dm(
                "Alice",
                ts(8, 20),
                "Ha! It gets easier once you build the habit",
            ),
            dm("you", ts(8, 25), "That's what everyone says..."),
            dm(
                "Alice",
                ts(8, 30),
                "Trust me, after a week it becomes automatic",
            ),
        ];

        // Quote reply: Alice replies to "coffee in hand"
        let mut alice_reply = dm(
            "Alice",
            ts(8, 35),
            "Honestly same, I need my coffee first too",
        );
        alice_reply.quote = Some(Quote {
            author: "you".to_string(),
            body: "Just getting started, coffee in hand".to_string(),
            timestamp_ms: ts(8, 5).timestamp_millis(),
            author_id: String::new(),
        });
        alice_msgs.push(alice_reply);

        alice_msgs.push(dm("you", ts(8, 40), "Are you free this weekend?"));
        alice_msgs.push(dm("Alice", ts(8, 42), "Yeah! What did you have in mind?"));

        // Link preview
        let mut link_msg = dm(
            "Alice",
            ts(8, 45),
            "There's this farmers market: https://localmarket.example.com",
        );
        link_msg.preview = Some(LinkPreview {
            url: "https://localmarket.example.com".to_string(),
            title: Some("Downtown Farmers Market".to_string()),
            description: Some(
                "Fresh produce, artisan goods, and live music every Saturday 8am-1pm".to_string(),
            ),
            image_path: None,
        });
        alice_msgs.push(link_msg);

        alice_msgs.push(dm("you", ts(8, 47), "Oh nice, what time should we go?"));
        alice_msgs.push(dm(
            "Alice",
            ts(8, 48),
            "Opens at 8, but 9 is fine. Less crowded.",
        ));
        alice_msgs.push(dm("you", ts(8, 50), "Perfect, let's do 9"));
        alice_msgs.push(dm("Alice", ts(8, 52), "I'll pick you up at 8:45"));

        // Edited message
        let mut edited_msg = dm(
            "you",
            ts(8, 55),
            "Actually make it 8:30, I want to browse early",
        );
        edited_msg.is_edited = true;
        alice_msgs.push(edited_msg);

        alice_msgs.push(dm("Alice", ts(8, 57), "Even better! See you Saturday"));

        // Varied delivery statuses on outgoing messages
        alice_msgs[1].status = Some(MessageStatus::Read); // "coffee in hand"
        alice_msgs[3].status = Some(MessageStatus::Read); // "barely get out of bed"
        alice_msgs[5].status = Some(MessageStatus::Read); // "what everyone says"
        alice_msgs[8].status = Some(MessageStatus::Delivered); // "are you free"
        alice_msgs[12].status = Some(MessageStatus::Delivered); // "let's do 9"
        alice_msgs[14].status = Some(MessageStatus::Sent); // edited msg

        let alice = Conversation {
            name: "Alice".to_string(),
            id: alice_id.clone(),
            messages: alice_msgs,
            unread: 0,
            is_group: false,
            expiration_timer: 0,
            accepted: true,
        };

        // --- Bob: code review (with styled text) ---
        let bob_id = "+15550002222".to_string();
        let mut bob_styled = dm(
            "Bob",
            ts(10, 5),
            "Can you review my PR? It's the auth refactor",
        );
        // "auth refactor" is bold (bytes 33..47)
        bob_styled.style_ranges = vec![(33, 47, StyleType::Bold)];

        let mut bob_code = dm(
            "Bob",
            ts(10, 8),
            "The key change is in verify_token() — switched from HMAC to Ed25519",
        );
        // "verify_token()" is monospace (bytes 22..36)
        bob_code.style_ranges = vec![(22, 36, StyleType::Monospace)];

        let mut bob_reply = dm(
            "you",
            ts(10, 12),
            "Looks good! Left a few comments on the error handling",
        );
        bob_reply.status = Some(MessageStatus::Read);

        let bob_thanks = dm(
            "Bob",
            ts(10, 15),
            "Thanks! I'll address those. Also the migration is backwards-compatible so no rush on deploy",
        );

        // Quote reply: Bob quotes the review comment
        let mut bob_followup = dm("Bob", ts(10, 20), "Fixed those error handling bits, PTAL");
        bob_followup.quote = Some(Quote {
            author: "you".to_string(),
            body: "Looks good! Left a few comments on the error handling".to_string(),
            timestamp_ms: ts(10, 12).timestamp_millis(),
            author_id: String::new(),
        });

        let mut bob_lgtm = dm("you", ts(10, 25), "LGTM, approved!");
        bob_lgtm.status = Some(MessageStatus::Delivered);

        // Italicize LGTM
        bob_lgtm.style_ranges = vec![(0, 4, StyleType::Bold)];

        let bob = Conversation {
            name: "Bob".to_string(),
            id: bob_id.clone(),
            messages: vec![
                bob_styled,
                bob_code,
                bob_reply,
                bob_thanks,
                bob_followup,
                bob_lgtm,
            ],
            unread: 0,
            is_group: false,
            expiration_timer: 0,
            accepted: true,
        };

        // --- Carol: single unread ---
        let carol_id = "+15550003333".to_string();
        let carol = Conversation {
            name: "Carol".to_string(),
            id: carol_id.clone(),
            messages: vec![dm(
                "Carol",
                ts(11, 45),
                "Did you see the announcement about the office move?",
            )],
            unread: 1,
            is_group: false,
            expiration_timer: 0,
            accepted: true,
        };

        // --- Dave: meetup conversation with disappearing messages ---
        let dave_id = "+15550004444".to_string();
        let mut dave_sys = dm("system", ts(7, 55), "Disappearing messages set to 1 day");
        dave_sys.is_system = true;

        let mut dave_msg1 = dm("Dave", ts(8, 0), "Meetup is at the usual place, 7pm");
        dave_msg1.expires_in_seconds = 86400;
        dave_msg1.expiration_start_ms = ts(8, 0).timestamp_millis();

        let mut dave_msg2 = dm("you", ts(8, 5), "Got it, I'll be there");
        dave_msg2.status = Some(MessageStatus::Read);
        dave_msg2.expires_in_seconds = 86400;
        dave_msg2.expiration_start_ms = ts(8, 5).timestamp_millis();

        let mut dave_msg3 = dm(
            "Dave",
            ts(8, 6),
            "Bring your laptop if you want to hack on stuff",
        );
        dave_msg3.expires_in_seconds = 86400;
        dave_msg3.expiration_start_ms = ts(8, 6).timestamp_millis();

        let dave = Conversation {
            name: "Dave".to_string(),
            id: dave_id.clone(),
            messages: vec![dave_sys, dave_msg1, dave_msg2, dave_msg3],
            unread: 0,
            is_group: false,
            expiration_timer: 86400,
            accepted: true,
        };

        // --- #Rust Devs: group discussion with @mentions, poll, pinned msg ---
        let rust_id = "group_rustdevs".to_string();

        let mut pinned_msg = dm(
            "Alice",
            ts(10, 30),
            "Has anyone tried the new async trait syntax?",
        );
        pinned_msg.is_pinned = true;

        let mut bob_group = dm(
            "Bob",
            ts(10, 32),
            "Yeah, it's so much cleaner than the pin-based approach",
        );
        // "so much cleaner" in italic (bytes 9..24)
        bob_group.style_ranges = vec![(9, 24, StyleType::Italic)];

        let dave_group = dm("Dave", ts(10, 35), "I'm still wrapping my head around it");

        let mut you_group = dm("you", ts(10, 40), "The desugaring docs helped me a lot");
        you_group.status = Some(MessageStatus::Read);

        let mut alice_mention = dm(
            "Alice",
            ts(10, 42),
            "Can you share the link? @Bob might want it too",
        );
        alice_mention.mention_ranges = vec![(24, 28)];

        let mut you_link = dm(
            "you",
            ts(10, 43),
            "Here you go: https://blog.rust-lang.org/async-traits",
        );
        you_link.status = Some(MessageStatus::Delivered);
        you_link.preview = Some(LinkPreview {
            url: "https://blog.rust-lang.org/async-traits".to_string(),
            title: Some("Async Trait Methods in Stable Rust".to_string()),
            description: Some(
                "A deep dive into the stabilization of async fn in traits".to_string(),
            ),
            image_path: None,
        });

        // Poll: "Which async runtime do you prefer?"
        let mut poll_msg = dm("Dave", ts(10, 50), "");
        poll_msg.poll_data = Some(PollData {
            question: "Which async runtime do you prefer?".to_string(),
            options: vec![
                PollOption {
                    id: 0,
                    text: "Tokio".to_string(),
                },
                PollOption {
                    id: 1,
                    text: "async-std".to_string(),
                },
                PollOption {
                    id: 2,
                    text: "smol".to_string(),
                },
            ],
            allow_multiple: false,
            closed: false,
        });
        poll_msg.poll_votes = vec![
            PollVote {
                voter: "+15550001111".to_string(),
                voter_name: Some("Alice".to_string()),
                option_indexes: vec![0],
                vote_count: 1,
            },
            PollVote {
                voter: "+15550002222".to_string(),
                voter_name: Some("Bob".to_string()),
                option_indexes: vec![0],
                vote_count: 1,
            },
            PollVote {
                voter: "+15550004444".to_string(),
                voter_name: Some("Dave".to_string()),
                option_indexes: vec![2],
                vote_count: 1,
            },
            PollVote {
                voter: "you".to_string(),
                voter_name: Some("you".to_string()),
                option_indexes: vec![0],
                vote_count: 1,
            },
        ];

        let rust_group = Conversation {
            name: "#Rust Devs".to_string(),
            id: rust_id.clone(),
            messages: vec![
                pinned_msg,
                bob_group,
                dave_group,
                you_group,
                alice_mention,
                you_link,
                poll_msg,
            ],
            unread: 0,
            is_group: true,
            expiration_timer: 0,
            accepted: true,
        };

        // --- #Family: group with unread and quote reply ---
        let family_id = "group_family".to_string();
        let mom_id = "+15550005555".to_string();
        let dad_id = "+15550006666".to_string();

        let mom_dinner = dm("Mom", ts(12, 0), "Dinner at our place Sunday?");
        let dad_grill = dm("Dad", ts(12, 5), "I'll fire up the grill");

        let mut you_family = dm("you", ts(12, 10), "Count me in!");
        you_family.status = Some(MessageStatus::Read);

        let mom_dessert = dm("Mom", ts(13, 30), "Great! Bring dessert if you can");
        // Quote reply to "I'll fire up the grill"
        let mut dad_reply = dm("Dad", ts(13, 35), "Got the burgers and corn ready");
        dad_reply.quote = Some(Quote {
            author: "Dad".to_string(),
            body: "I'll fire up the grill".to_string(),
            timestamp_ms: ts(12, 5).timestamp_millis(),
            author_id: dad_id.clone(),
        });

        let family_group = Conversation {
            name: "#Family".to_string(),
            id: family_id.clone(),
            messages: vec![mom_dinner, dad_grill, you_family, mom_dessert, dad_reply],
            unread: 2,
            is_group: true,
            expiration_timer: 0,
            accepted: true,
        };

        // --- Eve: message request (unknown sender) ---
        let eve_id = "+15550007777".to_string();
        let eve = Conversation {
            name: "+15550007777".to_string(),
            id: eve_id.clone(),
            messages: vec![dm(
                "+15550007777",
                ts(14, 0),
                "Hey, I got your number from the meetup. Is this the right person?",
            )],
            unread: 1,
            is_group: false,
            expiration_timer: 0,
            accepted: false,
        };

        // Insert conversations and set ordering
        let order = vec![
            eve_id.clone(),
            family_id.clone(),
            carol_id.clone(),
            rust_id.clone(),
            bob_id.clone(),
            alice_id.clone(),
            dave_id.clone(),
        ];

        for conv in [alice, bob, carol, dave, rust_group, family_group, eve] {
            let id = conv.id.clone();
            let msg_count = conv.messages.len();
            let unread = conv.unread;
            self.store.conversations.insert(id.clone(), conv);
            if msg_count > 0 {
                self.store
                    .last_read_index
                    .insert(id, msg_count.saturating_sub(unread));
            }
        }

        self.store.conversation_order = order;
        self.active_conversation = Some(alice_id.clone());
        self.status_message = "connected | demo mode".to_string();

        // Populate contact names and UUID maps for @mention autocomplete
        let demo_contacts: Vec<(&str, &str, &str)> = vec![
            (&alice_id, "Alice", "aaaa-alice-uuid"),
            (&bob_id, "Bob", "bbbb-bob-uuid"),
            (&carol_id, "Carol", "cccc-carol-uuid"),
            (&dave_id, "Dave", "dddd-dave-uuid"),
            (&mom_id, "Mom", "eeee-mom-uuid"),
            (&dad_id, "Dad", "ffff-dad-uuid"),
        ];
        for (phone, name, uuid) in &demo_contacts {
            self.store
                .contact_names
                .insert(phone.to_string(), name.to_string());
            self.store
                .uuid_to_name
                .insert(uuid.to_string(), name.to_string());
            self.store
                .number_to_uuid
                .insert(phone.to_string(), uuid.to_string());
        }

        // Populate groups with correct members
        self.store.groups.insert(
            rust_id.clone(),
            Group {
                id: rust_id,
                name: "#Rust Devs".to_string(),
                members: vec![alice_id.clone(), bob_id.clone(), dave_id.clone()],
                member_uuids: vec![],
            },
        );
        self.store.groups.insert(
            family_id.clone(),
            Group {
                id: family_id,
                name: "#Family".to_string(),
                members: vec![mom_id, dad_id],
                member_uuids: vec![],
            },
        );

        // Add sample reactions
        if let Some(conv) = self.store.conversations.get_mut(&alice_id) {
            // Alice's first message gets a thumbs up from "you"
            if let Some(msg) = conv.messages.get_mut(0) {
                msg.reactions.push(Reaction {
                    emoji: "\u{1f44d}".to_string(),
                    sender: "you".to_string(),
                });
            }
            // "coffee in hand" gets a heart from Alice
            if let Some(msg) = conv.messages.get_mut(1) {
                msg.reactions.push(Reaction {
                    emoji: "\u{2764}\u{fe0f}".to_string(),
                    sender: "Alice".to_string(),
                });
            }
            // "See you Saturday" gets multiple reactions
            if let Some(msg) = conv.messages.last_mut() {
                msg.reactions.push(Reaction {
                    emoji: "\u{1f389}".to_string(),
                    sender: "you".to_string(),
                });
            }
        }
        if let Some(conv) = self.store.conversations.get_mut("group_rustdevs") {
            // "desugaring docs" message gets multiple reactions
            if let Some(msg) = conv.messages.get_mut(3) {
                msg.reactions.push(Reaction {
                    emoji: "\u{1f44d}".to_string(),
                    sender: "Alice".to_string(),
                });
                msg.reactions.push(Reaction {
                    emoji: "\u{1f44d}".to_string(),
                    sender: "Bob".to_string(),
                });
                msg.reactions.push(Reaction {
                    emoji: "\u{2764}\u{fe0f}".to_string(),
                    sender: "Dave".to_string(),
                });
            }
            // Pinned msg gets a pushpin reaction
            if let Some(msg) = conv.messages.get_mut(0) {
                msg.reactions.push(Reaction {
                    emoji: "\u{1f4cc}".to_string(),
                    sender: "Dave".to_string(),
                });
            }
        }
        if let Some(conv) = self.store.conversations.get_mut("group_family") {
            // "Count me in!" gets hearts from both parents
            if let Some(msg) = conv.messages.get_mut(2) {
                msg.reactions.push(Reaction {
                    emoji: "\u{2764}\u{fe0f}".to_string(),
                    sender: "Mom".to_string(),
                });
                msg.reactions.push(Reaction {
                    emoji: "\u{2764}\u{fe0f}".to_string(),
                    sender: "Dad".to_string(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::signal::types::{
        Attachment, Contact, Group, Mention, SignalEvent, SignalMessage, StyleType, TextStyle,
    };
    use crossterm::event::{KeyCode, KeyModifiers};
    use rstest::{fixture, rstest};

    #[fixture]
    fn app() -> App {
        let db = Database::open_in_memory().unwrap();
        let mut app = App::new("+10000000000".to_string(), db);
        app.set_connected();
        app
    }

    // --- Contacts/groups only populate the name lookup, not the sidebar ---

    #[rstest]
    fn contact_list_does_not_create_conversations(mut app: App) {
        assert!(app.store.conversations.is_empty());

        app.handle_signal_event(SignalEvent::ContactList(vec![
            Contact {
                number: "+1".to_string(),
                name: Some("Alice".to_string()),
                uuid: None,
            },
            Contact {
                number: "+2".to_string(),
                name: Some("Bob".to_string()),
                uuid: None,
            },
        ]));

        // No conversations created — only name lookup populated
        assert!(app.store.conversations.is_empty());
        assert!(app.store.conversation_order.is_empty());
        assert_eq!(app.store.contact_names["+1"], "Alice");
        assert_eq!(app.store.contact_names["+2"], "Bob");
    }

    #[rstest]
    fn group_list_creates_conversations(mut app: App) {
        app.handle_signal_event(SignalEvent::GroupList(vec![
            Group {
                id: "g1".to_string(),
                name: "Family".to_string(),
                members: vec![],
                member_uuids: vec![],
            },
            Group {
                id: "g2".to_string(),
                name: "Work".to_string(),
                members: vec![],
                member_uuids: vec![],
            },
        ]));

        // Groups always create conversations (you're a member)
        assert_eq!(app.store.conversations.len(), 2);
        assert_eq!(app.store.conversations["g1"].name, "Family");
        assert_eq!(app.store.conversations["g2"].name, "Work");
        assert!(app.store.conversations["g1"].is_group);
        assert_eq!(app.store.contact_names["g1"], "Family");
    }

    // --- Contact names enrich existing conversations ---

    #[rstest]
    fn contact_name_updates_existing_conversation(mut app: App) {
        // A message arrives first with just a phone number
        let msg = SignalMessage {
            source: "+15551234567".to_string(),
            source_name: None,
            source_uuid: None,
            timestamp: chrono::Utc::now(),
            body: Some("hey".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
            mentions: vec![],
            text_styles: vec![],
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert_eq!(app.store.conversations["+15551234567"].name, "+15551234567");

        // Contact list arrives with a proper name — updates existing conv
        app.handle_signal_event(SignalEvent::ContactList(vec![Contact {
            number: "+15551234567".to_string(),
            name: Some("Alice".to_string()),
            uuid: None,
        }]));

        assert_eq!(app.store.conversations["+15551234567"].name, "Alice");
    }

    #[rstest]
    fn contact_without_name_does_not_overwrite_existing_name(mut app: App) {
        // Create conversation with a name already
        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: Some("Alice".to_string()),
            source_uuid: None,
            timestamp: chrono::Utc::now(),
            body: Some("hi".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
            mentions: vec![],
            text_styles: vec![],
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert_eq!(app.store.conversations["+1"].name, "Alice");

        // Contact arrives with no name — should NOT overwrite
        app.handle_signal_event(SignalEvent::ContactList(vec![Contact {
            number: "+1".to_string(),
            name: None,
            uuid: None,
        }]));

        assert_eq!(app.store.conversations["+1"].name, "Alice");
    }

    // --- Name lookup used when creating conversations from messages ---

    #[rstest]
    fn message_uses_contact_name_lookup(mut app: App) {
        // Contacts loaded first (no conversations created)
        app.handle_signal_event(SignalEvent::ContactList(vec![Contact {
            number: "+1".to_string(),
            name: Some("Alice".to_string()),
            uuid: None,
        }]));
        assert!(app.store.conversations.is_empty());

        // Message arrives with no source_name — should use lookup
        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: None,
            source_uuid: None,
            timestamp: chrono::Utc::now(),
            body: Some("hello!".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
            mentions: vec![],
            text_styles: vec![],
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        assert_eq!(app.store.conversations.len(), 1);
        assert_eq!(app.store.conversations["+1"].name, "Alice");
        assert_eq!(app.store.conversations["+1"].messages[0].sender, "Alice");
    }

    #[rstest]
    fn message_in_known_group_uses_name_lookup(mut app: App) {
        // Groups loaded — conversation created
        app.handle_signal_event(SignalEvent::GroupList(vec![Group {
            id: "g1".to_string(),
            name: "Family".to_string(),
            members: vec![],
            member_uuids: vec![],
        }]));
        assert_eq!(app.store.conversations.len(), 1);

        // Message arrives in that group (no group_name in metadata)
        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: Some("Alice".to_string()),
            source_uuid: None,
            timestamp: chrono::Utc::now(),
            body: Some("hey family".to_string()),
            attachments: vec![],
            group_id: Some("g1".to_string()),
            group_name: None,
            is_outgoing: false,
            destination: None,
            mentions: vec![],
            text_styles: vec![],
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        // Still 1 conversation, name preserved from group list
        assert_eq!(app.store.conversations.len(), 1);
        assert_eq!(app.store.conversations["g1"].name, "Family");
        assert_eq!(app.store.conversations["g1"].messages.len(), 1);
    }

    // --- No duplicate conversations ---

    #[rstest]
    fn no_duplicate_on_repeated_messages(mut app: App) {
        app.handle_signal_event(SignalEvent::ContactList(vec![Contact {
            number: "+1".to_string(),
            name: Some("Alice".to_string()),
            uuid: None,
        }]));

        for _ in 0..3 {
            let msg = SignalMessage {
                source: "+1".to_string(),
                source_name: Some("Alice".to_string()),
                source_uuid: None,
                timestamp: chrono::Utc::now(),
                body: Some("msg".to_string()),
                attachments: vec![],
                group_id: None,
                group_name: None,
                is_outgoing: false,
                destination: None,
                mentions: vec![],
                text_styles: vec![],
                quote: None,
                expires_in_seconds: 0,
                previews: Vec::new(),
            };
            app.handle_signal_event(SignalEvent::MessageReceived(msg));
        }

        assert_eq!(app.store.conversations.len(), 1);
        assert_eq!(app.store.conversation_order.len(), 1);
        assert_eq!(app.store.conversations["+1"].messages.len(), 3);
    }

    // --- Autocomplete tests ---

    #[rstest]
    #[case("/", true, None)]
    #[case("/jo", true, Some(1))]
    #[case("hello", false, Some(0))]
    #[case("/join ", false, None)]
    #[case("/zzz", false, Some(0))]
    fn autocomplete_visibility(
        mut app: App,
        #[case] input: &str,
        #[case] expected_visible: bool,
        #[case] expected_count: Option<usize>,
    ) {
        app.input.buffer = input.to_string();
        app.update_autocomplete();
        assert_eq!(
            app.is_overlay(OverlayKind::Autocomplete),
            expected_visible,
            "visibility for {input:?}"
        );
        if let Some(count) = expected_count {
            assert_eq!(
                app.autocomplete.command_candidates.len(),
                count,
                "count for {input:?}"
            );
        }
    }

    #[rstest]
    fn apply_autocomplete_trailing_space_for_arg_command(mut app: App) {
        app.input.buffer = "/jo".to_string();
        app.update_autocomplete();
        app.apply_autocomplete();
        // /join takes args, so buffer should end with a space
        assert_eq!(app.input.buffer, "/join ");
        assert_eq!(app.input.cursor, 6);
    }

    #[rstest]
    fn apply_autocomplete_no_space_for_no_arg_command(mut app: App) {
        app.input.buffer = "/pa".to_string();
        app.update_autocomplete();
        app.apply_autocomplete();
        // /part takes no args, no trailing space
        assert_eq!(app.input.buffer, "/part");
        assert_eq!(app.input.cursor, 5);
    }

    #[rstest]
    fn apply_autocomplete_index_clamped(mut app: App) {
        app.input.buffer = "/".to_string();
        app.update_autocomplete();
        let len = app.autocomplete.command_candidates.len();
        app.autocomplete.index = len + 5; // way out of bounds
        app.update_autocomplete(); // should clamp
        assert!(app.autocomplete.index < app.autocomplete.command_candidates.len());
    }

    // --- Join autocomplete tests ---

    #[rstest]
    fn join_autocomplete_shows_contacts(mut app: App) {
        app.store
            .contact_names
            .insert("+1".to_string(), "Alice".to_string());
        app.store
            .contact_names
            .insert("+2".to_string(), "Bob".to_string());
        app.input.buffer = "/join ".to_string();
        app.update_autocomplete();
        assert!(app.is_overlay(OverlayKind::Autocomplete));
        assert_eq!(app.autocomplete.mode, AutocompleteMode::Join);
        assert_eq!(app.autocomplete.join_candidates.len(), 2);
    }

    #[rstest]
    fn join_autocomplete_shows_groups(mut app: App) {
        app.store.groups.insert(
            "g1".to_string(),
            Group {
                id: "g1".to_string(),
                name: "Family".to_string(),
                members: vec![],
                member_uuids: vec![],
            },
        );
        app.input.buffer = "/join ".to_string();
        app.update_autocomplete();
        assert!(app.is_overlay(OverlayKind::Autocomplete));
        assert_eq!(app.autocomplete.mode, AutocompleteMode::Join);
        assert_eq!(app.autocomplete.join_candidates.len(), 1);
        assert!(app.autocomplete.join_candidates[0].0.starts_with('#'));
    }

    #[rstest]
    fn join_autocomplete_filters_by_name(mut app: App) {
        app.store
            .contact_names
            .insert("+1".to_string(), "Alice".to_string());
        app.store
            .contact_names
            .insert("+2".to_string(), "Bob".to_string());
        app.input.buffer = "/join al".to_string();
        app.update_autocomplete();
        assert!(app.is_overlay(OverlayKind::Autocomplete));
        assert_eq!(app.autocomplete.join_candidates.len(), 1);
        assert!(app.autocomplete.join_candidates[0].0.contains("Alice"));
    }

    #[rstest]
    fn join_autocomplete_filters_by_phone(mut app: App) {
        app.store
            .contact_names
            .insert("+1234".to_string(), "Alice".to_string());
        app.store
            .contact_names
            .insert("+5678".to_string(), "Bob".to_string());
        app.input.buffer = "/join +123".to_string();
        app.update_autocomplete();
        assert!(app.is_overlay(OverlayKind::Autocomplete));
        assert_eq!(app.autocomplete.join_candidates.len(), 1);
        assert!(app.autocomplete.join_candidates[0].1 == "+1234");
    }

    #[rstest]
    fn join_autocomplete_alias(mut app: App) {
        app.store
            .contact_names
            .insert("+1".to_string(), "Alice".to_string());
        app.input.buffer = "/j ".to_string();
        app.update_autocomplete();
        assert!(app.is_overlay(OverlayKind::Autocomplete));
        assert_eq!(app.autocomplete.mode, AutocompleteMode::Join);
        assert_eq!(app.autocomplete.join_candidates.len(), 1);
    }

    #[rstest]
    fn join_autocomplete_no_match_hides(mut app: App) {
        app.store
            .contact_names
            .insert("+1".to_string(), "Alice".to_string());
        app.input.buffer = "/join zzz".to_string();
        app.update_autocomplete();
        assert!(!app.is_overlay(OverlayKind::Autocomplete));
    }

    #[rstest]
    fn apply_join_autocomplete(mut app: App) {
        app.store
            .contact_names
            .insert("+1".to_string(), "Alice".to_string());
        app.input.buffer = "/join al".to_string();
        app.update_autocomplete();
        assert!(app.is_overlay(OverlayKind::Autocomplete));
        app.apply_autocomplete();
        assert_eq!(app.input.buffer, "/join +1");
        assert_eq!(app.input.cursor, 8);
        assert!(!app.is_overlay(OverlayKind::Autocomplete));
    }

    #[rstest]
    fn apply_join_autocomplete_group(mut app: App) {
        app.store.groups.insert(
            "g1".to_string(),
            Group {
                id: "g1".to_string(),
                name: "Family".to_string(),
                members: vec![],
                member_uuids: vec![],
            },
        );
        app.input.buffer = "/join fam".to_string();
        app.update_autocomplete();
        assert!(app.is_overlay(OverlayKind::Autocomplete));
        app.apply_autocomplete();
        assert_eq!(app.input.buffer, "/join g1");
        assert_eq!(app.input.cursor, 8);
    }

    #[rstest]
    fn join_autocomplete_includes_conversations(mut app: App) {
        // Create a conversation that isn't in contact_names
        app.store
            .get_or_create_conversation("+9999", "+9999", false, &app.db);
        app.input.buffer = "/join +999".to_string();
        app.update_autocomplete();
        assert!(app.is_overlay(OverlayKind::Autocomplete));
        assert_eq!(app.autocomplete.join_candidates.len(), 1);
    }

    #[rstest]
    fn join_autocomplete_skips_group_ids_in_contacts(mut app: App) {
        // group IDs in contact_names don't start with '+'
        app.store
            .contact_names
            .insert("g1".to_string(), "Family".to_string());
        app.store
            .contact_names
            .insert("+1".to_string(), "Alice".to_string());
        app.input.buffer = "/join ".to_string();
        app.update_autocomplete();
        assert!(app.is_overlay(OverlayKind::Autocomplete));
        // Only Alice should appear from contact_names (g1 is skipped as non-phone)
        let contact_entries: Vec<_> = app
            .autocomplete
            .join_candidates
            .iter()
            .filter(|(_, v)| v == "+1")
            .collect();
        assert_eq!(contact_entries.len(), 1);
    }

    #[rstest]
    fn join_autocomplete_index_clamped(mut app: App) {
        app.store
            .contact_names
            .insert("+1".to_string(), "Alice".to_string());
        app.input.buffer = "/join ".to_string();
        app.update_autocomplete();
        app.autocomplete.index = 100; // way out of bounds
        app.update_autocomplete(); // should clamp
        assert!(app.autocomplete.index < app.autocomplete.join_candidates.len());
    }

    // --- apply_input_edit tests ---

    #[rstest]
    fn input_edit_char_insert(mut app: App) {
        assert!(app.apply_input_edit(KeyCode::Char('a')));
        assert!(app.apply_input_edit(KeyCode::Char('b')));
        assert_eq!(app.input.buffer, "ab");
        assert_eq!(app.input.cursor, 2);
    }

    #[rstest]
    fn input_edit_backspace(mut app: App) {
        app.input.buffer = "abc".to_string();
        app.input.cursor = 3;
        assert!(app.apply_input_edit(KeyCode::Backspace));
        assert_eq!(app.input.buffer, "ab");
        assert_eq!(app.input.cursor, 2);
    }

    #[rstest]
    fn input_edit_delete(mut app: App) {
        app.input.buffer = "abc".to_string();
        app.input.cursor = 1;
        assert!(app.apply_input_edit(KeyCode::Delete));
        assert_eq!(app.input.buffer, "ac");
        assert_eq!(app.input.cursor, 1);
    }

    #[rstest]
    fn input_edit_left_right(mut app: App) {
        app.input.buffer = "abc".to_string();
        app.input.cursor = 2;
        assert!(app.apply_input_edit(KeyCode::Left));
        assert_eq!(app.input.cursor, 1);
        assert!(app.apply_input_edit(KeyCode::Right));
        assert_eq!(app.input.cursor, 2);
    }

    #[rstest]
    fn input_edit_home_end(mut app: App) {
        app.input.buffer = "abc".to_string();
        app.input.cursor = 1;
        assert!(app.apply_input_edit(KeyCode::Home));
        assert_eq!(app.input.cursor, 0);
        assert!(app.apply_input_edit(KeyCode::End));
        assert_eq!(app.input.cursor, 3);
    }

    #[rstest]
    fn input_edit_unhandled_key(mut app: App) {
        assert!(!app.apply_input_edit(KeyCode::F(1)));
    }

    // --- Input history tests ---

    #[rstest]
    fn history_up_empty_is_noop(mut app: App) {
        app.input.buffer = "draft".to_string();
        app.history_up();
        assert_eq!(app.input.buffer, "draft");
        assert_eq!(app.input.history_index, None);
    }

    #[rstest]
    fn history_down_without_browsing_is_noop(mut app: App) {
        app.input.buffer = "draft".to_string();
        app.history_down();
        assert_eq!(app.input.buffer, "draft");
        assert_eq!(app.input.history_index, None);
    }

    #[rstest]
    fn history_up_recalls_last_entry(mut app: App) {
        app.input.history = vec!["hello".to_string(), "goodbye".to_string()];
        app.input.buffer = "draft".to_string();
        app.input.cursor = 5;

        app.history_up();
        assert_eq!(app.input.buffer, "goodbye");
        assert_eq!(app.input.history_index, Some(1));
        assert_eq!(app.input.history_draft, "draft");
        assert_eq!(app.input.cursor, 7); // cursor at end of "goodbye" (7 bytes, distinct from draft's 5)
    }

    #[rstest]
    fn history_up_walks_to_oldest(mut app: App) {
        app.input.history = vec![
            "first".to_string(),
            "second".to_string(),
            "third".to_string(),
        ];
        app.input.buffer = String::new();

        app.history_up(); // -> "third"
        assert_eq!(app.input.buffer, "third");
        assert_eq!(app.input.history_index, Some(2));

        app.history_up(); // -> "second"
        assert_eq!(app.input.buffer, "second");
        assert_eq!(app.input.history_index, Some(1));

        app.history_up(); // -> "first"
        assert_eq!(app.input.buffer, "first");
        assert_eq!(app.input.history_index, Some(0));

        // At oldest — stays put
        app.history_up();
        assert_eq!(app.input.buffer, "first");
        assert_eq!(app.input.history_index, Some(0));
    }

    #[rstest]
    fn history_down_walks_forward_and_restores_draft(mut app: App) {
        app.input.history = vec!["aaa".to_string(), "bbb".to_string()];
        app.input.buffer = "my draft".to_string();

        // Go to oldest
        app.history_up(); // -> "bbb"
        app.history_up(); // -> "aaa"
        assert_eq!(app.input.buffer, "aaa");
        assert_eq!(app.input.history_index, Some(0));

        // Walk forward
        app.history_down(); // -> "bbb"
        assert_eq!(app.input.buffer, "bbb");
        assert_eq!(app.input.history_index, Some(1));

        // Past newest restores draft
        app.history_down();
        assert_eq!(app.input.buffer, "my draft");
        assert_eq!(app.input.history_index, None);
    }

    #[rstest]
    fn history_cursor_moves_to_end(mut app: App) {
        app.input.history = vec!["short".to_string(), "a longer entry".to_string()];
        app.input.buffer = String::new();
        app.input.cursor = 0;

        app.history_up(); // -> "a longer entry"
        assert_eq!(app.input.cursor, 14);

        app.history_up(); // -> "short"
        assert_eq!(app.input.cursor, 5);

        app.history_down(); // -> "a longer entry"
        assert_eq!(app.input.cursor, 14);

        app.history_down(); // -> draft ""
        assert_eq!(app.input.cursor, 0);
    }

    #[rstest]
    fn handle_input_saves_to_history(mut app: App) {
        // Need an active conversation for SendText to work
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());

        app.input.buffer = "hello".to_string();
        app.input.cursor = 5;
        app.handle_input();
        assert_eq!(app.input.history, vec!["hello".to_string()]);
        assert_eq!(app.input.history_index, None);

        app.input.buffer = "world".to_string();
        app.input.cursor = 5;
        app.handle_input();
        assert_eq!(
            app.input.history,
            vec!["hello".to_string(), "world".to_string()]
        );
    }

    #[rstest]
    fn handle_input_trims_and_skips_empty(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());

        // Whitespace-only input should not be saved
        app.input.buffer = "   ".to_string();
        app.handle_input();
        assert!(app.input.history.is_empty());

        // Input with surrounding whitespace should be trimmed
        app.input.buffer = "  hello  ".to_string();
        app.input.cursor = 9;
        app.handle_input();
        assert_eq!(app.input.history, vec!["hello".to_string()]);
    }

    #[rstest]
    fn handle_input_resets_history_index(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());

        app.input.history = vec!["old".to_string()];
        app.input.history_index = Some(0);
        app.input.buffer = "new".to_string();
        app.input.cursor = 3;
        app.handle_input();

        assert_eq!(app.input.history_index, None);
    }

    #[rstest]
    fn apply_input_edit_up_down_routes_to_history(mut app: App) {
        app.input.history = vec!["recalled".to_string()];
        app.input.buffer = "draft".to_string();

        assert!(app.apply_input_edit(KeyCode::Up));
        assert_eq!(app.input.buffer, "recalled");

        assert!(app.apply_input_edit(KeyCode::Down));
        assert_eq!(app.input.buffer, "draft");
    }

    // --- Multi-line input tests ---

    #[rstest]
    fn input_line_count_single_line(mut app: App) {
        app.input.buffer = "hello".to_string();
        assert_eq!(app.input_line_count(), 1);
    }

    #[rstest]
    fn input_line_count_multi_line(mut app: App) {
        app.input.buffer = "hello\nworld\nfoo".to_string();
        assert_eq!(app.input_line_count(), 3);
    }

    #[rstest]
    fn cursor_line_col_first_line(mut app: App) {
        app.input.buffer = "hello\nworld".to_string();
        app.input.cursor = 3;
        assert_eq!(app.cursor_line_col(), (0, 3));
    }

    #[rstest]
    fn cursor_line_col_second_line(mut app: App) {
        app.input.buffer = "hello\nworld".to_string();
        app.input.cursor = 8; // "world" index 2
        assert_eq!(app.cursor_line_col(), (1, 2));
    }

    #[rstest]
    fn cursor_line_col_at_newline(mut app: App) {
        app.input.buffer = "hello\nworld".to_string();
        app.input.cursor = 6; // start of "world"
        assert_eq!(app.cursor_line_col(), (1, 0));
    }

    #[rstest]
    fn up_navigates_between_lines(mut app: App) {
        app.input.buffer = "hello\nworld".to_string();
        app.input.cursor = 8; // line 1, col 2
        app.apply_input_edit(KeyCode::Up);
        assert_eq!(app.input.cursor, 2); // line 0, col 2
    }

    #[rstest]
    fn down_navigates_between_lines(mut app: App) {
        app.input.buffer = "hello\nworld".to_string();
        app.input.cursor = 2; // line 0, col 2
        app.apply_input_edit(KeyCode::Down);
        assert_eq!(app.input.cursor, 8); // line 1, col 2
    }

    #[rstest]
    fn up_clamps_to_shorter_line(mut app: App) {
        app.input.buffer = "hi\nhello world".to_string();
        app.input.cursor = 12; // line 1, col 9
        app.apply_input_edit(KeyCode::Up);
        assert_eq!(app.input.cursor, 2); // line 0, col 2 (clamped to "hi" length)
    }

    #[rstest]
    fn down_clamps_to_shorter_line(mut app: App) {
        app.input.buffer = "hello world\nhi".to_string();
        app.input.cursor = 9; // line 0, col 9
        app.apply_input_edit(KeyCode::Down);
        assert_eq!(app.input.cursor, 14); // line 1, col 2 (clamped to "hi" length)
    }

    #[rstest]
    fn up_on_first_line_uses_history(mut app: App) {
        app.input.buffer = "hello\nworld".to_string();
        app.input.cursor = 3; // line 0, col 3
        app.input.history = vec!["recalled".to_string()];
        app.apply_input_edit(KeyCode::Up);
        assert_eq!(app.input.buffer, "recalled");
    }

    #[rstest]
    fn down_on_last_line_falls_through_to_history(mut app: App) {
        // Single-line buffer on last line → Down should use history_down
        app.input.buffer = "current".to_string();
        app.input.cursor = 3;
        app.input.history = vec!["old".to_string()];
        app.input.history_index = Some(0);
        // history_down from index 0 with 1 item → restores draft
        // but we didn't save a draft via history_up, so draft is ""
        app.apply_input_edit(KeyCode::Down);
        assert_eq!(app.input.history_index, None); // exited history browsing
    }

    #[rstest]
    fn home_end_line_aware(mut app: App) {
        app.input.buffer = "hello\nworld".to_string();
        app.input.cursor = 8; // line 1, col 2
        app.apply_input_edit(KeyCode::Home);
        assert_eq!(app.input.cursor, 6); // start of line 1
        app.apply_input_edit(KeyCode::End);
        assert_eq!(app.input.cursor, 11); // end of line 1
    }

    #[rstest]
    fn alt_enter_inserts_newline(mut app: App) {
        app.mode = InputMode::Insert;
        app.input.buffer = "hello".to_string();
        app.input.cursor = 5;
        app.handle_insert_key(KeyModifiers::ALT, KeyCode::Enter);
        assert_eq!(app.input.buffer, "hello\n");
        assert_eq!(app.input.cursor, 6);
    }

    #[rstest]
    fn enter_sends_multiline_message(mut app: App) {
        app.mode = InputMode::Insert;
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        app.input.buffer = "hello\nworld".to_string();
        app.input.cursor = 11;
        let result = app.handle_insert_key(KeyModifiers::NONE, KeyCode::Enter);
        assert!(result.is_some()); // should produce a SendRequest
        assert!(app.input.buffer.is_empty()); // buffer cleared after send
    }

    #[rstest]
    fn paste_normalizes_line_endings(mut app: App) {
        app.mode = InputMode::Insert;
        app.handle_paste("hello\r\nworld\rfoo".to_string());
        assert_eq!(app.input.buffer, "hello\nworld\nfoo");
    }

    // --- Pagination tests ---

    #[rstest]
    fn load_from_db_marks_has_more(mut app: App) {
        // Insert exactly PAGE_SIZE messages
        let conv_id = "+pagination";
        app.db
            .upsert_conversation(conv_id, "PagTest", false)
            .unwrap();
        for i in 0..App::PAGE_SIZE {
            app.db
                .insert_message(
                    conv_id,
                    "Alice",
                    &format!("2025-01-01T00:{:02}:{:02}Z", i / 60, i % 60),
                    &format!("msg{i}"),
                    false,
                    None,
                    i as i64 * 1000,
                )
                .unwrap();
        }
        app.load_from_db().unwrap();
        assert!(app.store.has_more_messages.contains(conv_id));
    }

    #[rstest]
    fn load_from_db_no_more_when_under_page_size(mut app: App) {
        let conv_id = "+small";
        app.db.upsert_conversation(conv_id, "Small", false).unwrap();
        app.db
            .insert_message(
                conv_id,
                "Alice",
                "2025-01-01T00:00:00Z",
                "only one",
                false,
                None,
                0,
            )
            .unwrap();
        app.load_from_db().unwrap();
        assert!(!app.store.has_more_messages.contains(conv_id));
    }

    #[rstest]
    fn load_more_messages_prepends(mut app: App) {
        let conv_id = "+paginate";
        app.db.upsert_conversation(conv_id, "Test", false).unwrap();
        // Insert 150 messages (more than PAGE_SIZE=100)
        for i in 0..150 {
            app.db
                .insert_message(
                    conv_id,
                    "Alice",
                    &format!("2025-01-01T{:02}:{:02}:00Z", i / 60, i % 60),
                    &format!("msg{i}"),
                    false,
                    None,
                    i as i64 * 1000,
                )
                .unwrap();
        }
        app.load_from_db().unwrap();
        app.active_conversation = Some(conv_id.to_string());

        // Should have 100 messages loaded, has_more set
        assert_eq!(app.store.conversations[conv_id].messages.len(), 100);
        assert!(app.store.has_more_messages.contains(conv_id));

        // The loaded messages should be the 100 most recent (msg50..msg149)
        assert_eq!(app.store.conversations[conv_id].messages[0].body, "msg50");
        assert_eq!(app.store.conversations[conv_id].messages[99].body, "msg149");

        // Set last_read_index and scroll.focused_index to verify they shift
        app.store.last_read_index.insert(conv_id.to_string(), 90);
        app.scroll.focused_index = Some(95);

        // Trigger load_more
        app.load_more_messages();

        // Should now have 150 messages, oldest first
        assert_eq!(app.store.conversations[conv_id].messages.len(), 150);
        assert_eq!(app.store.conversations[conv_id].messages[0].body, "msg0");
        assert_eq!(
            app.store.conversations[conv_id].messages[149].body,
            "msg149"
        );

        // Indexes should have shifted by 50 (the prepend count)
        assert_eq!(app.store.last_read_index[conv_id], 140);
        assert_eq!(app.scroll.focused_index, Some(145));

        // No more messages to load
        assert!(!app.store.has_more_messages.contains(conv_id));
    }

    // --- Receipt handling tests ---

    #[rstest]
    fn receipt_upgrades_outgoing_message_status(mut app: App) {
        // Create a conversation with an outgoing message
        let conv_id = "+1";
        app.store
            .get_or_create_conversation(conv_id, "Alice", false, &app.db);
        let ts_ms = 1700000000000_i64;
        if let Some(conv) = app.store.conversations.get_mut(conv_id) {
            conv.messages.push(DisplayMessage {
                sender: "you".to_string(),
                timestamp: chrono::Utc::now(),
                body: "hello".to_string(),
                is_system: false,
                image_lines: None,
                image_path: None,
                status: Some(MessageStatus::Sent),
                timestamp_ms: ts_ms,
                reactions: Vec::new(),
                mention_ranges: Vec::new(),
                style_ranges: Vec::new(),
                body_raw: None,
                mentions: Vec::new(),
                quote: None,
                is_edited: false,
                is_deleted: false,
                is_pinned: false,
                sender_id: String::new(),
                expires_in_seconds: 0,
                expiration_start_ms: 0,
                poll_data: None,
                poll_votes: Vec::new(),
                preview: None,
                preview_image_lines: None,
                preview_image_path: None,
            });
        }

        // Delivery receipt
        app.handle_signal_event(SignalEvent::ReceiptReceived {
            sender: conv_id.to_string(),
            receipt_type: "DELIVERY".to_string(),
            timestamps: vec![ts_ms],
        });
        assert_eq!(
            app.store.conversations[conv_id].messages[0].status,
            Some(MessageStatus::Delivered)
        );

        // Read receipt — should upgrade
        app.handle_signal_event(SignalEvent::ReceiptReceived {
            sender: conv_id.to_string(),
            receipt_type: "READ".to_string(),
            timestamps: vec![ts_ms],
        });
        assert_eq!(
            app.store.conversations[conv_id].messages[0].status,
            Some(MessageStatus::Read)
        );
    }

    #[rstest]
    fn receipt_does_not_downgrade_status(mut app: App) {
        let conv_id = "+1";
        app.store
            .get_or_create_conversation(conv_id, "Alice", false, &app.db);
        let ts_ms = 1700000000000_i64;
        if let Some(conv) = app.store.conversations.get_mut(conv_id) {
            conv.messages.push(DisplayMessage {
                sender: "you".to_string(),
                timestamp: chrono::Utc::now(),
                body: "hello".to_string(),
                is_system: false,
                image_lines: None,
                image_path: None,
                status: Some(MessageStatus::Read),
                timestamp_ms: ts_ms,
                reactions: Vec::new(),
                mention_ranges: Vec::new(),
                style_ranges: Vec::new(),
                body_raw: None,
                mentions: Vec::new(),
                quote: None,
                is_edited: false,
                is_deleted: false,
                is_pinned: false,
                sender_id: String::new(),
                expires_in_seconds: 0,
                expiration_start_ms: 0,
                poll_data: None,
                poll_votes: Vec::new(),
                preview: None,
                preview_image_lines: None,
                preview_image_path: None,
            });
        }

        // Delivery receipt after Read — should NOT downgrade
        app.handle_signal_event(SignalEvent::ReceiptReceived {
            sender: conv_id.to_string(),
            receipt_type: "DELIVERY".to_string(),
            timestamps: vec![ts_ms],
        });
        assert_eq!(
            app.store.conversations[conv_id].messages[0].status,
            Some(MessageStatus::Read)
        );
    }

    #[rstest]
    fn send_timestamp_upgrades_sending_to_sent(mut app: App) {
        let conv_id = "+1";
        app.store
            .get_or_create_conversation(conv_id, "Alice", false, &app.db);
        let local_ts = 1700000000000_i64;
        let server_ts = 1700000000123_i64;

        if let Some(conv) = app.store.conversations.get_mut(conv_id) {
            conv.messages.push(DisplayMessage {
                sender: "you".to_string(),
                timestamp: chrono::Utc::now(),
                body: "hello".to_string(),
                is_system: false,
                image_lines: None,
                image_path: None,
                status: Some(MessageStatus::Sending),
                timestamp_ms: local_ts,
                reactions: Vec::new(),
                mention_ranges: Vec::new(),
                style_ranges: Vec::new(),
                body_raw: None,
                mentions: Vec::new(),
                quote: None,
                is_edited: false,
                is_deleted: false,
                is_pinned: false,
                sender_id: String::new(),
                expires_in_seconds: 0,
                expiration_start_ms: 0,
                poll_data: None,
                poll_votes: Vec::new(),
                preview: None,
                preview_image_lines: None,
                preview_image_path: None,
            });
        }

        // Register pending send
        app.pending
            .sends
            .insert("rpc-1".to_string(), (conv_id.to_string(), local_ts));

        app.handle_signal_event(SignalEvent::SendTimestamp {
            rpc_id: "rpc-1".to_string(),
            server_ts,
        });

        let msg = &app.store.conversations[conv_id].messages[0];
        assert_eq!(msg.status, Some(MessageStatus::Sent));
        assert_eq!(msg.timestamp_ms, server_ts);
    }

    #[rstest]
    fn send_failed_sets_failed_status(mut app: App) {
        let conv_id = "+1";
        app.store
            .get_or_create_conversation(conv_id, "Alice", false, &app.db);
        let local_ts = 1700000000000_i64;

        if let Some(conv) = app.store.conversations.get_mut(conv_id) {
            conv.messages.push(DisplayMessage {
                sender: "you".to_string(),
                timestamp: chrono::Utc::now(),
                body: "hello".to_string(),
                is_system: false,
                image_lines: None,
                image_path: None,
                status: Some(MessageStatus::Sending),
                timestamp_ms: local_ts,
                reactions: Vec::new(),
                mention_ranges: Vec::new(),
                style_ranges: Vec::new(),
                body_raw: None,
                mentions: Vec::new(),
                quote: None,
                is_edited: false,
                is_deleted: false,
                is_pinned: false,
                sender_id: String::new(),
                expires_in_seconds: 0,
                expiration_start_ms: 0,
                poll_data: None,
                poll_votes: Vec::new(),
                preview: None,
                preview_image_lines: None,
                preview_image_path: None,
            });
        }

        app.pending
            .sends
            .insert("rpc-1".to_string(), (conv_id.to_string(), local_ts));

        app.handle_signal_event(SignalEvent::SendFailed {
            rpc_id: "rpc-1".to_string(),
        });

        assert_eq!(
            app.store.conversations[conv_id].messages[0].status,
            Some(MessageStatus::Failed)
        );
    }

    // --- Paste cleanup tests ---

    #[rstest]
    fn send_timestamp_resets_paste_cleanup_deadline(mut app: App) {
        // Set up a sentinel entry (far-future deadline = awaiting confirmation)
        let tmp = std::env::temp_dir().join("test-paste-dummy.png");
        let sentinel = Instant::now() + std::time::Duration::from_secs(PASTE_CLEANUP_SENTINEL_SECS);
        app.pending_paste_cleanups
            .insert("rpc-1".to_string(), (tmp.clone(), sentinel));

        app.handle_signal_event(SignalEvent::SendTimestamp {
            rpc_id: "rpc-1".to_string(),
            server_ts: 0,
        });

        // Deadline should now be ~10s from now, well under the sentinel
        let (_, deadline) = app
            .pending_paste_cleanups
            .get("rpc-1")
            .expect("entry should still exist");
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            remaining <= std::time::Duration::from_secs(PASTE_CLEANUP_DELAY_SECS),
            "deadline should be reset to ~{PASTE_CLEANUP_DELAY_SECS}s, got {remaining:?}"
        );
    }

    #[rstest]
    fn send_failed_resets_paste_cleanup_deadline(mut app: App) {
        let tmp = std::env::temp_dir().join("test-paste-dummy-fail.png");
        let sentinel = Instant::now() + std::time::Duration::from_secs(PASTE_CLEANUP_SENTINEL_SECS);
        app.pending_paste_cleanups
            .insert("rpc-2".to_string(), (tmp.clone(), sentinel));

        app.handle_signal_event(SignalEvent::SendFailed {
            rpc_id: "rpc-2".to_string(),
        });

        let (_, deadline) = app
            .pending_paste_cleanups
            .get("rpc-2")
            .expect("entry should still exist");
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            remaining <= std::time::Duration::from_secs(PASTE_CLEANUP_DELAY_SECS),
            "deadline should be reset to ~{PASTE_CLEANUP_DELAY_SECS}s, got {remaining:?}"
        );
    }

    #[rstest]
    fn cleanup_paste_files_removes_file_after_deadline(mut app: App) {
        // Create a real temp file
        let tmp =
            std::env::temp_dir().join(format!("test-paste-cleanup-{}.png", std::process::id()));
        std::fs::write(&tmp, b"fake image data").expect("write temp file");
        assert!(tmp.exists());

        // Insert with a deadline already in the past
        let past = Instant::now() - std::time::Duration::from_secs(1);
        app.pending_paste_cleanups
            .insert("rpc-3".to_string(), (tmp.clone(), past));

        app.cleanup_paste_files();

        assert!(!tmp.exists(), "temp file should have been deleted");
        assert!(
            app.pending_paste_cleanups.is_empty(),
            "entry should be removed"
        );
    }

    #[rstest]
    fn cleanup_paste_files_keeps_file_before_deadline(mut app: App) {
        let tmp = std::env::temp_dir().join(format!("test-paste-keep-{}.png", std::process::id()));
        std::fs::write(&tmp, b"fake image data").expect("write temp file");

        // Insert with a future deadline
        let future = Instant::now() + std::time::Duration::from_secs(60);
        app.pending_paste_cleanups
            .insert("rpc-4".to_string(), (tmp.clone(), future));

        app.cleanup_paste_files();

        // File should still exist; clean it up manually
        assert!(tmp.exists(), "temp file should not have been deleted yet");
        let _ = std::fs::remove_file(&tmp);
        assert!(
            !app.pending_paste_cleanups.is_empty(),
            "entry should still be present"
        );
    }

    #[rstest]
    fn incoming_messages_have_no_status(mut app: App) {
        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: Some("Alice".to_string()),
            source_uuid: None,
            timestamp: chrono::Utc::now(),
            body: Some("hello".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
            mentions: vec![],
            text_styles: vec![],
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        assert_eq!(app.store.conversations["+1"].messages[0].status, None);
    }

    #[rstest]
    fn receipt_before_send_timestamp_is_buffered_and_replayed(mut app: App) {
        let conv_id = "+1";
        app.store
            .get_or_create_conversation(conv_id, "Alice", false, &app.db);
        let local_ts = 1700000000000_i64;
        let server_ts = 1700000000123_i64;

        // Create outgoing message with local timestamp (Sending state)
        if let Some(conv) = app.store.conversations.get_mut(conv_id) {
            conv.messages.push(DisplayMessage {
                sender: "you".to_string(),
                timestamp: chrono::Utc::now(),
                body: "hello".to_string(),
                is_system: false,
                image_lines: None,
                image_path: None,
                status: Some(MessageStatus::Sending),
                timestamp_ms: local_ts,
                reactions: Vec::new(),
                mention_ranges: Vec::new(),
                style_ranges: Vec::new(),
                body_raw: None,
                mentions: Vec::new(),
                quote: None,
                is_edited: false,
                is_deleted: false,
                is_pinned: false,
                sender_id: String::new(),
                expires_in_seconds: 0,
                expiration_start_ms: 0,
                poll_data: None,
                poll_votes: Vec::new(),
                preview: None,
                preview_image_lines: None,
                preview_image_path: None,
            });
        }

        app.pending
            .sends
            .insert("rpc-1".to_string(), (conv_id.to_string(), local_ts));

        // Receipt arrives BEFORE SendTimestamp (references server_ts which we don't know yet)
        app.handle_signal_event(SignalEvent::ReceiptReceived {
            sender: conv_id.to_string(),
            receipt_type: "DELIVERY".to_string(),
            timestamps: vec![server_ts],
        });

        // Receipt should be buffered, message still Sending
        assert_eq!(
            app.store.conversations[conv_id].messages[0].status,
            Some(MessageStatus::Sending)
        );
        assert_eq!(app.pending.receipts.len(), 1);

        // Now SendTimestamp arrives — updates timestamp_ms and replays buffered receipts
        app.handle_signal_event(SignalEvent::SendTimestamp {
            rpc_id: "rpc-1".to_string(),
            server_ts,
        });

        // Message should now be Delivered (Sending → Sent by SendTimestamp, then → Delivered by replayed receipt)
        assert_eq!(
            app.store.conversations[conv_id].messages[0].status,
            Some(MessageStatus::Delivered)
        );
        assert!(app.pending.receipts.is_empty());
    }

    // --- Reaction tests ---

    #[rstest]
    fn handle_reaction_adds_to_message(mut app: App) {
        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: Some("Alice".to_string()),
            source_uuid: None,
            timestamp: chrono::Utc::now(),
            body: Some("hello".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
            mentions: vec![],
            text_styles: vec![],
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        let ts_ms = app.store.conversations["+1"].messages[0].timestamp_ms;

        // React with thumbs up
        app.handle_signal_event(SignalEvent::ReactionReceived {
            conv_id: "+1".to_string(),
            emoji: "\u{1f44d}".to_string(),
            sender: "+2".to_string(),
            sender_name: Some("Bob".to_string()),
            target_author: "+1".to_string(),
            target_timestamp: ts_ms,
            is_remove: false,
        });

        let reactions = &app.store.conversations["+1"].messages[0].reactions;
        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0].emoji, "\u{1f44d}");
        // Sender should be resolved to display name
        assert_eq!(reactions[0].sender, "Bob");
    }

    #[rstest]
    fn handle_reaction_replaces_existing_from_same_sender(mut app: App) {
        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: Some("Alice".to_string()),
            source_uuid: None,
            timestamp: chrono::Utc::now(),
            body: Some("hello".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
            mentions: vec![],
            text_styles: vec![],
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        let ts_ms = app.store.conversations["+1"].messages[0].timestamp_ms;

        // First reaction
        app.handle_signal_event(SignalEvent::ReactionReceived {
            conv_id: "+1".to_string(),
            emoji: "\u{1f44d}".to_string(),
            sender: "+2".to_string(),
            sender_name: Some("Bob".to_string()),
            target_author: "+1".to_string(),
            target_timestamp: ts_ms,
            is_remove: false,
        });
        // Replace with different emoji
        app.handle_signal_event(SignalEvent::ReactionReceived {
            conv_id: "+1".to_string(),
            emoji: "\u{2764}\u{fe0f}".to_string(),
            sender: "+2".to_string(),
            sender_name: Some("Bob".to_string()),
            target_author: "+1".to_string(),
            target_timestamp: ts_ms,
            is_remove: false,
        });

        let reactions = &app.store.conversations["+1"].messages[0].reactions;
        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0].emoji, "\u{2764}\u{fe0f}");
    }

    #[rstest]
    fn handle_reaction_remove(mut app: App) {
        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: Some("Alice".to_string()),
            source_uuid: None,
            timestamp: chrono::Utc::now(),
            body: Some("hello".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
            mentions: vec![],
            text_styles: vec![],
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        let ts_ms = app.store.conversations["+1"].messages[0].timestamp_ms;

        // Add reaction
        app.handle_signal_event(SignalEvent::ReactionReceived {
            conv_id: "+1".to_string(),
            emoji: "\u{1f44d}".to_string(),
            sender: "+2".to_string(),
            sender_name: Some("Bob".to_string()),
            target_author: "+1".to_string(),
            target_timestamp: ts_ms,
            is_remove: false,
        });
        assert_eq!(app.store.conversations["+1"].messages[0].reactions.len(), 1);

        // Remove it
        app.handle_signal_event(SignalEvent::ReactionReceived {
            conv_id: "+1".to_string(),
            emoji: "\u{1f44d}".to_string(),
            sender: "+2".to_string(),
            sender_name: Some("Bob".to_string()),
            target_author: "+1".to_string(),
            target_timestamp: ts_ms,
            is_remove: true,
        });
        assert_eq!(app.store.conversations["+1"].messages[0].reactions.len(), 0);
    }

    #[rstest]
    fn handle_reaction_on_own_message(mut app: App) {
        // Send a message (outgoing) — simulate by creating conversation and pushing directly
        let conv_id = "+1";
        app.store
            .get_or_create_conversation(conv_id, "Alice", false, &app.db);
        let ts_ms = 1700000000000_i64;
        if let Some(conv) = app.store.conversations.get_mut(conv_id) {
            conv.messages.push(DisplayMessage {
                sender: "you".to_string(),
                timestamp: chrono::Utc::now(),
                body: "hello".to_string(),
                is_system: false,
                image_lines: None,
                image_path: None,
                status: Some(MessageStatus::Sent),
                timestamp_ms: ts_ms,
                reactions: Vec::new(),
                mention_ranges: Vec::new(),
                style_ranges: Vec::new(),
                body_raw: None,
                mentions: Vec::new(),
                quote: None,
                is_edited: false,
                is_deleted: false,
                is_pinned: false,
                sender_id: String::new(),
                expires_in_seconds: 0,
                expiration_start_ms: 0,
                poll_data: None,
                poll_votes: Vec::new(),
                preview: None,
                preview_image_lines: None,
                preview_image_path: None,
            });
        }

        // Someone reacts to our message — target_author is our account number
        app.handle_signal_event(SignalEvent::ReactionReceived {
            conv_id: conv_id.to_string(),
            emoji: "\u{1f44d}".to_string(),
            sender: "+1".to_string(),
            sender_name: Some("Alice".to_string()),
            target_author: "+10000000000".to_string(), // test_app account
            target_timestamp: ts_ms,
            is_remove: false,
        });

        let reactions = &app.store.conversations[conv_id].messages[0].reactions;
        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0].sender, "Alice");
    }

    #[rstest]
    fn handle_reaction_unknown_message_persists_to_db(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);

        // Reaction for a message not in memory (timestamp doesn't match any)
        app.handle_signal_event(SignalEvent::ReactionReceived {
            conv_id: "+1".to_string(),
            emoji: "\u{1f44d}".to_string(),
            sender: "+2".to_string(),
            sender_name: None,
            target_author: "+1".to_string(),
            target_timestamp: 9999999999999,
            is_remove: false,
        });

        // No reactions on any message (none matched)
        assert!(app.store.conversations["+1"].messages.is_empty());
        // But it was persisted to DB
        let db_reactions = app.db.load_reactions("+1").unwrap();
        assert_eq!(db_reactions.len(), 1);
    }

    #[rstest]
    fn contact_list_resolves_reactions_and_quotes(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "+1", false, &app.db);

        // Simulate DB-loaded messages: one from a contact (+2=Bob), one from
        // a non-contact (+3=Charlie, known only from sender_id on a message)
        let conv = app.store.conversations.get_mut("+1").unwrap();
        conv.messages.push(DisplayMessage {
            sender: "Charlie".to_string(),
            body: "hey".to_string(),
            timestamp: chrono::Utc::now(),
            is_system: false,
            image_lines: None,
            image_path: None,
            status: None,
            timestamp_ms: 900,
            reactions: Vec::new(),
            mention_ranges: Vec::new(),
            style_ranges: Vec::new(),
            body_raw: None,
            mentions: Vec::new(),
            quote: None,
            is_edited: false,
            is_deleted: false,
            is_pinned: false,
            sender_id: "+3".to_string(), // Charlie's phone — not in contacts
            expires_in_seconds: 0,
            expiration_start_ms: 0,
            poll_data: None,
            poll_votes: Vec::new(),
            preview: None,
            preview_image_lines: None,
            preview_image_path: None,
        });
        conv.messages.push(DisplayMessage {
            sender: "Alice".to_string(),
            body: "hello".to_string(),
            timestamp: chrono::Utc::now(),
            is_system: false,
            image_lines: None,
            image_path: None,
            status: None,
            timestamp_ms: 1000,
            reactions: vec![
                Reaction {
                    emoji: "\u{1f44d}".to_string(),
                    sender: "+2".to_string(),
                }, // contact
                Reaction {
                    emoji: "\u{2764}".to_string(),
                    sender: "+10000000000".to_string(),
                }, // own account
                Reaction {
                    emoji: "\u{1f602}".to_string(),
                    sender: "+3".to_string(),
                }, // non-contact
            ],
            mention_ranges: Vec::new(),
            style_ranges: Vec::new(),
            body_raw: None,
            mentions: Vec::new(),
            // Quote from own account (should become "you")
            quote: Some(Quote {
                author: "+10000000000".to_string(),
                body: "quoted".to_string(),
                timestamp_ms: 500,
                author_id: "+10000000000".to_string(),
            }),
            is_edited: false,
            is_deleted: false,
            is_pinned: false,
            sender_id: "+1".to_string(),
            expires_in_seconds: 0,
            expiration_start_ms: 0,
            poll_data: None,
            poll_votes: Vec::new(),
            preview: None,
            preview_image_lines: None,
            preview_image_path: None,
        });
        // A message with a quote from a non-contact
        conv.messages.push(DisplayMessage {
            sender: "you".to_string(),
            body: "reply".to_string(),
            timestamp: chrono::Utc::now(),
            is_system: false,
            image_lines: None,
            image_path: None,
            status: None,
            timestamp_ms: 1100,
            reactions: Vec::new(),
            mention_ranges: Vec::new(),
            style_ranges: Vec::new(),
            body_raw: None,
            mentions: Vec::new(),
            quote: Some(Quote {
                author: "+3".to_string(),
                body: "hey".to_string(),
                timestamp_ms: 900,
                author_id: "+3".to_string(),
            }),
            is_edited: false,
            is_deleted: false,
            is_pinned: false,
            sender_id: "+10000000000".to_string(),
            expires_in_seconds: 0,
            expiration_start_ms: 0,
            poll_data: None,
            poll_votes: Vec::new(),
            preview: None,
            preview_image_lines: None,
            preview_image_path: None,
        });

        // Contact list arrives — only +2 is a formal contact
        app.handle_signal_event(SignalEvent::ContactList(vec![
            Contact {
                number: "+1".to_string(),
                name: Some("Alice".to_string()),
                uuid: None,
            },
            Contact {
                number: "+2".to_string(),
                name: Some("Bob".to_string()),
                uuid: None,
            },
        ]));

        let msgs = &app.store.conversations["+1"].messages;

        // Reactions resolved: +2→Bob (contact), own→you, +3→Charlie (from sender_id)
        assert_eq!(msgs[1].reactions[0].sender, "Bob");
        assert_eq!(msgs[1].reactions[1].sender, "you");
        assert_eq!(msgs[1].reactions[2].sender, "Charlie");

        // Quote authors resolved: own→you, +3→Charlie (from sender_id)
        assert_eq!(msgs[1].quote.as_ref().unwrap().author, "you");
        assert_eq!(msgs[2].quote.as_ref().unwrap().author, "Charlie");
    }

    // --- @Mention tests ---

    #[rstest]
    #[case("basic", &[("uuid-alice", "Alice")], "\u{FFFC} check this out",
        &[(0, 1, "uuid-alice")], "@Alice check this out", &["@Alice"])]
    #[case("multiple", &[("uuid-alice", "Alice"), ("uuid-bob", "Bob")],
        "\u{FFFC} and \u{FFFC} should join",
        &[(0, 1, "uuid-alice"), (6, 1, "uuid-bob")],
        "@Alice and @Bob should join", &["@Alice", "@Bob"])]
    #[case("unknown_uuid", &[], "\u{FFFC} said hi",
        &[(0, 1, "abcdef12-3456")], "@abcdef12 said hi", &["@abcdef12"])]
    #[case("empty", &[], "no mentions here", &[], "no mentions here", &[])]
    fn resolve_mentions_variants(
        mut app: App,
        #[case] _label: &str,
        #[case] uuid_names: &[(&str, &str)],
        #[case] body: &str,
        #[case] mention_data: &[(usize, usize, &str)],
        #[case] expected_body: &str,
        #[case] expected_tags: &[&str],
    ) {
        for (uuid, name) in uuid_names {
            app.store
                .uuid_to_name
                .insert(uuid.to_string(), name.to_string());
        }
        let mentions: Vec<Mention> = mention_data
            .iter()
            .map(|(start, length, uuid)| Mention {
                start: *start,
                length: *length,
                uuid: uuid.to_string(),
            })
            .collect();
        let (resolved, ranges) = app.store.resolve_mentions(body, &mentions);
        assert_eq!(resolved, expected_body);
        assert_eq!(ranges.len(), expected_tags.len());
        for (range, tag) in ranges.iter().zip(expected_tags.iter()) {
            assert_eq!(&resolved[range.0..range.1], *tag);
        }
    }

    #[rstest]
    fn mention_reresolves_when_contact_arrives_after_message(mut app: App) {
        // Regression test for #283: message with mention arrives before
        // contact list is processed, so the mention initially falls back
        // to a truncated UUID. When the contact list arrives, the mention
        // should update to the real name.
        let msg = SignalMessage {
            source: "+15550001111".to_string(),
            source_name: None,
            source_uuid: Some("aaaaaaaa-1111-1111-1111-111111111111".to_string()),
            timestamp: chrono::Utc::now(),
            body: Some("hi \u{FFFC} welcome".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
            mentions: vec![Mention {
                start: 3,
                length: 1,
                uuid: "bbbbbbbb-2222-2222-2222-222222222222".to_string(),
            }],
            text_styles: vec![],
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        // Initial resolution falls back to truncated UUID.
        let body = &app.store.conversations["+15550001111"].messages[0].body;
        assert!(
            body.contains("@bbbbbbbb"),
            "expected truncated UUID fallback, got {body:?}"
        );

        // Contact list arrives with the mentioned user.
        app.handle_signal_event(SignalEvent::ContactList(vec![Contact {
            number: "+15550002222".to_string(),
            name: Some("Bob".to_string()),
            uuid: Some("bbbbbbbb-2222-2222-2222-222222222222".to_string()),
        }]));

        // Mention should now resolve to the real name.
        let body = &app.store.conversations["+15550001111"].messages[0].body;
        assert_eq!(body, "hi @Bob welcome");
    }

    #[rstest]
    fn mention_autocomplete_in_direct_chat(mut app: App) {
        // Create a 1:1 conversation with a known contact
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.store
            .contact_names
            .insert("+1".to_string(), "Alice".to_string());
        app.active_conversation = Some("+1".to_string());
        app.input.buffer = "@Al".to_string();
        app.input.cursor = 3;
        app.update_autocomplete();

        // Should trigger mention autocomplete in 1:1 with the contact
        assert!(app.is_overlay(OverlayKind::Autocomplete));
        assert_eq!(app.autocomplete.mode, AutocompleteMode::Mention);
        assert_eq!(app.autocomplete.mention_candidates.len(), 1);
        assert_eq!(app.autocomplete.mention_candidates[0].1, "Alice");
    }

    #[rstest]
    fn mention_autocomplete_in_group(mut app: App) {
        // Set up group with members
        app.store.groups.insert(
            "g1".to_string(),
            Group {
                id: "g1".to_string(),
                name: "Test Group".to_string(),
                members: vec!["+1".to_string(), "+2".to_string()],
                member_uuids: vec![],
            },
        );
        app.store
            .contact_names
            .insert("+1".to_string(), "Alice".to_string());
        app.store
            .contact_names
            .insert("+2".to_string(), "Bob".to_string());
        app.store
            .get_or_create_conversation("g1", "Test Group", true, &app.db);
        app.active_conversation = Some("g1".to_string());

        app.input.buffer = "@Al".to_string();
        app.input.cursor = 3;
        app.update_autocomplete();

        assert!(app.is_overlay(OverlayKind::Autocomplete));
        assert_eq!(app.autocomplete.mode, AutocompleteMode::Mention);
        assert_eq!(app.autocomplete.mention_candidates.len(), 1);
        assert_eq!(app.autocomplete.mention_candidates[0].1, "Alice");
    }

    #[rstest]
    fn apply_mention_autocomplete(mut app: App) {
        // Set up group with members
        app.store.groups.insert(
            "g1".to_string(),
            Group {
                id: "g1".to_string(),
                name: "Test Group".to_string(),
                members: vec!["+1".to_string()],
                member_uuids: vec![],
            },
        );
        app.store
            .contact_names
            .insert("+1".to_string(), "Alice".to_string());
        app.store
            .number_to_uuid
            .insert("+1".to_string(), "uuid-alice".to_string());
        app.store
            .get_or_create_conversation("g1", "Test Group", true, &app.db);
        app.active_conversation = Some("g1".to_string());

        app.input.buffer = "Hey @Al".to_string();
        app.input.cursor = 7;
        app.update_autocomplete();
        assert!(app.is_overlay(OverlayKind::Autocomplete));

        app.apply_autocomplete();
        assert_eq!(app.input.buffer, "Hey @Alice ");
        assert_eq!(app.autocomplete.pending_mentions.len(), 1);
        assert_eq!(app.autocomplete.pending_mentions[0].0, "Alice");
        assert_eq!(
            app.autocomplete.pending_mentions[0].1.as_deref(),
            Some("uuid-alice")
        );
    }

    #[rstest]
    fn prepare_outgoing_mentions(mut app: App) {
        app.autocomplete.pending_mentions =
            vec![("Alice".to_string(), Some("uuid-alice".to_string()))];

        let (wire, mentions) = app.prepare_outgoing_mentions("Hey @Alice what's up");
        assert_eq!(wire, "Hey \u{FFFC} what's up");
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].0, 4); // UTF-16 offset of U+FFFC
        assert_eq!(mentions[0].1, "uuid-alice");
    }

    #[rstest]
    fn prepare_outgoing_no_pending_mentions(app: App) {
        let (wire, mentions) = app.prepare_outgoing_mentions("Hello world");
        assert_eq!(wire, "Hello world");
        assert!(mentions.is_empty());
    }

    #[rstest]
    fn contact_list_builds_uuid_maps(mut app: App) {
        app.handle_signal_event(SignalEvent::ContactList(vec![Contact {
            number: "+1".to_string(),
            name: Some("Alice".to_string()),
            uuid: Some("uuid-alice".to_string()),
        }]));

        assert_eq!(app.store.uuid_to_name.get("uuid-alice").unwrap(), "Alice");
        assert_eq!(app.store.number_to_uuid.get("+1").unwrap(), "uuid-alice");
    }

    #[rstest]
    fn group_list_stores_groups(mut app: App) {
        app.handle_signal_event(SignalEvent::GroupList(vec![Group {
            id: "g1".to_string(),
            name: "Test".to_string(),
            members: vec!["+1".to_string(), "+2".to_string()],
            member_uuids: vec![],
        }]));

        assert!(app.store.groups.contains_key("g1"));
        assert_eq!(app.store.groups["g1"].members.len(), 2);
    }

    #[rstest]
    fn incoming_message_resolves_mentions(mut app: App) {
        app.store
            .uuid_to_name
            .insert("uuid-bob".to_string(), "Bob".to_string());

        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: Some("Alice".to_string()),
            source_uuid: None,
            timestamp: chrono::Utc::now(),
            body: Some("\u{FFFC} check this".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
            mentions: vec![Mention {
                start: 0,
                length: 1,
                uuid: "uuid-bob".to_string(),
            }],
            text_styles: vec![],
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        let conv = &app.store.conversations["+1"];
        assert_eq!(conv.messages[0].body, "@Bob check this");
        assert_eq!(conv.messages[0].mention_ranges.len(), 1);
    }

    #[rstest]
    fn backspace_at_zero_clears_pending_attachment(mut app: App) {
        app.pending_attachment = Some(std::path::PathBuf::from("/tmp/photo.jpg"));
        app.input.cursor = 0;
        app.input.buffer.clear();

        app.apply_input_edit(KeyCode::Backspace);
        assert!(app.pending_attachment.is_none());
    }

    #[rstest]
    fn empty_text_with_attachment_sends(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        app.pending_attachment = Some(std::path::PathBuf::from("/tmp/photo.jpg"));
        app.input.buffer.clear();
        app.input.cursor = 0;

        let result = app.handle_input();
        assert!(result.is_some());
        // Attachment should be consumed
        assert!(app.pending_attachment.is_none());
    }

    #[rstest]
    fn attach_no_conversation_shows_error(mut app: App) {
        app.active_conversation = None;
        app.open_file_browser();
        assert!(!app.is_overlay(OverlayKind::FilePicker));
        assert!(app.status_message.contains("No active conversation"));
    }

    #[rstest]
    fn clears_attachment_on_next_conversation(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        app.pending_attachment = Some(std::path::PathBuf::from("/tmp/photo.jpg"));
        app.store
            .get_or_create_conversation("+2", "Bob", false, &app.db);
        app.next_conversation();
        assert!(app.pending_attachment.is_none());
    }

    #[rstest]
    fn history_browse_state_does_not_bleed_across_conv_switch(mut app: App) {
        // Reproduces #362: pressing Up in conversation A starts history browsing
        // (sets history_index and history_draft). Switching to conversation B
        // must clear that browse state so pressing Down in B does nothing.
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.store
            .get_or_create_conversation("+2", "Bob", false, &app.db);
        app.active_conversation = Some("+1".to_string());

        app.input.history = vec!["earlier".to_string()];
        app.input.buffer = "in-progress draft".to_string();
        app.input.cursor = "in-progress draft".len();

        app.history_up();
        assert_eq!(app.input.buffer, "earlier");
        assert_eq!(app.input.history_index, Some(0));
        assert_eq!(app.input.history_draft, "in-progress draft");

        app.next_conversation();

        assert_eq!(app.active_conversation.as_deref(), Some("+2"));
        assert!(app.input.buffer.is_empty(), "buffer should be cleared");
        assert_eq!(app.input.cursor, 0);
        assert_eq!(app.input.history_index, None);
        assert_eq!(app.input.history_draft, "");
        assert_eq!(
            app.input.history,
            vec!["earlier".to_string()],
            "session history must be preserved across switches"
        );
    }

    #[rstest]
    fn spinner_pauses_during_sync(app: App) {
        // Default state on a fresh App: loading=true AND sync.active=true
        // (SyncState::new initializes active=true). The fix for #326 requires
        // the spinner to pause in this overlap window so its 50ms tick rate
        // doesn't bypass the 500ms sync redraw throttle in main's event loop.
        assert!(app.loading);
        assert!(app.sync.active);
        assert!(
            !app.should_tick_spinner(),
            "spinner must not tick while sync is active"
        );
    }

    #[rstest]
    fn spinner_ticks_after_sync_ends_but_loading_continues(mut app: App) {
        // Sync ended (no more incoming burst) but contact list hasn't arrived
        // yet, so loading is still true. Spinner should resume so the user
        // sees the "Loading contacts..." status animate.
        app.sync.active = false;
        assert!(app.loading);
        assert!(app.should_tick_spinner());
    }

    #[rstest]
    fn spinner_does_not_tick_when_loading_is_false(mut app: App) {
        // Steady-state app: nothing to spin for.
        app.loading = false;
        app.sync.active = false;
        assert!(!app.should_tick_spinner());
    }

    #[rstest]
    fn clears_attachment_on_part_command(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        app.pending_attachment = Some(std::path::PathBuf::from("/tmp/photo.jpg"));
        app.input.buffer = "/part".to_string();
        app.input.cursor = 5;
        app.handle_input();
        assert!(app.pending_attachment.is_none());
    }

    #[rstest]
    fn search_opens_overlay(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());

        // Insert a message into the DB so search has something to find
        app.db
            .insert_message(
                "+1",
                "Alice",
                "2025-01-01T00:00:00Z",
                "hello world",
                false,
                None,
                1000,
            )
            .unwrap();

        app.input.buffer = "/search hello".to_string();
        app.input.cursor = 13;
        app.handle_input();

        assert!(app.is_overlay(OverlayKind::Search));
        assert_eq!(app.search.query, "hello");
        assert!(!app.search.results.is_empty());
        assert_eq!(app.search.results[0].body, "hello world");
    }

    #[rstest]
    fn search_without_query_shows_error(mut app: App) {
        app.input.buffer = "/search".to_string();
        app.input.cursor = 7;
        app.handle_input();

        assert!(!app.is_overlay(OverlayKind::Search));
        assert!(app.status_message.contains("requires"));
    }

    #[rstest]
    fn search_overlay_esc_closes(mut app: App) {
        app.open_overlay(OverlayKind::Search);
        app.search.query = "test".to_string();

        app.handle_search_key(KeyCode::Esc);

        assert!(!app.is_overlay(OverlayKind::Search));
        assert!(app.search.query.is_empty());
    }

    #[rstest]
    fn search_overlay_typing_refines(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        app.db
            .insert_message(
                "+1",
                "Alice",
                "2025-01-01T00:00:00Z",
                "hello world",
                false,
                None,
                1000,
            )
            .unwrap();
        app.db
            .insert_message(
                "+1",
                "Alice",
                "2025-01-01T00:01:00Z",
                "goodbye world",
                false,
                None,
                2000,
            )
            .unwrap();

        app.open_overlay(OverlayKind::Search);
        app.search.query = "hello".to_string();
        app.search.run(app.active_conversation.as_deref(), &app.db);
        assert_eq!(app.search.results.len(), 1);

        // Type more to search for "world" instead
        app.search.query = "world".to_string();
        app.search.run(app.active_conversation.as_deref(), &app.db);
        assert_eq!(app.search.results.len(), 2);
    }

    #[rstest]
    fn system_message_inserted_with_is_system_true(mut app: App) {
        let ts = chrono::Utc::now();
        let ts_ms = ts.timestamp_millis();
        app.handle_signal_event(SignalEvent::SystemMessage {
            conv_id: "+15551234567".to_string(),
            body: "Missed voice call".to_string(),
            timestamp: ts,
            timestamp_ms: ts_ms,
        });

        assert!(app.store.conversations.contains_key("+15551234567"));
        let conv = &app.store.conversations["+15551234567"];
        assert_eq!(conv.messages.len(), 1);
        assert!(conv.messages[0].is_system);
        assert_eq!(conv.messages[0].body, "Missed voice call");
        assert!(conv.messages[0].sender.is_empty());
    }

    #[rstest]
    fn unread_bar_clears_on_active_incoming_message(mut app: App) {
        app.sync.active = false;

        // Deliver a message while conversation is NOT active → creates unread
        let msg1 = SignalMessage {
            source: "+15551234567".to_string(),
            source_name: Some("Alice".to_string()),
            source_uuid: None,
            timestamp: chrono::Utc::now(),
            body: Some("first".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
            mentions: vec![],
            text_styles: vec![],
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg1));

        // Conversation exists with 1 message, last_read_index should be 0 (unread)
        assert_eq!(app.store.conversations["+15551234567"].messages.len(), 1);
        let read_idx = app
            .store
            .last_read_index
            .get("+15551234567")
            .copied()
            .unwrap_or(0);
        assert_eq!(read_idx, 0);

        // Now make it the active conversation
        app.active_conversation = Some("+15551234567".to_string());

        // Deliver another message while conversation IS active
        let msg2 = SignalMessage {
            source: "+15551234567".to_string(),
            source_name: Some("Alice".to_string()),
            source_uuid: None,
            timestamp: chrono::Utc::now(),
            body: Some("second".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
            mentions: vec![],
            text_styles: vec![],
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg2));

        // last_read_index should now equal messages.len() → no unread bar
        let total = app.store.conversations["+15551234567"].messages.len();
        let read_idx = app.store.last_read_index["+15551234567"];
        assert_eq!(total, 2);
        assert_eq!(read_idx, total);
    }

    #[rstest]
    fn read_sync_advances_read_marker_and_clears_unread(mut app: App) {
        // Create a conversation with 3 messages (all incoming, unread)
        let msg = |body: &str, ts_ms: i64| SignalMessage {
            source: "+15551234567".to_string(),
            source_name: Some("Alice".to_string()),
            source_uuid: None,
            timestamp: DateTime::from_timestamp_millis(ts_ms).unwrap(),
            body: Some(body.to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
            mentions: vec![],
            text_styles: vec![],
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg("one", 1000)));
        app.handle_signal_event(SignalEvent::MessageReceived(msg("two", 2000)));
        app.handle_signal_event(SignalEvent::MessageReceived(msg("three", 3000)));

        assert_eq!(app.store.conversations["+15551234567"].unread, 3);
        assert_eq!(
            app.store
                .last_read_index
                .get("+15551234567")
                .copied()
                .unwrap_or(0),
            0
        );

        // Simulate reading through timestamp 2000 on another device
        app.handle_signal_event(SignalEvent::ReadSyncReceived {
            read_messages: vec![("+15551234567".to_string(), 2000)],
        });

        // Read marker should advance to index 2 (after msg "one" and "two")
        assert_eq!(app.store.last_read_index["+15551234567"], 2);
        // Only "three" should remain unread
        assert_eq!(app.store.conversations["+15551234567"].unread, 1);
    }

    #[rstest]
    fn read_sync_does_not_retreat_read_marker(mut app: App) {
        let msg = |body: &str, ts_ms: i64| SignalMessage {
            source: "+15551234567".to_string(),
            source_name: Some("Alice".to_string()),
            source_uuid: None,
            timestamp: DateTime::from_timestamp_millis(ts_ms).unwrap(),
            body: Some(body.to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
            mentions: vec![],
            text_styles: vec![],
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg("one", 1000)));
        app.handle_signal_event(SignalEvent::MessageReceived(msg("two", 2000)));
        app.handle_signal_event(SignalEvent::MessageReceived(msg("three", 3000)));

        // First sync reads up to ts 3000 (all messages)
        app.handle_signal_event(SignalEvent::ReadSyncReceived {
            read_messages: vec![("+15551234567".to_string(), 3000)],
        });
        assert_eq!(app.store.last_read_index["+15551234567"], 3);
        assert_eq!(app.store.conversations["+15551234567"].unread, 0);

        // A stale sync for ts 1000 should NOT retreat the read marker
        app.handle_signal_event(SignalEvent::ReadSyncReceived {
            read_messages: vec![("+15551234567".to_string(), 1000)],
        });
        assert_eq!(app.store.last_read_index["+15551234567"], 3);
        assert_eq!(app.store.conversations["+15551234567"].unread, 0);
    }

    // --- Text style resolution tests ---

    #[rstest]
    fn text_style_ranges_resolved_to_byte_offsets(app: App) {
        // ASCII body: "hello bold world"
        // "bold" is at UTF-16 offset 6, length 4
        let body = "hello bold world";
        let styles = vec![
            TextStyle {
                start: 6,
                length: 4,
                style: StyleType::Bold,
            },
            TextStyle {
                start: 11,
                length: 5,
                style: StyleType::Italic,
            },
        ];
        let resolved = app.store.resolve_text_styles(body, &styles, &[]);

        // In pure ASCII, UTF-16 offsets == byte offsets
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0], (6, 10, StyleType::Bold)); // "bold"
        assert_eq!(resolved[1], (11, 16, StyleType::Italic)); // "world"
    }

    #[rstest]
    fn text_style_ranges_with_multibyte_chars(app: App) {
        // Body with multi-byte chars: "Hi \u{1F600} bold" (emoji is 4 bytes UTF-8, 2 units UTF-16)
        // UTF-16: H(1) i(1) ' '(1) \u{1F600}(2) ' '(1) b(1) o(1) l(1) d(1) = offsets
        // "bold" starts at UTF-16 offset 6, length 4
        let body = "Hi \u{1F600} bold";
        let styles = vec![TextStyle {
            start: 6,
            length: 4,
            style: StyleType::Bold,
        }];
        let resolved = app.store.resolve_text_styles(body, &styles, &[]);

        // "Hi " = 3 bytes, emoji = 4 bytes, " " = 1 byte => "bold" starts at byte 8
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].0, 8); // byte start of "bold"
        assert_eq!(resolved[0].1, 12); // byte end of "bold"
        assert_eq!(resolved[0].2, StyleType::Bold);
    }

    #[rstest]
    fn text_style_ranges_with_mentions(mut app: App) {
        app.store
            .uuid_to_name
            .insert("uuid-bob".to_string(), "Bob".to_string());

        // Original body: "\u{FFFC} is bold"
        // After mention resolution: "@Bob is bold"
        // Mention at UTF-16 offset 0, length 1 => replaced with "@Bob" (4 chars)
        // "bold" is at original UTF-16 offset 5, length 4
        // After resolution shift: offset 5 + 3 (replacement grew by 3) = 8
        let resolved_body = "@Bob is bold";
        let mentions = vec![Mention {
            start: 0,
            length: 1,
            uuid: "uuid-bob".to_string(),
        }];
        let styles = vec![TextStyle {
            start: 5,
            length: 4,
            style: StyleType::Strikethrough,
        }];
        let resolved = app
            .store
            .resolve_text_styles(resolved_body, &styles, &mentions);

        assert_eq!(resolved.len(), 1);
        // "bold" in "@Bob is bold" starts at byte 8
        assert_eq!(resolved[0].0, 8);
        assert_eq!(resolved[0].1, 12);
        assert_eq!(resolved[0].2, StyleType::Strikethrough);
    }

    #[rstest]
    fn text_style_ranges_empty_styles(app: App) {
        let resolved = app.store.resolve_text_styles("hello world", &[], &[]);
        assert!(resolved.is_empty());
    }

    // --- Group management tests ---

    #[test]
    fn group_command_parsed() {
        assert!(matches!(
            crate::input::parse_input("/group"),
            crate::input::InputAction::Group
        ));
        assert!(matches!(
            crate::input::parse_input("/g"),
            crate::input::InputAction::Group
        ));
    }

    #[rstest]
    fn group_menu_items_in_group(mut app: App) {
        app.store
            .get_or_create_conversation("g1", "Family", true, &app.db);
        app.active_conversation = Some("g1".to_string());
        let items = app.group_menu_items();
        assert_eq!(items.len(), 5);
        assert_eq!(items[0].label, "Members");
        assert_eq!(items[items.len() - 1].label, "Leave");
    }

    #[rstest]
    fn group_menu_items_not_in_group(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        let items = app.group_menu_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "Create group");
    }

    #[rstest]
    fn group_menu_items_no_conversation(app: App) {
        let items = app.group_menu_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "Create group");
    }

    #[rstest]
    fn group_add_filter_excludes_existing_members(mut app: App) {
        app.store
            .get_or_create_conversation("g1", "Family", true, &app.db);
        app.active_conversation = Some("g1".to_string());
        app.store.groups.insert(
            "g1".to_string(),
            Group {
                id: "g1".to_string(),
                name: "Family".to_string(),
                members: vec!["+1".to_string(), "+2".to_string()],
                member_uuids: vec![],
            },
        );
        app.store
            .contact_names
            .insert("+1".to_string(), "Alice".to_string());
        app.store
            .contact_names
            .insert("+2".to_string(), "Bob".to_string());
        app.store
            .contact_names
            .insert("+3".to_string(), "Charlie".to_string());

        app.refresh_group_add_filter();

        // Only Charlie should appear (not Alice or Bob who are already members)
        assert_eq!(app.group_menu.filtered.len(), 1);
        assert_eq!(app.group_menu.filtered[0].0, "+3");
    }

    #[rstest]
    fn group_remove_filter_excludes_self(mut app: App) {
        app.store
            .get_or_create_conversation("g1", "Family", true, &app.db);
        app.active_conversation = Some("g1".to_string());
        app.store.groups.insert(
            "g1".to_string(),
            Group {
                id: "g1".to_string(),
                name: "Family".to_string(),
                members: vec![
                    "+10000000000".to_string(),
                    "+1".to_string(),
                    "+2".to_string(),
                ],
                member_uuids: vec![],
            },
        );
        app.store
            .contact_names
            .insert("+1".to_string(), "Alice".to_string());
        app.store
            .contact_names
            .insert("+2".to_string(), "Bob".to_string());

        app.refresh_group_remove_filter();

        // Self (+10000000000) should be excluded
        assert_eq!(app.group_menu.filtered.len(), 2);
        let phones: Vec<&str> = app
            .group_menu
            .filtered
            .iter()
            .map(|(p, _)| p.as_str())
            .collect();
        assert!(!phones.contains(&"+10000000000"));
        assert!(phones.contains(&"+1"));
        assert!(phones.contains(&"+2"));
    }

    #[rstest]
    fn group_menu_state_transitions(mut app: App) {
        app.store
            .get_or_create_conversation("g1", "Family", true, &app.db);
        app.active_conversation = Some("g1".to_string());
        app.store.groups.insert(
            "g1".to_string(),
            Group {
                id: "g1".to_string(),
                name: "Family".to_string(),
                members: vec!["+1".to_string()],
                member_uuids: vec![],
            },
        );

        // Open group menu via handle_input
        app.input.buffer = "/group".to_string();
        app.input.cursor = 6;
        app.handle_input();
        assert_eq!(app.group_menu.state, Some(GroupMenuState::Menu));

        // Press 'm' to go to Members
        app.handle_group_menu_key(KeyCode::Char('m'));
        assert_eq!(app.group_menu.state, Some(GroupMenuState::Members));

        // Esc goes back to Menu
        app.handle_group_menu_key(KeyCode::Esc);
        assert_eq!(app.group_menu.state, Some(GroupMenuState::Menu));

        // Press 'l' to go to LeaveConfirm
        app.handle_group_menu_key(KeyCode::Char('l'));
        assert_eq!(app.group_menu.state, Some(GroupMenuState::LeaveConfirm));

        // Press 'n' to cancel leave
        app.handle_group_menu_key(KeyCode::Char('n'));
        assert_eq!(app.group_menu.state, Some(GroupMenuState::Menu));

        // Esc closes the menu entirely
        app.handle_group_menu_key(KeyCode::Esc);
        assert_eq!(app.group_menu.state, None);
    }

    #[rstest]
    fn group_leave_produces_send_request(mut app: App) {
        app.store
            .get_or_create_conversation("g1", "Family", true, &app.db);
        app.active_conversation = Some("g1".to_string());
        app.store.groups.insert(
            "g1".to_string(),
            Group {
                id: "g1".to_string(),
                name: "Family".to_string(),
                members: vec![],
                member_uuids: vec![],
            },
        );

        app.group_menu.state = Some(GroupMenuState::LeaveConfirm);
        let req = app.handle_group_menu_key(KeyCode::Char('y'));
        assert!(req.is_some());
        assert!(matches!(req, Some(SendRequest::LeaveGroup { group_id }) if group_id == "g1"));
        assert_eq!(app.group_menu.state, None);
    }

    #[rstest]
    fn group_create_produces_send_request(mut app: App) {
        app.group_menu.state = Some(GroupMenuState::Create);
        app.group_menu.input = "New Group".to_string();
        let req = app.handle_group_menu_key(KeyCode::Enter);
        assert!(req.is_some());
        assert!(matches!(req, Some(SendRequest::CreateGroup { name }) if name == "New Group"));
        assert_eq!(app.group_menu.state, None);
    }

    #[rstest]
    fn group_rename_produces_send_request(mut app: App) {
        app.store
            .get_or_create_conversation("g1", "Old Name", true, &app.db);
        app.active_conversation = Some("g1".to_string());
        app.group_menu.state = Some(GroupMenuState::Rename);
        app.group_menu.input = "New Name".to_string();
        let req = app.handle_group_menu_key(KeyCode::Enter);
        assert!(req.is_some());
        assert!(
            matches!(req, Some(SendRequest::RenameGroup { group_id, name }) if group_id == "g1" && name == "New Name")
        );
        assert_eq!(app.group_menu.state, None);
    }

    // --- Message request tests ---

    fn msg_from(source: &str) -> SignalMessage {
        SignalMessage {
            source: source.to_string(),
            source_name: None,
            source_uuid: None,
            timestamp: chrono::Utc::now(),
            body: Some("hello".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
            mentions: vec![],
            text_styles: vec![],
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        }
    }

    #[rstest]
    fn unknown_sender_creates_unaccepted_conversation(mut app: App) {
        app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
        assert!(!app.store.conversations["+1"].accepted);
    }

    #[rstest]
    fn known_contact_creates_accepted_conversation(mut app: App) {
        app.store
            .contact_names
            .insert("+1".to_string(), "Alice".to_string());
        app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
        assert!(app.store.conversations["+1"].accepted);
    }

    #[rstest]
    fn outgoing_sync_creates_accepted_conversation(mut app: App) {
        let msg = SignalMessage {
            source: "+10000000000".to_string(),
            source_name: None,
            source_uuid: None,
            timestamp: chrono::Utc::now(),
            body: Some("hey".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: true,
            destination: Some("+1".to_string()),
            mentions: vec![],
            text_styles: vec![],
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert!(app.store.conversations["+1"].accepted);
    }

    #[rstest]
    fn contact_sync_auto_accepts_matching_conversations(mut app: App) {
        // Message from unknown creates unaccepted
        app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
        assert!(!app.store.conversations["+1"].accepted);

        // Contact list arrives with +1 → auto-accept
        app.handle_signal_event(SignalEvent::ContactList(vec![Contact {
            number: "+1".to_string(),
            name: Some("Alice".to_string()),
            uuid: None,
        }]));
        assert!(app.store.conversations["+1"].accepted);
    }

    #[rstest]
    fn accept_key_returns_send_request_and_marks_accepted(mut app: App) {
        app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
        app.active_conversation = Some("+1".to_string());
        app.open_overlay(OverlayKind::MessageRequest);

        let req = app.handle_message_request_key(KeyCode::Char('a'));
        assert!(app.store.conversations["+1"].accepted);
        assert!(!app.is_overlay(OverlayKind::MessageRequest));
        assert!(matches!(
            req,
            Some(SendRequest::MessageRequestResponse { ref response_type, .. })
            if response_type == "accept"
        ));
    }

    #[rstest]
    fn delete_key_removes_conversation(mut app: App) {
        app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
        app.active_conversation = Some("+1".to_string());
        // Pre-populate auxiliary state that the dedup with delete_active_conversation
        // is supposed to clean up (regression guard for the partial cleanup the
        // 'd' arm did before PR #312).
        app.scroll.positions.insert("+1".to_string(), (3, Some(0)));
        app.store.last_read_index.insert("+1".to_string(), 1);
        app.store.has_more_messages.insert("+1".to_string());
        app.muted_conversations
            .insert("+1".to_string(), MuteState::Permanent);
        app.blocked_conversations.insert("+1".to_string());
        app.open_overlay(OverlayKind::MessageRequest);

        let req = app.handle_message_request_key(KeyCode::Char('d'));
        assert!(!app.store.conversations.contains_key("+1"));
        assert!(!app.store.conversation_order.contains(&"+1".to_string()));
        assert!(!app.scroll.positions.contains_key("+1"));
        assert!(!app.store.last_read_index.contains_key("+1"));
        assert!(!app.store.has_more_messages.contains("+1"));
        assert!(!app.muted_conversations.contains_key("+1"));
        assert!(!app.blocked_conversations.contains("+1"));
        assert!(app.active_conversation.is_none());
        assert!(!app.is_overlay(OverlayKind::MessageRequest));
        assert!(matches!(
            req,
            Some(SendRequest::MessageRequestResponse { ref response_type, .. })
            if response_type == "delete"
        ));
    }

    #[rstest]
    fn esc_closes_message_request_overlay(mut app: App) {
        app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
        app.active_conversation = Some("+1".to_string());
        app.open_overlay(OverlayKind::MessageRequest);

        let req = app.handle_message_request_key(KeyCode::Esc);
        assert!(req.is_none());
        assert!(!app.is_overlay(OverlayKind::MessageRequest));
        assert!(app.active_conversation.is_none());
    }

    // --- /delete slash command + confirmation overlay (PR #312) ---

    #[rstest]
    fn slash_delete_opens_confirmation_overlay(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        app.input.buffer = "/delete".to_string();
        app.input.cursor = 7;

        let req = app.handle_input();

        assert!(req.is_none());
        assert!(app.is_overlay(OverlayKind::DeleteConversationConfirm));
        // Confirmation overlay is just a prompt — must not delete yet.
        assert!(app.store.conversations.contains_key("+1"));
        assert_eq!(app.active_conversation.as_deref(), Some("+1"));
    }

    #[rstest]
    fn slash_delete_without_active_conversation_sets_error(mut app: App) {
        app.input.buffer = "/delete".to_string();
        app.input.cursor = 7;

        let req = app.handle_input();

        assert!(req.is_none());
        assert!(!app.is_overlay(OverlayKind::DeleteConversationConfirm));
        assert!(app.status_message.contains("No active conversation"));
    }

    #[rstest]
    fn delete_confirm_yes_removes_accepted_conversation(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        app.scroll.positions.insert("+1".to_string(), (3, Some(0)));
        app.store.last_read_index.insert("+1".to_string(), 1);
        app.open_overlay(OverlayKind::DeleteConversationConfirm);

        let (handled, req) = app.handle_overlay_key(KeyCode::Char('y'));

        assert!(handled);
        // Accepted conversation: purely local, no remote response.
        assert!(req.is_none());
        assert!(!app.is_overlay(OverlayKind::DeleteConversationConfirm));
        assert!(!app.store.conversations.contains_key("+1"));
        assert!(!app.scroll.positions.contains_key("+1"));
        assert!(!app.store.last_read_index.contains_key("+1"));
        assert!(app.active_conversation.is_none());
    }

    #[rstest]
    fn delete_confirm_yes_on_unaccepted_returns_remote_delete(mut app: App) {
        app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+15550007777")));
        assert!(!app.store.conversations["+15550007777"].accepted);
        app.active_conversation = Some("+15550007777".to_string());
        app.open_overlay(OverlayKind::DeleteConversationConfirm);

        let (_, req) = app.handle_overlay_key(KeyCode::Char('y'));

        let Some(SendRequest::MessageRequestResponse {
            recipient,
            is_group,
            response_type,
        }) = req
        else {
            panic!("expected MessageRequestResponse(delete) for unaccepted conversation");
        };
        assert_eq!(recipient, "+15550007777");
        assert!(!is_group);
        assert_eq!(response_type, "delete");
        assert!(!app.store.conversations.contains_key("+15550007777"));
        assert!(app.active_conversation.is_none());
    }

    #[rstest]
    fn delete_confirm_no_keeps_conversation(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        app.open_overlay(OverlayKind::DeleteConversationConfirm);

        let (handled, req) = app.handle_overlay_key(KeyCode::Char('n'));

        assert!(handled);
        assert!(req.is_none());
        assert!(!app.is_overlay(OverlayKind::DeleteConversationConfirm));
        assert!(app.store.conversations.contains_key("+1"));
        assert_eq!(app.active_conversation.as_deref(), Some("+1"));
    }

    #[rstest]
    fn delete_confirm_esc_keeps_conversation(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        app.open_overlay(OverlayKind::DeleteConversationConfirm);

        let (_, req) = app.handle_overlay_key(KeyCode::Esc);

        assert!(req.is_none());
        assert!(!app.is_overlay(OverlayKind::DeleteConversationConfirm));
        assert!(app.store.conversations.contains_key("+1"));
    }

    #[rstest]
    fn update_status_does_not_clobber_active_app_overlay(mut app: App) {
        // Regression guard for PR #345: opening MessageRequest during a
        // conversation switch must not close an already-open App-owned
        // overlay (e.g. Settings mid-edit). The user would see their
        // settings state silently vanish.
        app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+unaccepted")));
        app.open_overlay(OverlayKind::Settings);
        app.active_conversation = Some("+unaccepted".to_string());

        // Simulate the conversation switch that triggers update_status.
        // (update_status is private; calling the public code path via an
        // explicit switch reaches it.)
        app.update_status();

        assert_eq!(
            app.active_overlay(),
            Some(OverlayKind::Settings),
            "Settings overlay must not be clobbered by MessageRequest"
        );
    }

    #[rstest]
    fn autocomplete_esc_closes_overlay(mut app: App) {
        // Regression guard for PR #346: pre-fix, AutocompleteState::clear()
        // no longer touched visibility, so Esc cleared candidates but left
        // current_overlay = Some(Autocomplete), trapping subsequent input
        // in the empty handler.
        app.mode = InputMode::Insert;
        app.input.buffer = "/j".to_string();
        app.input.cursor = 2;
        app.update_autocomplete();
        assert!(app.is_overlay(OverlayKind::Autocomplete));

        app.handle_autocomplete_key(KeyCode::Esc);
        assert!(
            !app.is_overlay(OverlayKind::Autocomplete),
            "Esc should close the autocomplete overlay"
        );
    }

    #[rstest]
    fn autocomplete_no_match_closes_overlay(mut app: App) {
        // Regression guard for PR #346: typing past any candidate match
        // should drop visibility, not leave the empty overlay open.
        app.mode = InputMode::Insert;
        app.input.buffer = "/j".to_string();
        app.input.cursor = 2;
        app.update_autocomplete();
        assert!(app.is_overlay(OverlayKind::Autocomplete));

        // Type a string that matches no command/mention/join.
        app.input.buffer = "/zzznothingmatches".to_string();
        app.input.cursor = app.input.buffer.len();
        app.update_autocomplete();
        assert!(
            !app.is_overlay(OverlayKind::Autocomplete),
            "no-match autocomplete refresh should close the overlay"
        );
    }

    #[rstest]
    fn bell_skipped_for_unaccepted_conversation(mut app: App) {
        app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
        assert!(!app.notifications.pending_bell);
    }

    #[rstest]
    fn bell_skipped_for_blocked_conversation(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        if let Some(conv) = app.store.conversations.get_mut("+1") {
            conv.accepted = true;
        }
        app.blocked_conversations.insert("+1".to_string());
        app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
        assert!(!app.notifications.pending_bell);
    }

    #[rstest]
    fn read_receipts_not_sent_for_unaccepted(mut app: App) {
        app.send_read_receipts = true;
        app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
        app.queue_read_receipts_for_conv("+1", 0);
        assert!(app.pending.read_receipts.is_empty());
    }

    #[rstest]
    fn read_receipts_not_sent_for_blocked(mut app: App) {
        app.send_read_receipts = true;
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        if let Some(conv) = app.store.conversations.get_mut("+1") {
            conv.accepted = true;
        }
        app.blocked_conversations.insert("+1".to_string());
        app.handle_signal_event(SignalEvent::MessageReceived(msg_from("+1")));
        app.queue_read_receipts_for_conv("+1", 0);
        assert!(app.pending.read_receipts.is_empty());
    }

    // --- Block / Unblock tests ---

    #[rstest]
    fn block_adds_to_set_and_returns_send_request(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        app.input.buffer = "/block".to_string();
        let req = app.handle_input();
        assert!(app.blocked_conversations.contains("+1"));
        assert!(
            matches!(req, Some(SendRequest::Block { ref recipient, is_group }) if recipient == "+1" && !is_group)
        );
        assert!(app.status_message.contains("blocked"));
    }

    #[rstest]
    fn unblock_removes_from_set_and_returns_send_request(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        app.blocked_conversations.insert("+1".to_string());
        app.input.buffer = "/unblock".to_string();
        let req = app.handle_input();
        assert!(!app.blocked_conversations.contains("+1"));
        assert!(
            matches!(req, Some(SendRequest::Unblock { ref recipient, is_group }) if recipient == "+1" && !is_group)
        );
        assert!(app.status_message.contains("unblocked"));
    }

    #[rstest]
    #[case("/block", true, "already blocked")]
    #[case("/unblock", false, "not blocked")]
    fn block_unblock_already_in_state(
        mut app: App,
        #[case] cmd: &str,
        #[case] pre_blocked: bool,
        #[case] expected_msg: &str,
    ) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        if pre_blocked {
            app.blocked_conversations.insert("+1".to_string());
        }
        app.input.buffer = cmd.to_string();
        let req = app.handle_input();
        assert!(req.is_none());
        assert!(app.status_message.contains(expected_msg));
    }

    #[rstest]
    #[case("/block", "no active conversation")]
    #[case("/unblock", "no active conversation")]
    fn block_unblock_no_active_conversation(
        mut app: App,
        #[case] cmd: &str,
        #[case] expected_msg: &str,
    ) {
        app.input.buffer = cmd.to_string();
        let req = app.handle_input();
        assert!(req.is_none());
        assert!(app.status_message.contains(expected_msg));
    }

    // --- Mouse support tests ---

    fn mouse_down(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        }
    }

    fn mouse_scroll_up(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        }
    }

    fn mouse_scroll_down(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        }
    }

    #[rstest]
    fn mouse_disabled_ignores_events(mut app: App) {
        app.mouse.enabled = false;
        app.mouse.messages_area = Rect::new(0, 0, 80, 20);
        let result = app.handle_mouse_event(mouse_scroll_up(10, 10));
        assert!(result.is_none());
        assert_eq!(app.scroll.offset, 0);
    }

    #[rstest]
    fn mouse_overlay_scroll_navigates_list(mut app: App) {
        app.open_overlay(OverlayKind::Settings);
        app.settings_index = 0;
        app.mouse.messages_area = Rect::new(0, 0, 80, 20);
        // Scroll down in overlay should navigate settings list (j), not scroll messages
        app.handle_mouse_event(mouse_scroll_down(10, 10));
        assert_eq!(app.settings_index, 1);
        assert_eq!(app.scroll.offset, 0); // messages not scrolled
    }

    #[rstest]
    #[case(0, true, 3)]
    #[case(10, false, 7)]
    #[case(1, false, 0)]
    fn mouse_scroll_behavior(
        mut app: App,
        #[case] initial_offset: usize,
        #[case] scroll_up: bool,
        #[case] expected_offset: usize,
    ) {
        app.mouse.messages_area = Rect::new(0, 0, 80, 20);
        app.scroll.offset = initial_offset;
        let event = if scroll_up {
            mouse_scroll_up(10, 10)
        } else {
            mouse_scroll_down(10, 10)
        };
        app.handle_mouse_event(event);
        assert_eq!(app.scroll.offset, expected_offset);
    }

    #[rstest]
    fn mouse_sidebar_click_switches_conversation(mut app: App) {
        // Create two conversations
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.store
            .get_or_create_conversation("+2", "Bob", false, &app.db);
        app.active_conversation = Some("+1".to_string());

        // Sidebar inner starts at row 0, so clicking row 1 selects the second conv
        app.mouse.sidebar_inner = Some(Rect::new(0, 0, 20, 10));
        app.handle_mouse_event(mouse_down(5, 1));
        assert_eq!(app.active_conversation.as_deref(), Some("+2"));
    }

    #[rstest]
    fn mouse_input_click_positions_cursor(mut app: App) {
        app.mode = InputMode::Normal;
        app.input.buffer = "hello world".to_string();
        app.input.cursor = 0;
        // Input area with borders: x=10, y=20, w=40, h=3
        app.mouse.input_area = Rect::new(10, 20, 40, 3);
        app.mouse.input_prefix_len = 2; // "> "

        // Click at column 18 (inside input area)
        // content_start_col = 10 + 1 + 2 = 13, so click_offset = 18 - 13 = 5
        app.handle_mouse_event(mouse_down(18, 21));
        assert_eq!(app.mode, InputMode::Insert);
        assert_eq!(app.input.cursor, 5);
    }

    #[rstest]
    fn mouse_input_click_handles_multibyte(mut app: App) {
        app.mode = InputMode::Normal;
        app.input.buffer = "caf\u{e9} ok".to_string(); // "café ok" — é is 2 bytes
        app.input.cursor = 0;
        app.mouse.input_area = Rect::new(0, 0, 40, 3);
        app.mouse.input_prefix_len = 2;

        // Click at column 7: content_start = 0+1+2 = 3, target_col = 7-3 = 4
        // Characters: c(1) a(1) f(1) é(2bytes,1col) → 4 chars = 5 bytes
        app.handle_mouse_event(mouse_down(7, 1));
        assert_eq!(app.input.cursor, 5); // byte offset of space after "café"
    }

    /// Toggle an overlay to the requested state. Routes through
    /// `open_overlay`/`close_overlay` so the test mirrors production
    /// callers, and special-cases GroupMenu's sub-state field.
    fn toggle_overlay(app: &mut App, kind: OverlayKind, on: bool) {
        if on {
            app.open_overlay(kind);
        } else if app.is_overlay(kind) {
            app.close_overlay();
        }
        // GroupMenu carries an extra Option<GroupMenuState> sub-state that
        // historically also encoded visibility. Keep it in sync so tests
        // exercising group-menu rendering still see Some(Menu) when open.
        if matches!(kind, OverlayKind::GroupMenu) {
            app.group_menu.state = if on { Some(GroupMenuState::Menu) } else { None };
        }
    }

    const ALL_OVERLAYS: &[OverlayKind] = &[
        OverlayKind::SidebarFilter,
        OverlayKind::PollVote,
        OverlayKind::PinDuration,
        OverlayKind::ActionMenu,
        OverlayKind::DeleteConfirm,
        OverlayKind::FilePicker,
        OverlayKind::EmojiPicker,
        OverlayKind::ReactionPicker,
        OverlayKind::MessageRequest,
        OverlayKind::GroupMenu,
        OverlayKind::About,
        OverlayKind::Profile,
        OverlayKind::Help,
        OverlayKind::Verify,
        OverlayKind::Forward,
        OverlayKind::Contacts,
        OverlayKind::Search,
        OverlayKind::SettingsProfiles,
        OverlayKind::ThemePicker,
        OverlayKind::Keybindings,
        OverlayKind::Customize,
        OverlayKind::Settings,
        OverlayKind::Autocomplete,
    ];

    #[rstest]
    fn active_overlay_covers_every_variant(mut app: App) {
        // Tripwire: `ALL_OVERLAYS` is a hand-maintained slice because Rust has
        // no stable way to enumerate enum variants. Adding a variant without
        // extending this slice would silently skip it; the length check turns
        // that into a loud test failure.
        assert_eq!(
            ALL_OVERLAYS.len(),
            23,
            "ALL_OVERLAYS is out of sync with OverlayKind - update when adding or removing a variant"
        );

        assert_eq!(app.active_overlay(), None);
        assert!(!app.has_overlay());

        for &kind in ALL_OVERLAYS {
            toggle_overlay(&mut app, kind, true);
            assert_eq!(
                app.active_overlay(),
                Some(kind),
                "active_overlay did not match after enabling {kind:?}"
            );
            assert!(app.has_overlay(), "has_overlay returned false for {kind:?}");
            toggle_overlay(&mut app, kind, false);
        }

        assert_eq!(app.active_overlay(), None);
        assert!(!app.has_overlay());
    }

    // --- Vim normal-mode keybinding tests ---

    #[rstest]
    fn gg_scrolls_to_top(mut app: App) {
        for i in 0..20 {
            let msg = make_msg("+1", Some(&format!("msg {i}")), None, false);
            app.handle_signal_event(SignalEvent::MessageReceived(msg));
        }
        app.active_conversation = Some("+1".to_string());
        app.scroll.offset = 0;
        app.mode = InputMode::Normal;

        // First g sets pending
        app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('g'));
        assert_eq!(app.pending_normal_key, Some('g'));

        // Second g scrolls to top
        app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('g'));
        assert_eq!(app.pending_normal_key, None);
        assert_eq!(app.scroll.offset, 20);
    }

    #[rstest]
    fn dd_shows_delete_confirm(mut app: App) {
        let msg = make_msg("+1", Some("hello"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        app.active_conversation = Some("+1".to_string());
        app.mode = InputMode::Normal;

        // First d sets pending
        app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('d'));
        assert_eq!(app.pending_normal_key, Some('d'));
        assert!(!app.is_overlay(OverlayKind::DeleteConfirm));

        // Second d triggers delete confirm
        app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('d'));
        assert_eq!(app.pending_normal_key, None);
        assert!(app.is_overlay(OverlayKind::DeleteConfirm));
    }

    #[rstest]
    fn pending_key_cancelled_by_esc(mut app: App) {
        app.mode = InputMode::Normal;
        app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('g'));
        assert_eq!(app.pending_normal_key, Some('g'));

        app.handle_normal_key(KeyModifiers::NONE, KeyCode::Esc);
        assert_eq!(app.pending_normal_key, None);
    }

    #[rstest]
    fn pending_key_discarded_on_other_key(mut app: App) {
        let msg = make_msg("+1", Some("hello"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        app.active_conversation = Some("+1".to_string());
        app.mode = InputMode::Normal;

        app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('g'));
        assert_eq!(app.pending_normal_key, Some('g'));

        // Pressing 'j' clears pending and processes j normally
        app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('j'));
        assert_eq!(app.pending_normal_key, None);
    }

    #[rstest]
    fn o_preserves_composer_buffer(mut app: App) {
        app.mode = InputMode::Normal;
        app.input.buffer = "hello world".to_string();
        app.input.cursor = 5;

        app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('o'));

        assert_eq!(app.mode, InputMode::Insert);
        assert_eq!(app.input.buffer, "hello world\n");
        assert_eq!(app.input.cursor, 12);
    }

    #[rstest]
    fn jk_focus_messages(mut app: App) {
        for i in 0..5 {
            let msg = make_msg("+1", Some(&format!("msg {i}")), None, false);
            app.handle_signal_event(SignalEvent::MessageReceived(msg));
        }
        app.active_conversation = Some("+1".to_string());
        app.mode = InputMode::Normal;

        // k (FocusPrevMessage) should invoke jump_to_adjacent_message
        app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('k'));
        assert!(app.scroll.focused_index.is_some());
    }

    #[rstest]
    fn pending_key_cleared_on_mode_transition(mut app: App) {
        app.mode = InputMode::Normal;

        // Press g to set pending
        app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('g'));
        assert_eq!(app.pending_normal_key, Some('g'));

        // Press i to enter Insert mode -- pending should be cleared
        app.handle_normal_key(KeyModifiers::NONE, KeyCode::Char('i'));
        assert_eq!(app.pending_normal_key, None);
        assert_eq!(app.mode, InputMode::Insert);
    }

    #[rstest]
    fn ctrl_e_scrolls_without_focus(mut app: App) {
        for i in 0..20 {
            let msg = make_msg("+1", Some(&format!("msg {i}")), None, false);
            app.handle_signal_event(SignalEvent::MessageReceived(msg));
        }
        app.active_conversation = Some("+1".to_string());
        app.mode = InputMode::Normal;
        app.scroll.offset = 5;
        app.scroll.focused_index = Some(10);

        // Ctrl-E (ScrollDown) should scroll viewport and clear focus
        app.handle_normal_key(KeyModifiers::CONTROL, KeyCode::Char('e'));
        assert_eq!(app.scroll.offset, 4);
        assert_eq!(app.scroll.focused_index, None);
    }

    // --- Helper for building a SignalMessage ---

    fn make_msg(
        source: &str,
        body: Option<&str>,
        group_id: Option<&str>,
        is_outgoing: bool,
    ) -> SignalMessage {
        SignalMessage {
            source: source.to_string(),
            source_name: None,
            source_uuid: None,
            timestamp: chrono::Utc::now(),
            body: body.map(|s| s.to_string()),
            attachments: vec![],
            group_id: group_id.map(|s| s.to_string()),
            group_name: None,
            is_outgoing,
            destination: None,
            mentions: vec![],
            text_styles: vec![],
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        }
    }

    /// Like `make_msg` but with an explicit millisecond timestamp offset, so
    /// tests that send multiple messages in quick succession get distinct
    /// timestamps (avoiding the dedup path).
    fn make_msg_with_ts(
        source: &str,
        body: Option<&str>,
        group_id: Option<&str>,
        is_outgoing: bool,
        ts_ms: i64,
    ) -> SignalMessage {
        let mut m = make_msg(source, body, group_id, is_outgoing);
        m.timestamp = chrono::DateTime::from_timestamp_millis(ts_ms).unwrap();
        m
    }

    // --- Typing indicator tests ---

    #[rstest]
    fn typing_indicator_adds_and_removes(mut app: App) {
        app.handle_signal_event(SignalEvent::TypingIndicator {
            sender: "+1".to_string(),
            sender_name: Some("Alice".to_string()),
            is_typing: true,
            group_id: None,
        });
        assert!(app.typing.indicators.contains_key("+1"));
        assert_eq!(app.store.contact_names.get("+1").unwrap(), "Alice");

        app.handle_signal_event(SignalEvent::TypingIndicator {
            sender: "+1".to_string(),
            sender_name: None,
            is_typing: false,
            group_id: None,
        });
        assert!(!app.typing.indicators.contains_key("+1"));
    }

    // --- Error event ---

    #[rstest]
    fn error_event_sets_status(mut app: App) {
        app.handle_signal_event(SignalEvent::Error("connection lost".to_string()));
        assert!(app.status_message.contains("connection lost"));
    }

    // --- Attachment tests ---

    #[rstest]
    fn message_with_image_attachment(mut app: App) {
        let mut msg = make_msg("+1", None, None, false);
        msg.attachments = vec![Attachment {
            id: "a1".to_string(),
            content_type: "image/jpeg".to_string(),
            filename: Some("photo.jpg".to_string()),
            local_path: None,
        }];
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        let conv = &app.store.conversations["+1"];
        assert!(
            conv.messages
                .iter()
                .any(|m| m.body.contains("[image: photo.jpg]"))
        );
    }

    #[rstest]
    fn message_with_non_image_attachment(mut app: App) {
        let mut msg = make_msg("+1", None, None, false);
        msg.attachments = vec![Attachment {
            id: "a1".to_string(),
            content_type: "application/pdf".to_string(),
            filename: Some("doc.pdf".to_string()),
            local_path: None,
        }];
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        let conv = &app.store.conversations["+1"];
        assert!(
            conv.messages
                .iter()
                .any(|m| m.body.contains("[attachment: doc.pdf]"))
        );
    }

    #[rstest]
    fn message_with_body_and_attachment(mut app: App) {
        let mut msg = make_msg("+1", Some("look at this"), None, false);
        msg.attachments = vec![Attachment {
            id: "a1".to_string(),
            content_type: "image/png".to_string(),
            filename: Some("img.png".to_string()),
            local_path: None,
        }];
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        let conv = &app.store.conversations["+1"];
        // Should have 2 display messages: text body + attachment
        assert_eq!(conv.messages.len(), 2);
        assert!(conv.messages[0].body.contains("look at this"));
        assert!(conv.messages[1].body.contains("[image: img.png]"));
    }

    #[rstest]
    fn attachment_without_filename_uses_content_type(mut app: App) {
        let mut msg = make_msg("+1", None, None, false);
        msg.attachments = vec![Attachment {
            id: "a1".to_string(),
            content_type: "audio/ogg".to_string(),
            filename: None,
            local_path: None,
        }];
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        let conv = &app.store.conversations["+1"];
        assert!(
            conv.messages
                .iter()
                .any(|m| m.body.contains("[attachment: audio/ogg]"))
        );
    }

    // --- Bell / notification tests ---

    #[rstest]
    fn bell_rings_for_background_dm(mut app: App) {
        app.sync.active = false;
        // "+1" must be a known contact so conversation is accepted
        app.store
            .contact_names
            .insert("+1".to_string(), "Alice".to_string());
        app.store
            .get_or_create_conversation("+other", "Other", false, &app.db);
        app.active_conversation = Some("+other".to_string());
        app.notifications.notify_direct = true;

        let msg = make_msg("+1", Some("hey"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert!(app.notifications.pending_bell);
    }

    #[rstest]
    fn bell_not_set_for_active_conversation(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        app.notifications.notify_direct = true;

        let msg = make_msg("+1", Some("hey"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert!(!app.notifications.pending_bell);
    }

    #[rstest]
    fn bell_skipped_when_notify_disabled(mut app: App) {
        app.store
            .get_or_create_conversation("+other", "Other", false, &app.db);
        app.active_conversation = Some("+other".to_string());
        app.notifications.notify_direct = false;

        let msg = make_msg("+1", Some("hey"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert!(!app.notifications.pending_bell);
    }

    #[rstest]
    fn bell_for_group_respects_setting(mut app: App) {
        app.sync.active = false;
        app.handle_signal_event(SignalEvent::GroupList(vec![Group {
            id: "g1".to_string(),
            name: "Team".to_string(),
            members: vec![],
            member_uuids: vec![],
        }]));
        app.store
            .get_or_create_conversation("+other", "Other", false, &app.db);
        app.active_conversation = Some("+other".to_string());

        // group notifications enabled
        app.notifications.notify_group = true;
        let msg = make_msg("+1", Some("hi team"), Some("g1"), false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert!(app.notifications.pending_bell);

        // reset and disable
        app.notifications.pending_bell = false;
        app.notifications.notify_group = false;
        let msg2 = make_msg("+2", Some("again"), Some("g1"), false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg2));
        assert!(!app.notifications.pending_bell);
    }

    // --- Unread count tests ---

    #[rstest]
    fn unread_increments_for_background(mut app: App) {
        // No active conversation
        app.active_conversation = None;
        let msg = make_msg("+1", Some("hey"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert_eq!(app.store.conversations["+1"].unread, 1);
    }

    #[rstest]
    fn unread_no_increment_for_active(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        let msg = make_msg("+1", Some("hey"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert_eq!(app.store.conversations["+1"].unread, 0);
    }

    // --- Read receipt tests ---

    #[rstest]
    fn active_conv_queues_read_receipt(mut app: App) {
        app.sync.active = false;
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        app.send_read_receipts = true;

        let msg = make_msg("+1", Some("hey"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert!(
            !app.pending.read_receipts.is_empty(),
            "expected read receipt to be queued"
        );
        let (recipient, _) = &app.pending.read_receipts[0];
        assert_eq!(recipient, "+1");
    }

    // --- Expiration timer sync ---

    #[rstest]
    fn handle_message_syncs_expiration_timer(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        assert_eq!(app.store.conversations["+1"].expiration_timer, 0);

        let mut msg = make_msg("+1", Some("secret"), None, false);
        msg.expires_in_seconds = 3600;
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert_eq!(app.store.conversations["+1"].expiration_timer, 3600);
    }

    // --- Paste command tests ---

    #[rstest]
    fn paste_text_inserts_into_composer(mut app: App) {
        // handle_paste_text delegates to handle_paste for plain text, which guards on Insert mode
        app.mode = InputMode::Insert;
        app.active_conversation = Some("test-conv".to_string());
        app.handle_paste_text("hello world");
        assert_eq!(app.input.buffer, "hello world");
    }

    #[rstest]
    fn paste_file_path_inserts_as_text(mut app: App) {
        // File paths in clipboard text are treated as plain text, not auto-attached
        let path = format!("{}/Cargo.toml", env!("CARGO_MANIFEST_DIR"));
        app.mode = InputMode::Insert;
        app.active_conversation = Some("test-conv".to_string());
        app.handle_paste_text(&path);
        assert!(app.pending_attachment.is_none());
        assert_eq!(app.input.buffer, path);
    }

    #[rstest]
    fn paste_empty_text_shows_status_message(mut app: App) {
        app.active_conversation = Some("test-conv".to_string());
        app.handle_paste_text("   ");
        assert!(app.status_message.contains("empty"));
        assert!(app.pending_attachment.is_none());
        assert!(app.input.buffer.is_empty());
    }

    #[rstest]
    fn paste_clipboard_image_saves_png_as_attachment(mut app: App) {
        let img_data = arboard::ImageData {
            width: 2,
            height: 2,
            bytes: std::borrow::Cow::Owned(vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ]),
        };

        app.active_conversation = Some("test-conv".to_string());
        app.handle_clipboard_image(img_data);

        assert!(app.pending_attachment.is_some());
        let path = app.pending_attachment.as_ref().unwrap();
        assert!(path.exists(), "PNG file should have been written to disk");
        assert!(path.to_string_lossy().contains("clipboard_"));
        assert!(path.extension().is_some_and(|e| e == "png"));

        // Clean up
        let _ = std::fs::remove_file(path);
    }

    #[rstest]
    fn paste_command_without_active_conversation_sets_error(mut app: App) {
        // active_conversation is None by default in test fixture
        app.handle_paste_command();
        assert!(app.status_message.contains("No active conversation"));
    }

    // --- Typing indicator scoping ---

    #[rstest]
    fn group_typing_indicator_keyed_by_group_not_sender(mut app: App) {
        // Alice types in group-a. The typing indicator must be stored under
        // "group-a", not under Alice's phone number.
        app.handle_signal_event(SignalEvent::TypingIndicator {
            sender: "+1".to_string(),
            sender_name: Some("Alice".to_string()),
            is_typing: true,
            group_id: Some("group-a".to_string()),
        });

        assert!(
            app.typing.indicators.contains_key("group-a"),
            "typing indicator should be keyed by group ID"
        );
        assert!(
            !app.typing.indicators.contains_key("+1"),
            "typing indicator must NOT be keyed by sender phone"
        );
        // Inner map stores the sender phone so we can resolve the display name
        assert!(app.typing.indicators["group-a"].contains_key("+1"));
    }

    #[rstest]
    fn group_typing_does_not_bleed_into_other_group(mut app: App) {
        // Alice types in group-a. Viewing group-b must show no typing indicator.
        app.store
            .get_or_create_conversation("group-a", "Group A", true, &app.db);
        app.store
            .get_or_create_conversation("group-b", "Group B", true, &app.db);

        app.handle_signal_event(SignalEvent::TypingIndicator {
            sender: "+1".to_string(),
            sender_name: Some("Alice".to_string()),
            is_typing: true,
            group_id: Some("group-a".to_string()),
        });

        // Viewing group-b: no indicator should be visible for it
        assert!(
            !app.typing.indicators.contains_key("group-b"),
            "group-a typing must not bleed into group-b"
        );
    }

    #[rstest]
    fn direct_typing_indicator_keyed_by_sender(mut app: App) {
        // 1:1 typing (no group_id) must still be keyed by sender phone number.
        app.handle_signal_event(SignalEvent::TypingIndicator {
            sender: "+1".to_string(),
            sender_name: None,
            is_typing: true,
            group_id: None,
        });

        assert!(
            app.typing.indicators.contains_key("+1"),
            "1:1 typing indicator should be keyed by sender phone"
        );
    }

    #[rstest]
    fn concurrent_typers_in_group(mut app: App) {
        // Alice and Bob both type in the same group
        app.handle_signal_event(SignalEvent::TypingIndicator {
            sender: "+1".to_string(),
            sender_name: Some("Alice".to_string()),
            is_typing: true,
            group_id: Some("group-a".to_string()),
        });
        app.handle_signal_event(SignalEvent::TypingIndicator {
            sender: "+2".to_string(),
            sender_name: Some("Bob".to_string()),
            is_typing: true,
            group_id: Some("group-a".to_string()),
        });

        let senders = &app.typing.indicators["group-a"];
        assert_eq!(senders.len(), 2);
        assert!(senders.contains_key("+1"));
        assert!(senders.contains_key("+2"));

        // Alice stops typing
        app.handle_signal_event(SignalEvent::TypingIndicator {
            sender: "+1".to_string(),
            sender_name: None,
            is_typing: false,
            group_id: Some("group-a".to_string()),
        });

        let senders = &app.typing.indicators["group-a"];
        assert_eq!(senders.len(), 1);
        assert!(senders.contains_key("+2"));

        // Bob stops typing — entry should be fully removed
        app.handle_signal_event(SignalEvent::TypingIndicator {
            sender: "+2".to_string(),
            sender_name: None,
            is_typing: false,
            group_id: Some("group-a".to_string()),
        });

        assert!(!app.typing.indicators.contains_key("group-a"));
    }

    #[test]
    fn is_stale_filters_correctly() {
        let empty_group = Conversation {
            name: "abc123groupid".to_string(),
            id: "abc123groupid".to_string(),
            messages: vec![],
            unread: 0,
            is_group: true,
            expiration_timer: 0,
            accepted: true,
        };
        assert!(
            empty_group.is_stale(),
            "group with no messages and name==id is stale"
        );

        let named_group = Conversation {
            name: "Book Club".to_string(),
            id: "abc123groupid".to_string(),
            messages: vec![],
            unread: 0,
            is_group: true,
            expiration_timer: 0,
            accepted: true,
        };
        assert!(
            !named_group.is_stale(),
            "group with a real name is not stale"
        );

        let phone_contact = Conversation {
            name: "+15551234567".to_string(),
            id: "+15551234567".to_string(),
            messages: vec![],
            unread: 0,
            is_group: false,
            expiration_timer: 0,
            accepted: true,
        };
        assert!(
            !phone_contact.is_stale(),
            "contact with phone number is not stale"
        );

        let uuid_contact = Conversation {
            name: "8eb3dbda-1234-5678".to_string(),
            id: "8eb3dbda-1234-5678".to_string(),
            messages: vec![],
            unread: 0,
            is_group: false,
            expiration_timer: 0,
            accepted: true,
        };
        assert!(
            uuid_contact.is_stale(),
            "contact with UUID-only name is stale"
        );
    }

    #[test]
    fn extract_file_uri_from_image_body() {
        let body = "[image: photo.jpg](file:///home/user/photo.jpg)";
        assert_eq!(
            extract_file_uri(body),
            Some("file:///home/user/photo.jpg".to_string())
        );
    }

    #[test]
    fn extract_file_uri_from_attachment_body() {
        let body = "[attachment: doc.pdf](file:///home/user/doc.pdf)";
        assert_eq!(
            extract_file_uri(body),
            Some("file:///home/user/doc.pdf".to_string())
        );
    }

    #[test]
    fn extract_file_uri_none_for_plain_text() {
        assert_eq!(extract_file_uri("hello world"), None);
    }

    #[test]
    fn extract_http_url_from_body() {
        let body = "check this out https://example.com/page and more text";
        assert_eq!(
            extract_http_url(body),
            Some("https://example.com/page".to_string())
        );
    }

    #[test]
    fn extract_http_url_skips_file_uri() {
        let body = "[image: photo.jpg](file:///home/user/photo.jpg) see https://example.com";
        assert_eq!(
            extract_http_url(body),
            Some("https://example.com".to_string())
        );
    }

    #[test]
    fn extract_http_url_none_for_plain_text() {
        assert_eq!(extract_http_url("hello world"), None);
    }

    #[test]
    fn extract_http_url_with_trailing_paren() {
        let body = "link (https://example.com/path) here";
        assert_eq!(
            extract_http_url(body),
            Some("https://example.com/path".to_string())
        );
    }

    // --- Action menu item tests ---

    #[rstest]
    fn action_menu_shows_open_attachment(mut app: App) {
        let mut msg = make_msg("+1", None, None, false);
        msg.attachments = vec![Attachment {
            id: "123".to_string(),
            content_type: "application/pdf".to_string(),
            filename: Some("doc.pdf".to_string()),
            local_path: Some("/tmp/doc.pdf".to_string()),
        }];
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        app.active_conversation = Some("+1".to_string());
        let items = app.action_menu_items();
        assert!(
            items.iter().any(|a| a.label == "Open attachment"),
            "expected Open attachment in menu"
        );
    }

    #[rstest]
    fn action_menu_shows_open_link(mut app: App) {
        let msg = make_msg("+1", Some("check https://example.com"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        app.active_conversation = Some("+1".to_string());
        let items = app.action_menu_items();
        assert!(
            items.iter().any(|a| a.label == "Open link"),
            "expected Open link in menu"
        );
    }

    #[rstest]
    fn action_menu_shows_both_open_items(mut app: App) {
        // Send a message with a URL body and an attachment with a local path.
        // The body and attachment are stored as separate DisplayMessages with the
        // same timestamp; body is at index 0, attachment at index 1.
        // Focus index 0 (the body message) to verify "Open link" appears.
        let mut msg = make_msg("+1", Some("see https://example.com"), None, false);
        msg.attachments = vec![Attachment {
            id: "456".to_string(),
            content_type: "image/png".to_string(),
            filename: Some("photo.png".to_string()),
            local_path: Some("/tmp/photo.png".to_string()),
        }];
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        app.active_conversation = Some("+1".to_string());
        // Index 0 is the body message ("see https://example.com")
        app.scroll.focused_index = Some(0);
        let items_body = app.action_menu_items();
        assert!(
            items_body.iter().any(|a| a.label == "Open link"),
            "expected Open link for body message"
        );
        // Index 1 is the attachment message ("[image: photo.png](file:///tmp/photo.png)")
        app.scroll.focused_index = Some(1);
        let items_att = app.action_menu_items();
        assert!(
            items_att.iter().any(|a| a.label == "Open attachment"),
            "expected Open attachment for attachment message"
        );
    }

    #[rstest]
    fn action_menu_no_open_for_plain_text(mut app: App) {
        let msg = make_msg("+1", Some("just a regular message"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        app.active_conversation = Some("+1".to_string());
        let items = app.action_menu_items();
        assert!(
            !items.iter().any(|a| a.label == "Open attachment"),
            "should not have Open attachment"
        );
        assert!(
            !items.iter().any(|a| a.label == "Open link"),
            "should not have Open link"
        );
    }

    #[rstest]
    fn action_menu_respects_focused_index(mut app: App) {
        // Message 0: has a URL
        let msg1 = make_msg("+1", Some("check https://example.com"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg1));
        // Message 1: plain text (last/newest)
        let msg2 = make_msg("+1", Some("just text"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg2));
        app.active_conversation = Some("+1".to_string());

        // Without focus, defaults to last message (plain text) — no Open link
        let items = app.action_menu_items();
        assert!(
            !items.iter().any(|a| a.label == "Open link"),
            "last msg has no URL"
        );

        // Focus the first message (the one with a URL)
        app.scroll.focused_index = Some(0);
        let items = app.action_menu_items();
        assert!(
            items.iter().any(|a| a.label == "Open link"),
            "focused msg has URL, should show Open link"
        );
    }

    // --- SyncState tests ---

    #[rstest]
    fn sync_starts_active(app: App) {
        assert!(app.sync.active);
        assert_eq!(app.sync.message_count, 0);
        assert!(!app.sync.user_scrolled);
    }

    #[rstest]
    fn sync_should_end_requires_quiet_and_min_elapsed(mut app: App) {
        assert!(!app.sync.should_end());
        app.sync.started_at = Instant::now() - std::time::Duration::from_secs(15);
        assert!(app.sync.should_end());
        app.sync.last_message_time = Some(Instant::now());
        assert!(!app.sync.should_end());
        app.sync.last_message_time = Some(Instant::now() - std::time::Duration::from_secs(5));
        assert!(app.sync.should_end());
    }

    #[rstest]
    fn sync_suppresses_notifications(mut app: App) {
        assert!(app.sync.active);
        app.store
            .contact_names
            .insert("+1".to_string(), "Alice".to_string());
        app.store
            .get_or_create_conversation("+other", "Other", false, &app.db);
        app.active_conversation = Some("+other".to_string());
        app.notifications.notify_direct = true;
        let msg = make_msg("+1", Some("hello"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert!(!app.notifications.pending_bell);
        assert_eq!(
            app.sync
                .suppressed_notifications
                .get("+1")
                .copied()
                .unwrap_or(0),
            1
        );
        assert!(app.sync.message_count > 0);
    }

    #[rstest]
    fn notifications_fire_after_sync_ends(mut app: App) {
        app.sync.active = false;
        app.store
            .contact_names
            .insert("+1".to_string(), "Alice".to_string());
        app.store
            .get_or_create_conversation("+other", "Other", false, &app.db);
        app.active_conversation = Some("+other".to_string());
        app.notifications.notify_direct = true;
        let msg = make_msg("+1", Some("hello"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert!(app.notifications.pending_bell);
    }

    #[rstest]
    fn sync_captures_pin_on_first_message(mut app: App) {
        // Conversation has a prior message (so there's something to anchor to).
        // First sync arrival captures the pin to that message's timestamp + the
        // current scroll.offset.
        assert!(app.sync.active);
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        let prior = make_msg_with_ts("+1", Some("prior message"), None, false, 1000);
        app.handle_signal_event(SignalEvent::MessageReceived(prior));
        let pinned_ts = app.store.conversations["+1"]
            .messages
            .last()
            .unwrap()
            .timestamp;
        // Set a non-zero offset to verify it's captured rather than coerced to 0.
        app.scroll.offset = 4;

        let msg = make_msg_with_ts("+1", Some("first sync msg"), None, false, 2000);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        let (pin_ts, pin_offset) = app
            .sync
            .pin
            .expect("pin should be captured on first sync msg");
        assert_eq!(pin_ts, pinned_ts);
        assert_eq!(pin_offset, 4);
    }

    #[rstest]
    fn sync_pin_only_captured_once(mut app: App) {
        // Subsequent sync messages must NOT overwrite the pin -- it anchors
        // to the user's view at sync start, not the most recent arrival.
        assert!(app.sync.active);
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        let prior = make_msg_with_ts("+1", Some("prior"), None, false, 1000);
        app.handle_signal_event(SignalEvent::MessageReceived(prior));
        let pinned_ts = app.store.conversations["+1"]
            .messages
            .last()
            .unwrap()
            .timestamp;

        let msg1 = make_msg_with_ts("+1", Some("first"), None, false, 2000);
        app.handle_signal_event(SignalEvent::MessageReceived(msg1));
        let msg2 = make_msg_with_ts("+1", Some("second"), None, false, 3000);
        app.handle_signal_event(SignalEvent::MessageReceived(msg2));

        let (pin_ts, _) = app.sync.pin.expect("pin should still be set");
        assert_eq!(
            pin_ts, pinned_ts,
            "pin must still anchor to the original message"
        );
    }

    #[rstest]
    fn sync_does_not_capture_pin_after_user_scroll(mut app: App) {
        assert!(app.sync.active);
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        let prior = make_msg_with_ts("+1", Some("prior"), None, false, 1000);
        app.handle_signal_event(SignalEvent::MessageReceived(prior));
        app.sync.user_scrolled = true;

        let msg = make_msg_with_ts("+1", Some("sync"), None, false, 2000);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        assert!(app.sync.pin.is_none());
        // Old behavior: scroll.offset += 1 was suppressed by user_scrolled.
        // New behavior preserves that property — scroll.offset stays put.
        assert_eq!(app.scroll.offset, 0);
    }

    #[rstest]
    fn sync_pin_skips_non_active_conversation(mut app: App) {
        // Pin is per-active-conversation. Sync messages for a non-active conv
        // must not capture or overwrite the active conv's pin.
        assert!(app.sync.active);
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.store
            .get_or_create_conversation("+2", "Bob", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        let prior_other = make_msg_with_ts("+2", Some("prior"), None, false, 1000);
        app.handle_signal_event(SignalEvent::MessageReceived(prior_other));

        let msg = make_msg_with_ts("+2", Some("from non-active"), None, false, 2000);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        assert!(
            app.sync.pin.is_none(),
            "sync messages for the non-active conversation must not capture pin"
        );
    }

    #[rstest]
    fn sync_pin_cleared_on_end_sync(mut app: App) {
        app.sync.pin = Some((Utc::now(), 3));
        app.end_sync();
        assert!(app.sync.pin.is_none());
    }

    #[rstest]
    fn sync_pin_cleared_on_conv_switch(mut app: App) {
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.store
            .get_or_create_conversation("+2", "Bob", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        app.sync.pin = Some((Utc::now(), 2));
        app.next_conversation();
        assert!(
            app.sync.pin.is_none(),
            "switching conversations must clear the pin so it doesn't apply to the new conv"
        );
    }

    #[rstest]
    fn sync_does_not_advance_read_index_for_active_conv(mut app: App) {
        assert!(app.sync.active);
        app.store
            .get_or_create_conversation("+1", "Alice", false, &app.db);
        app.active_conversation = Some("+1".to_string());
        let initial_read = app.store.last_read_index.get("+1").copied().unwrap_or(0);
        let msg = make_msg("+1", Some("hello"), None, false);
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        let after_read = app.store.last_read_index.get("+1").copied().unwrap_or(0);
        assert_eq!(
            initial_read, after_read,
            "read index should not advance during sync"
        );
    }

    #[rstest]
    fn end_sync_snaps_to_bottom_and_fires_bell(mut app: App) {
        app.sync.active = true;
        app.sync.message_count = 50;
        app.scroll.offset = 30;
        app.sync
            .suppressed_notifications
            .insert("+1".to_string(), 10);
        app.sync
            .suppressed_notifications
            .insert("+2".to_string(), 5);
        app.end_sync();
        assert!(!app.sync.active);
        assert_eq!(app.scroll.offset, 0);
        assert!(app.notifications.pending_bell);
        assert!(app.sync.suppressed_notifications.is_empty());
    }

    #[rstest]
    fn end_sync_respects_user_scroll(mut app: App) {
        app.sync.active = true;
        app.scroll.offset = 15;
        app.sync.user_scrolled = true;
        app.end_sync();
        assert!(!app.sync.active);
        assert_eq!(app.scroll.offset, 15);
    }

    #[rstest]
    fn end_sync_no_bell_when_no_suppressed(mut app: App) {
        app.sync.active = true;
        app.sync.message_count = 5;
        app.end_sync();
        assert!(!app.sync.active);
        assert!(!app.notifications.pending_bell);
    }
}
