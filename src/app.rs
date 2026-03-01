use chrono::{DateTime, Local, Utc};
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::text::Line;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use crate::db::Database;
use crate::image_render;
use crate::image_render::ImageProtocol;
use crate::input::{self, InputAction, COMMANDS};
use crate::signal::types::{Contact, Group, MessageStatus, Reaction, SignalEvent, SignalMessage};

/// Log a database error via debug_log (no-op when --debug is off).
fn db_warn<T>(result: Result<T, impl std::fmt::Display>, context: &str) {
    if let Err(e) = result {
        crate::debug_log::logf(format_args!("db {context}: {e}"));
    }
}

/// An image visible on screen, for native protocol overlay rendering.
pub struct VisibleImage {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Insert,
}

/// A single displayed message in a conversation
#[derive(Debug, Clone)]
pub struct DisplayMessage {
    pub sender: String,
    pub timestamp: DateTime<Utc>,
    pub body: String,
    pub is_system: bool,
    /// Pre-rendered halfblock image lines (for image attachments)
    pub image_lines: Option<Vec<Line<'static>>>,
    /// Local filesystem path for native protocol rendering (Kitty/iTerm2)
    pub image_path: Option<String>,
    /// Delivery/read status (Some for outgoing, None for incoming)
    pub status: Option<MessageStatus>,
    /// Millisecond epoch timestamp for receipt matching
    pub timestamp_ms: i64,
    /// Emoji reactions on this message
    pub reactions: Vec<Reaction>,
}

impl DisplayMessage {
    pub fn format_time(&self) -> String {
        let local: DateTime<Local> = self.timestamp.with_timezone(&Local);
        local.format("%H:%M").to_string()
    }
}

/// A conversation (1:1 or group)
#[derive(Debug, Clone)]
pub struct Conversation {
    /// Display name (contact name/number or group name)
    pub name: String,
    /// Unique key — phone number for 1:1, group ID for groups
    pub id: String,
    pub messages: Vec<DisplayMessage>,
    pub unread: usize,
    pub is_group: bool,
}

/// Application state
pub struct App {
    pub conversations: HashMap<String, Conversation>,
    /// Ordered list of conversation IDs for sidebar display
    pub conversation_order: Vec<String>,
    /// Currently selected conversation ID
    pub active_conversation: Option<String>,
    /// Text input buffer
    pub input_buffer: String,
    /// Cursor position in input buffer
    pub input_cursor: usize,
    /// Previously submitted inputs for Up/Down recall
    pub input_history: Vec<String>,
    /// Current position in history (None = not browsing)
    pub history_index: Option<usize>,
    /// Saves in-progress input when browsing history
    pub history_draft: String,
    /// Whether sidebar is visible
    pub sidebar_visible: bool,
    /// Scroll offset for messages (0 = bottom)
    pub scroll_offset: usize,
    /// Status bar message
    pub status_message: String,
    /// Whether the app should quit
    pub should_quit: bool,
    /// Our own account number for identifying outgoing messages
    #[allow(dead_code)]
    pub account: String,
    /// Resizable sidebar width (min 14, max 40)
    pub sidebar_width: u16,
    /// Per-conversation typing indicators with expiry timestamp
    pub typing_indicators: HashMap<String, Instant>,
    /// Last-read message index per conversation (for unread marker)
    pub last_read_index: HashMap<String, usize>,
    /// Whether we are connected to signal-cli
    pub connected: bool,
    /// Current input mode (Normal or Insert)
    pub mode: InputMode,
    /// SQLite database for persistent storage
    pub db: Database,
    /// Persistent error from signal-cli connection failure
    pub connection_error: Option<String>,
    /// Contact/group name lookup (number/id → display name) for name resolution
    pub contact_names: HashMap<String, String>,
    /// Bell pending — set by handle_message, drained by main loop
    pub pending_bell: bool,
    /// Terminal bell for 1:1 messages in background conversations
    pub notify_direct: bool,
    /// Terminal bell for group messages in background conversations
    pub notify_group: bool,
    /// Conversations muted from notifications
    pub muted_conversations: HashSet<String>,
    /// Autocomplete popup visible
    pub autocomplete_visible: bool,
    /// Indices into COMMANDS for current matches
    pub autocomplete_candidates: Vec<usize>,
    /// Selected item in autocomplete popup
    pub autocomplete_index: usize,
    /// Settings overlay visible
    pub show_settings: bool,
    /// Cursor position in settings list
    pub settings_index: usize,
    /// Help overlay visible
    pub show_help: bool,
    /// Contacts overlay visible
    pub show_contacts: bool,
    /// Cursor position in contacts list
    pub contacts_index: usize,
    /// Type-to-filter text for contacts overlay
    pub contacts_filter: String,
    /// Filtered list of (phone_number, display_name) for contacts overlay
    pub contacts_filtered: Vec<(String, String)>,
    /// Show inline halfblock image previews in chat
    pub inline_images: bool,
    /// Link regions detected in the last rendered frame (for OSC 8 injection)
    pub link_regions: Vec<crate::ui::LinkRegion>,
    /// Maps display text → hidden URL for attachment links (cleared each frame)
    pub link_url_map: HashMap<String, String>,
    /// Detected terminal image protocol (Kitty, iTerm2, or Halfblock)
    pub image_protocol: ImageProtocol,
    /// Images visible on screen for native protocol overlay (cleared each frame)
    pub visible_images: Vec<VisibleImage>,
    /// Experimental: use native terminal image protocols (Kitty/iTerm2) instead of halfblock
    pub native_images: bool,
    /// Cache of base64-encoded pre-resized PNGs for native protocol (path → base64)
    pub native_image_cache: HashMap<String, String>,
    /// Previous active conversation ID, for detecting chat switches
    pub prev_active_conversation: Option<String>,
    /// Incognito mode — in-memory DB, no local persistence
    pub incognito: bool,
    /// Show delivery/read receipt status symbols on outgoing messages
    pub show_receipts: bool,
    /// Use colored status symbols (vs monochrome DarkGray)
    pub color_receipts: bool,
    /// Use Nerd Font glyphs for status symbols
    pub nerd_fonts: bool,
    /// Pending send RPCs: rpc_id → (conv_id, local_timestamp_ms)
    pub pending_sends: HashMap<String, (String, i64)>,
    /// Receipts that arrived before their matching SendTimestamp — replayed after each SendTimestamp
    pub pending_receipts: Vec<(String, String, Vec<i64>)>,
    /// Timestamp of the message at the scroll cursor (set during draw, cleared at scroll_offset=0)
    pub focused_message_time: Option<DateTime<Utc>>,
    /// Reaction picker overlay visible
    pub show_reaction_picker: bool,
    /// Selected index in the reaction picker
    pub reaction_picker_index: usize,
    /// Show verbose reaction display (usernames instead of counts)
    pub reaction_verbose: bool,
}

pub const QUICK_REACTIONS: &[&str] = &["\u{1f44d}", "\u{1f44e}", "\u{2764}\u{fe0f}", "\u{1f602}", "\u{1f62e}", "\u{1f622}", "\u{1f64f}", "\u{1f525}"];

/// A request from the UI to the main loop to send something.
pub enum SendRequest {
    Message {
        recipient: String,
        body: String,
        is_group: bool,
        local_ts_ms: i64,
    },
    Reaction {
        conv_id: String,
        emoji: String,
        is_group: bool,
        target_author: String,
        target_timestamp: i64,
        remove: bool,
    },
}

/// A single settings toggle entry: label, getter, setter, and optional config persistence.
pub struct SettingDef {
    pub label: &'static str,
    get: fn(&App) -> bool,
    set: fn(&mut App, bool),
    save: Option<fn(&mut crate::config::Config, bool)>,
    on_toggle: Option<fn(&mut App)>,
}

pub const SETTINGS: &[SettingDef] = &[
    SettingDef {
        label: "Direct message notifications",
        get: |a| a.notify_direct,
        set: |a, v| a.notify_direct = v,
        save: Some(|c, v| c.notify_direct = v),
        on_toggle: None,
    },
    SettingDef {
        label: "Group message notifications",
        get: |a| a.notify_group,
        set: |a, v| a.notify_group = v,
        save: Some(|c, v| c.notify_group = v),
        on_toggle: None,
    },
    SettingDef {
        label: "Sidebar visible",
        get: |a| a.sidebar_visible,
        set: |a, v| a.sidebar_visible = v,
        save: None, // runtime-only, not persisted
        on_toggle: None,
    },
    SettingDef {
        label: "Inline image previews",
        get: |a| a.inline_images,
        set: |a, v| a.inline_images = v,
        save: Some(|c, v| c.inline_images = v),
        on_toggle: Some(|a| a.refresh_image_previews()),
    },
    SettingDef {
        label: "Native images (experimental)",
        get: |a| a.native_images,
        set: |a, v| a.native_images = v,
        save: Some(|c, v| c.native_images = v),
        on_toggle: None,
    },
    SettingDef {
        label: "Read receipts",
        get: |a| a.show_receipts,
        set: |a, v| a.show_receipts = v,
        save: Some(|c, v| c.show_receipts = v),
        on_toggle: None,
    },
    SettingDef {
        label: "Receipt colors",
        get: |a| a.color_receipts,
        set: |a, v| a.color_receipts = v,
        save: Some(|c, v| c.color_receipts = v),
        on_toggle: None,
    },
    SettingDef {
        label: "Nerd Font icons",
        get: |a| a.nerd_fonts,
        set: |a, v| a.nerd_fonts = v,
        save: Some(|c, v| c.nerd_fonts = v),
        on_toggle: None,
    },
    SettingDef {
        label: "Verbose reactions",
        get: |a| a.reaction_verbose,
        set: |a, v| a.reaction_verbose = v,
        save: Some(|c, v| c.reaction_verbose = v),
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
        let mut config = crate::config::Config::load(None).unwrap_or_default();
        config.account = self.account.clone();
        for def in SETTINGS {
            if let Some(save_fn) = def.save {
                save_fn(&mut config, (def.get)(self));
            }
        }
        if let Err(e) = config.save() {
            crate::debug_log::logf(format_args!("settings save error: {e}"));
        }
    }

    /// Re-render or clear image previews on all messages (after toggling inline_images).
    fn refresh_image_previews(&mut self) {
        for conv in self.conversations.values_mut() {
            for msg in &mut conv.messages {
                if msg.body.starts_with("[image:") {
                    if self.inline_images {
                        // Re-render from stored path
                        if let Some(ref p) = msg.image_path {
                            msg.image_lines = image_render::render_image(Path::new(p), 40);
                        }
                    } else {
                        msg.image_lines = None;
                    }
                }
            }
        }
    }

    /// Handle a key press while the settings overlay is open.
    pub fn handle_settings_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.settings_index < SETTINGS.len() - 1 {
                    self.settings_index += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.settings_index = self.settings_index.saturating_sub(1);
            }
            KeyCode::Char(' ') | KeyCode::Enter | KeyCode::Tab => {
                self.toggle_setting(self.settings_index);
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.show_settings = false;
                self.save_settings();
            }
            _ => {}
        }
    }

    /// Build the filtered contacts list from contact_names using the current filter.
    pub fn refresh_contacts_filter(&mut self) {
        let filter_lower = self.contacts_filter.to_lowercase();
        let mut contacts: Vec<(String, String)> = self
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
        contacts.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
        self.contacts_filtered = contacts;
        // Clamp index
        if self.contacts_filtered.is_empty() {
            self.contacts_index = 0;
        } else if self.contacts_index >= self.contacts_filtered.len() {
            self.contacts_index = self.contacts_filtered.len() - 1;
        }
    }

    /// Handle a key press while the reaction picker overlay is open.
    fn handle_reaction_picker_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        match code {
            KeyCode::Char('h') | KeyCode::Left => {
                self.reaction_picker_index = self.reaction_picker_index.saturating_sub(1);
                None
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if self.reaction_picker_index < QUICK_REACTIONS.len() - 1 {
                    self.reaction_picker_index += 1;
                }
                None
            }
            KeyCode::Char(c @ '1'..='8') => {
                let idx = (c as u8 - b'1') as usize;
                if idx < QUICK_REACTIONS.len() {
                    self.reaction_picker_index = idx;
                    self.show_reaction_picker = false;
                    self.prepare_reaction_send()
                } else {
                    None
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.show_reaction_picker = false;
                self.prepare_reaction_send()
            }
            KeyCode::Esc => {
                self.show_reaction_picker = false;
                None
            }
            _ => None,
        }
    }

    /// Build a SendRequest::Reaction from the current picker selection and focused message.
    fn prepare_reaction_send(&mut self) -> Option<SendRequest> {
        let emoji = QUICK_REACTIONS.get(self.reaction_picker_index)?.to_string();
        let conv_id = self.active_conversation.clone()?;
        let conv = self.conversations.get(&conv_id)?;
        let is_group = conv.is_group;

        let total = conv.messages.len();
        let index = total.saturating_sub(1).saturating_sub(self.scroll_offset);
        let msg = conv.messages.get(index)?;

        let target_timestamp = msg.timestamp_ms;
        let target_author = if msg.sender == "you" {
            self.account.clone()
        } else {
            // Reverse lookup: find the phone number for this display name
            self.contact_names
                .iter()
                .find(|(_, name)| name.as_str() == msg.sender)
                .map(|(num, _)| num.clone())
                .unwrap_or_else(|| msg.sender.clone())
        };

        // Optimistic local update
        if let Some(conv) = self.conversations.get_mut(&conv_id) {
            if let Some(msg) = conv.messages.get_mut(index) {
                // One reaction per user — replace or push
                if let Some(existing) = msg.reactions.iter_mut().find(|r| r.sender == "you") {
                    existing.emoji = emoji.clone();
                } else {
                    msg.reactions.push(Reaction {
                        emoji: emoji.clone(),
                        sender: "you".to_string(),
                    });
                }
            }
        }

        // Persist to DB
        db_warn(
            self.db.upsert_reaction(&conv_id, target_timestamp, &target_author, "you", &emoji),
            "upsert_reaction",
        );

        Some(SendRequest::Reaction {
            conv_id,
            emoji,
            is_group,
            target_author,
            target_timestamp,
            remove: false,
        })
    }

    /// Handle a key press while the contacts overlay is open.
    pub fn handle_contacts_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.contacts_filtered.is_empty()
                    && self.contacts_index < self.contacts_filtered.len() - 1
                {
                    self.contacts_index += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.contacts_index = self.contacts_index.saturating_sub(1);
            }
            KeyCode::Enter => {
                if let Some((number, _)) = self.contacts_filtered.get(self.contacts_index) {
                    let number = number.clone();
                    self.show_contacts = false;
                    self.contacts_filter.clear();
                    self.join_conversation(&number);
                }
            }
            KeyCode::Esc => {
                self.show_contacts = false;
                self.contacts_filter.clear();
            }
            KeyCode::Backspace => {
                self.contacts_filter.pop();
                self.refresh_contacts_filter();
            }
            KeyCode::Char(c) => {
                // j/k are handled above for navigation, so only printable chars
                // that aren't j/k fall through to here — but since j/k are matched
                // first, we need a different approach: use the filter for all chars
                // Actually j/k are already matched above, so this won't fire for them
                self.contacts_filter.push(c);
                self.refresh_contacts_filter();
            }
            _ => {}
        }
    }

    /// Handle a key press while the autocomplete popup is visible.
    /// Returns `Some(SendRequest)` when the user submits a command
    /// that requires sending a message. Returns `None` otherwise.
    pub fn handle_autocomplete_key(&mut self, code: KeyCode) -> Option<SendRequest> {
        match code {
            KeyCode::Up => {
                let len = self.autocomplete_candidates.len();
                if len > 0 {
                    self.autocomplete_index = if self.autocomplete_index == 0 {
                        len - 1
                    } else {
                        self.autocomplete_index - 1
                    };
                }
            }
            KeyCode::Down => {
                let len = self.autocomplete_candidates.len();
                if len > 0 {
                    self.autocomplete_index = (self.autocomplete_index + 1) % len;
                }
            }
            KeyCode::Tab => {
                self.apply_autocomplete();
            }
            KeyCode::Esc => {
                self.autocomplete_visible = false;
                self.autocomplete_candidates.clear();
                self.autocomplete_index = 0;
            }
            KeyCode::Enter => {
                self.apply_autocomplete();
                return self.handle_input();
            }
            _ => {
                self.apply_input_edit(code);
                self.update_autocomplete();
            }
        }
        None
    }

    pub fn new(account: String, db: Database) -> Self {
        Self {
            conversations: HashMap::new(),
            conversation_order: Vec::new(),
            active_conversation: None,
            input_buffer: String::new(),
            input_cursor: 0,
            input_history: Vec::new(),
            history_index: None,
            history_draft: String::new(),
            sidebar_visible: true,
            scroll_offset: 0,
            status_message: "connecting...".to_string(),
            should_quit: false,
            account,
            sidebar_width: 22,
            typing_indicators: HashMap::new(),
            last_read_index: HashMap::new(),
            connected: false,
            mode: InputMode::Insert,
            db,
            connection_error: None,
            contact_names: HashMap::new(),
            pending_bell: false,
            notify_direct: true,
            notify_group: true,
            muted_conversations: HashSet::new(),
            autocomplete_visible: false,
            autocomplete_candidates: Vec::new(),
            autocomplete_index: 0,
            show_settings: false,
            settings_index: 0,
            show_help: false,
            show_contacts: false,
            contacts_index: 0,
            contacts_filter: String::new(),
            contacts_filtered: Vec::new(),
            inline_images: true,
            link_regions: Vec::new(),
            link_url_map: HashMap::new(),
            image_protocol: image_render::detect_protocol(),
            visible_images: Vec::new(),
            native_images: false,
            native_image_cache: HashMap::new(),
            prev_active_conversation: None,
            incognito: false,
            show_receipts: true,
            color_receipts: true,
            nerd_fonts: false,
            pending_sends: HashMap::new(),
            pending_receipts: Vec::new(),
            focused_message_time: None,
            show_reaction_picker: false,
            reaction_picker_index: 0,
            reaction_verbose: false,
        }
    }

    /// Load conversations and messages from the database on startup
    pub fn load_from_db(&mut self) -> anyhow::Result<()> {
        let conv_data = self.db.load_conversations(500)?;
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

            // Re-render image previews from stored paths
            for msg in &mut conv.messages {
                if msg.body.starts_with("[image:") {
                    let path_str = if let Some(uri_pos) = msg.body.find("file:///") {
                        // Trim trailing ')' from new format: [image: label](file:///path)
                        let uri_slice = msg.body[uri_pos..].trim_end_matches(')');
                        Some(file_uri_to_path(uri_slice))
                    } else if let Some(arrow_pos) = msg.body.find(" -> ") {
                        Some(msg.body[arrow_pos + 4..].trim_end_matches(']').to_string())
                    } else {
                        None
                    };
                    if let Some(p) = path_str {
                        let path = Path::new(&p);
                        if path.exists() {
                            msg.image_path = Some(p.clone());
                            if self.inline_images {
                                msg.image_lines = image_render::render_image(path, 40);
                            }
                        }
                    }
                }
            }

            self.conversations.insert(id.clone(), conv);
            // Derive last_read_index from unread count
            if msg_count > 0 {
                let read_index = msg_count.saturating_sub(unread);
                self.last_read_index.insert(id, read_index);
            }
        }

        self.conversation_order = order;
        self.muted_conversations = self.db.load_muted()?;
        Ok(())
    }

    /// Resize sidebar by delta, clamped between 14..=40
    pub fn resize_sidebar(&mut self, delta: i16) {
        let new_width = (self.sidebar_width as i16 + delta).clamp(14, 40) as u16;
        self.sidebar_width = new_width;
    }

    /// Mark current conversation as fully read
    pub fn mark_read(&mut self) {
        if let Some(ref conv_id) = self.active_conversation {
            if let Some(conv) = self.conversations.get(conv_id) {
                self.last_read_index
                    .insert(conv_id.clone(), conv.messages.len());
            }
            // Persist read marker
            let conv_id = conv_id.clone();
            if let Ok(Some(rowid)) = self.db.last_message_rowid(&conv_id) {
                db_warn(self.db.save_read_marker(&conv_id, rowid), "save_read_marker");
            }
        }
    }

    /// Remove typing indicators older than 5 seconds
    pub fn cleanup_typing(&mut self) {
        let now = Instant::now();
        self.typing_indicators
            .retain(|_, ts| now.duration_since(*ts).as_secs() < 5);
    }

    /// Handle global keys that work in both Normal and Insert mode.
    /// Returns true if the key was consumed.
    pub fn handle_global_key(&mut self, modifiers: KeyModifiers, code: KeyCode) -> bool {
        match (modifiers, code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                self.should_quit = true;
                true
            }
            (KeyModifiers::NONE, KeyCode::Tab) => {
                self.next_conversation();
                true
            }
            (KeyModifiers::SHIFT, KeyCode::BackTab) => {
                self.prev_conversation();
                true
            }
            (KeyModifiers::CONTROL, KeyCode::Left) => {
                self.resize_sidebar(-2);
                true
            }
            (KeyModifiers::CONTROL, KeyCode::Right) => {
                self.resize_sidebar(2);
                true
            }
            (_, KeyCode::PageUp) => {
                self.scroll_offset = self.scroll_offset.saturating_add(5);
                true
            }
            (_, KeyCode::PageDown) => {
                self.scroll_offset = self.scroll_offset.saturating_sub(5);
                true
            }
            _ => false,
        }
    }

    /// Handle overlay keys (help, contacts, settings, autocomplete).
    /// Returns `Some((recipient, body, is_group, local_ts_ms))` if an autocomplete
    /// command triggers a message send. Returns `None` otherwise.
    /// Returns `Ok(true)` if the key was consumed by an overlay.
    pub fn handle_overlay_key(&mut self, code: KeyCode) -> (bool, Option<SendRequest>) {
        if self.show_reaction_picker {
            let send = self.handle_reaction_picker_key(code);
            return (true, send);
        }
        if self.show_help {
            self.show_help = false;
            return (true, None);
        }
        if self.show_contacts {
            self.handle_contacts_key(code);
            return (true, None);
        }
        if self.show_settings {
            self.handle_settings_key(code);
            return (true, None);
        }
        if self.autocomplete_visible {
            let send = self.handle_autocomplete_key(code);
            return (true, send);
        }
        (false, None)
    }

    /// Handle Normal mode key. Returns true if consumed.
    pub fn handle_normal_key(&mut self, modifiers: KeyModifiers, code: KeyCode) {
        match (modifiers, code) {
            // Scrolling
            (_, KeyCode::Char('j')) => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            (_, KeyCode::Char('k')) => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
            }
            (KeyModifiers::CONTROL, KeyCode::Char('d')) => {
                self.scroll_offset = self.scroll_offset.saturating_sub(10);
            }
            (KeyModifiers::CONTROL, KeyCode::Char('u')) => {
                self.scroll_offset = self.scroll_offset.saturating_add(10);
            }
            (_, KeyCode::Char('g')) => {
                if let Some(ref id) = self.active_conversation {
                    if let Some(conv) = self.conversations.get(id) {
                        self.scroll_offset = conv.messages.len();
                    }
                }
            }
            (_, KeyCode::Char('G')) => {
                self.scroll_offset = 0;
            }

            // Switch to Insert mode
            (_, KeyCode::Char('i')) => {
                self.mode = InputMode::Insert;
            }
            (_, KeyCode::Char('a')) => {
                if self.input_cursor < self.input_buffer.len() {
                    self.input_cursor += 1;
                }
                self.mode = InputMode::Insert;
            }
            (_, KeyCode::Char('I')) => {
                self.input_cursor = 0;
                self.mode = InputMode::Insert;
            }
            (_, KeyCode::Char('A')) => {
                self.input_cursor = self.input_buffer.len();
                self.mode = InputMode::Insert;
            }
            (_, KeyCode::Char('o')) => {
                self.input_buffer.clear();
                self.input_cursor = 0;
                self.mode = InputMode::Insert;
            }

            // Cursor movement
            (_, KeyCode::Char('h')) => {
                self.input_cursor = self.input_cursor.saturating_sub(1);
            }
            (_, KeyCode::Char('l')) => {
                if self.input_cursor < self.input_buffer.len() {
                    self.input_cursor += 1;
                }
            }
            (_, KeyCode::Char('0')) => {
                self.input_cursor = 0;
            }
            (_, KeyCode::Char('$')) => {
                self.input_cursor = self.input_buffer.len();
            }
            (_, KeyCode::Char('w')) => {
                let buf = &self.input_buffer;
                let mut pos = self.input_cursor;
                while pos < buf.len() {
                    let c = buf[pos..].chars().next().unwrap();
                    if c.is_whitespace() { break; }
                    pos += c.len_utf8();
                }
                while pos < buf.len() {
                    let c = buf[pos..].chars().next().unwrap();
                    if !c.is_whitespace() { break; }
                    pos += c.len_utf8();
                }
                self.input_cursor = pos;
            }
            (_, KeyCode::Char('b')) => {
                let buf = &self.input_buffer;
                let mut pos = self.input_cursor;
                while pos > 0 {
                    let prev = buf[..pos].chars().next_back().unwrap();
                    if !prev.is_whitespace() { break; }
                    pos -= prev.len_utf8();
                }
                while pos > 0 {
                    let prev = buf[..pos].chars().next_back().unwrap();
                    if prev.is_whitespace() { break; }
                    pos -= prev.len_utf8();
                }
                self.input_cursor = pos;
            }

            // Buffer editing
            (_, KeyCode::Char('x')) => {
                if self.input_cursor < self.input_buffer.len() {
                    self.input_buffer.remove(self.input_cursor);
                    if self.input_cursor > 0
                        && self.input_cursor >= self.input_buffer.len()
                    {
                        self.input_cursor = self.input_buffer.len().saturating_sub(1);
                    }
                }
            }
            (_, KeyCode::Char('D')) => {
                self.input_buffer.truncate(self.input_cursor);
            }

            // Copy message to clipboard
            (_, KeyCode::Char('y')) => {
                self.copy_selected_message(false);
            }
            (_, KeyCode::Char('Y')) => {
                self.copy_selected_message(true);
            }

            // React to focused message
            (_, KeyCode::Char('r')) => {
                if self.selected_message().is_some_and(|m| !m.is_system) {
                    self.show_reaction_picker = true;
                    self.reaction_picker_index = 0;
                }
            }

            // Quick actions
            (_, KeyCode::Char('/')) => {
                self.input_buffer = "/".to_string();
                self.input_cursor = 1;
                self.mode = InputMode::Insert;
                self.update_autocomplete();
            }
            (_, KeyCode::Esc) => {
                if !self.input_buffer.is_empty() {
                    self.input_buffer.clear();
                    self.input_cursor = 0;
                }
            }

            _ => {}
        }
    }

    /// Handle Insert mode key.
    /// Returns `Some(SendRequest)` if Enter triggers a message send (via handle_input).
    pub fn handle_insert_key(&mut self, modifiers: KeyModifiers, code: KeyCode) -> Option<SendRequest> {
        match (modifiers, code) {
            (_, KeyCode::Esc) => {
                self.mode = InputMode::Normal;
                self.autocomplete_visible = false;
                None
            }
            (_, KeyCode::Enter) => self.handle_input(),
            _ => {
                let needs_ac_update = matches!(
                    code,
                    KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_)
                );
                self.apply_input_edit(code);
                if needs_ac_update {
                    self.update_autocomplete();
                }
                None
            }
        }
    }

    /// Handle an event from signal-cli
    pub fn handle_signal_event(&mut self, event: SignalEvent) {
        match event {
            SignalEvent::MessageReceived(msg) => self.handle_message(msg),
            SignalEvent::ReceiptReceived { sender, receipt_type, timestamps } => {
                self.handle_receipt(&sender, &receipt_type, &timestamps);
            }
            SignalEvent::SendTimestamp { rpc_id, server_ts } => {
                self.handle_send_timestamp(&rpc_id, server_ts);
            }
            SignalEvent::SendFailed { rpc_id } => {
                self.status_message = "send failed".to_string();
                self.handle_send_failed(&rpc_id);
            }
            SignalEvent::TypingIndicator { sender, sender_name, is_typing } => {
                // Store name in contact lookup if we learned it from this event
                if let Some(ref name) = sender_name {
                    self.contact_names.entry(sender.clone()).or_insert_with(|| name.clone());
                }
                // Store typing state per-conversation (use sender as key for 1:1)
                if is_typing {
                    self.typing_indicators.insert(sender.clone(), Instant::now());
                } else {
                    self.typing_indicators.remove(&sender);
                }
            }
            SignalEvent::ReactionReceived {
                conv_id, emoji, sender, sender_name, target_author, target_timestamp, is_remove,
            } => {
                if let Some(ref name) = sender_name {
                    self.contact_names.entry(sender.clone()).or_insert_with(|| name.clone());
                }
                self.handle_reaction(&conv_id, &emoji, &sender, &target_author, target_timestamp, is_remove);
            }
            SignalEvent::ContactList(contacts) => self.handle_contact_list(contacts),
            SignalEvent::GroupList(groups) => self.handle_group_list(groups),
            SignalEvent::Error(err) => {
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

        // Store source_name in contact lookup for future resolution (typing indicators, etc.)
        if !msg.is_outgoing {
            if let Some(ref name) = msg.source_name {
                self.contact_names.entry(msg.source.clone()).or_insert_with(|| name.clone());
            }
        }

        // Resolve conversation name: prefer message metadata, then contact lookup, then raw ID
        // For groups, source_name is the sender (not the group), so skip it
        let is_group = msg.group_id.is_some();
        let conv_name = msg
            .group_name
            .as_deref()
            .or(if is_group { None } else { msg.source_name.as_deref() })
            .unwrap_or_else(|| {
                self.contact_names.get(&conv_id).map(|s| s.as_str()).unwrap_or(&conv_id)
            })
            .to_string();

        let sender_display = if msg.is_outgoing {
            "you".to_string()
        } else {
            msg.source_name
                .clone()
                .or_else(|| self.contact_names.get(&msg.source).cloned())
                .unwrap_or_else(|| short_name(&msg.source))
        };

        // Ensure conversation exists (drop the mutable ref immediately)
        self.get_or_create_conversation(&conv_id, &conv_name, is_group);

        let ts_rfc3339 = msg.timestamp.to_rfc3339();
        let msg_ts_ms = msg.timestamp.timestamp_millis();
        // Outgoing synced messages already have a server timestamp; incoming messages have no status
        let msg_status = if msg.is_outgoing { Some(MessageStatus::Sent) } else { None };

        // Helper: push a DisplayMessage and persist to DB
        let mut push_msg = |body: String,
                            image_lines: Option<Vec<Line<'static>>>,
                            image_path: Option<String>| {
            if let Some(conv) = self.conversations.get_mut(&conv_id) {
                conv.messages.push(DisplayMessage {
                    sender: sender_display.clone(),
                    timestamp: msg.timestamp,
                    body: body.clone(),
                    is_system: false,
                    image_lines,
                    image_path,
                    status: msg_status,
                    timestamp_ms: msg_ts_ms,
                    reactions: Vec::new(),
                });
            }
            db_warn(
                self.db.insert_message(
                    &conv_id, &sender_display, &ts_rfc3339, &body, false, msg_status, msg_ts_ms,
                ),
                "insert_message",
            );
        };

        // Add text body
        if let Some(ref body) = msg.body {
            push_msg(body.clone(), None, None);
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
                let rendered = if self.inline_images {
                    att.local_path
                        .as_deref()
                        .and_then(|p| image_render::render_image(Path::new(p), 40))
                } else {
                    None
                };
                push_msg(
                    format!("[image: {label}]{path_info}"),
                    rendered,
                    att.local_path.clone(),
                );
            } else {
                push_msg(format!("[attachment: {label}]{path_info}"), None, None);
            }
        }

        let is_active = self
            .active_conversation
            .as_ref()
            .map(|a| a == &conv_id)
            .unwrap_or(false);

        if !is_active && !msg.is_outgoing {
            if let Some(c) = self.conversations.get_mut(&conv_id) {
                c.unread += 1;
            }
            let type_enabled = if is_group { self.notify_group } else { self.notify_direct };
            if type_enabled && !self.muted_conversations.contains(&conv_id) {
                self.pending_bell = true;
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
        // Pre-resolve names to avoid borrow conflict with self.conversations.
        let account = &self.account;
        let target_display = self.contact_names.get(target_author).cloned();
        // Resolve sender phone number to display name for rendering
        let is_self = sender == self.account;
        let sender_display = if is_self {
            "you".to_string()
        } else {
            self.contact_names
                .get(sender)
                .cloned()
                .unwrap_or_else(|| sender.to_string())
        };
        if let Some(conv) = self.conversations.get_mut(conv_id) {
            let found = conv.messages.iter_mut().rev().find(|m| {
                if m.timestamp_ms != target_timestamp {
                    return false;
                }
                if m.sender == "you" {
                    target_author == account.as_str()
                } else {
                    m.sender == target_author
                        || target_display.as_deref() == Some(m.sender.as_str())
                }
            });
            if let Some(msg) = found {
                if is_remove {
                    // Match by display name or "you" (for own reactions from other devices)
                    msg.reactions.retain(|r| r.sender != sender_display);
                } else {
                    // One reaction per user — replace or push
                    if let Some(existing) = msg.reactions.iter_mut().find(|r| r.sender == sender_display) {
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
            db_warn(
                self.db.remove_reaction(conv_id, target_timestamp, target_author, sender),
                "remove_reaction",
            );
        } else {
            db_warn(
                self.db.upsert_reaction(conv_id, target_timestamp, target_author, sender, emoji),
                "upsert_reaction",
            );
        }
    }

    fn handle_contact_list(&mut self, contacts: Vec<Contact>) {
        for contact in contacts {
            // Store name in lookup for future message resolution
            if let Some(ref name) = contact.name {
                if !name.is_empty() {
                    self.contact_names.insert(contact.number.clone(), name.clone());
                }
            }
            // Update name on existing conversations only — don't create new ones
            if let Some(conv) = self.conversations.get_mut(&contact.number) {
                if let Some(ref contact_name) = contact.name {
                    if !contact_name.is_empty() && conv.name != *contact_name {
                        conv.name = contact_name.clone();
                        db_warn(self.db.upsert_conversation(&contact.number, contact_name, false), "upsert_conversation");
                    }
                }
            }
        }
    }

    fn handle_group_list(&mut self, groups: Vec<Group>) {
        for group in groups {
            // Store name in lookup for future message resolution
            if !group.name.is_empty() {
                self.contact_names.insert(group.id.clone(), group.name.clone());
            }
            // Groups are always "active" (you're a member), so create conversations
            let conv = self.get_or_create_conversation(&group.id, &group.name, true);
            if !group.name.is_empty() && conv.name != group.name {
                conv.name = group.name.clone();
                db_warn(self.db.upsert_conversation(&group.id, &group.name, true), "upsert_conversation");
            }
        }
    }

    fn handle_send_timestamp(&mut self, rpc_id: &str, server_ts: i64) {
        if let Some((conv_id, local_ts)) = self.pending_sends.remove(rpc_id) {
            crate::debug_log::logf(format_args!(
                "send confirmed: conv={conv_id} local_ts={local_ts} server_ts={server_ts}"
            ));
            if let Some(conv) = self.conversations.get_mut(&conv_id) {
                // Find the outgoing message with matching local timestamp
                for msg in conv.messages.iter_mut().rev() {
                    if msg.sender == "you" && msg.timestamp_ms == local_ts {
                        let effective_ts = if server_ts != 0 { server_ts } else { local_ts };
                        // Update the DB row's timestamp_ms from local → server
                        db_warn(self.db.update_message_timestamp_ms(
                            &conv_id,
                            local_ts,
                            effective_ts,
                            MessageStatus::Sent.to_i32(),
                        ), "update_message_timestamp_ms");
                        msg.timestamp_ms = effective_ts;
                        msg.status = Some(MessageStatus::Sent);
                        break;
                    }
                }
            }

            // Replay any buffered receipts that may have arrived before this SendTimestamp
            if !self.pending_receipts.is_empty() {
                let receipts = std::mem::take(&mut self.pending_receipts);
                for (sender, receipt_type, timestamps) in receipts {
                    self.handle_receipt(&sender, &receipt_type, &timestamps);
                }
            }
        }
    }

    fn handle_send_failed(&mut self, rpc_id: &str) {
        if let Some((conv_id, local_ts)) = self.pending_sends.remove(rpc_id) {
            if let Some(conv) = self.conversations.get_mut(&conv_id) {
                for msg in conv.messages.iter_mut().rev() {
                    if msg.sender == "you" && msg.timestamp_ms == local_ts {
                        msg.status = Some(MessageStatus::Failed);
                        db_warn(self.db.update_message_status(
                            &conv_id,
                            local_ts,
                            MessageStatus::Failed.to_i32(),
                        ), "update_message_status");
                        break;
                    }
                }
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
        for msg in conv.messages.iter_mut().rev() {
            if msg.sender == "you" && msg.timestamp_ms == ts {
                if let Some(current) = msg.status {
                    if new_status > current {
                        msg.status = Some(new_status);
                        db_warn(
                            db.update_message_status(conv_id, ts, new_status.to_i32()),
                            "update_message_status",
                        );
                    }
                }
                return true;
            }
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
        if let Some(conv) = self.conversations.get_mut(&conv_id) {
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
                for (cid, conv) in &mut self.conversations {
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
                "receipt: buffering {receipt_type} from {sender} (no matching ts)"
            ));
            self.pending_receipts.push((
                sender.to_string(),
                receipt_type.to_string(),
                timestamps.to_vec(),
            ));
        } else if matched_any {
            crate::debug_log::logf(format_args!(
                "receipt: {receipt_type} from {sender} -> {new_status:?}"
            ));
        }
    }

    fn get_or_create_conversation(
        &mut self,
        id: &str,
        name: &str,
        is_group: bool,
    ) -> &mut Conversation {
        db_warn(self.db.upsert_conversation(id, name, is_group), "upsert_conversation");
        if !self.conversations.contains_key(id) {
            self.conversations.insert(
                id.to_string(),
                Conversation {
                    name: name.to_string(),
                    id: id.to_string(),
                    messages: Vec::new(),
                    unread: 0,
                    is_group,
                },
            );
            self.conversation_order.push(id.to_string());
        }
        self.conversations.get_mut(id).unwrap()
    }

    /// Handle a line of user input; returns Some((conv_id, body, is_group, local_ts_ms)) if we need to send a message
    pub fn handle_input(&mut self) -> Option<SendRequest> {
        let input = self.input_buffer.clone();
        let trimmed = input.trim();
        if !trimmed.is_empty() {
            self.input_history.push(trimmed.to_string());
        }
        self.history_index = None;
        self.input_buffer.clear();
        self.input_cursor = 0;

        let action = input::parse_input(&input);
        match action {
            InputAction::SendText(text) => {
                if text.is_empty() {
                    return None;
                }
                if let Some(ref conv_id) = self.active_conversation {
                    let is_group = self
                        .conversations
                        .get(conv_id)
                        .map(|c| c.is_group)
                        .unwrap_or(false);
                    let conv_id = conv_id.clone();

                    // Add our own message to the display
                    let now = Utc::now();
                    let local_ts_ms = now.timestamp_millis();
                    if let Some(conv) = self.conversations.get_mut(&conv_id) {
                        conv.messages.push(DisplayMessage {
                            sender: "you".to_string(),
                            timestamp: now,
                            body: text.clone(),
                            is_system: false,
                            image_lines: None,
                            image_path: None,
                            status: Some(MessageStatus::Sending),
                            timestamp_ms: local_ts_ms,
                            reactions: Vec::new(),
                        });
                    }
                    db_warn(self.db.insert_message(
                        &conv_id,
                        "you",
                        &now.to_rfc3339(),
                        &text,
                        false,
                        Some(MessageStatus::Sending),
                        local_ts_ms,
                    ), "insert_message");
                    self.scroll_offset = 0;
                    return Some(SendRequest::Message {
                        recipient: conv_id,
                        body: text,
                        is_group,
                        local_ts_ms,
                    });
                } else {
                    self.status_message =
                        "No active conversation. Use /join <name> first.".to_string();
                }
            }
            InputAction::Join(target) => {
                self.join_conversation(&target);
            }
            InputAction::Part => {
                self.active_conversation = None;
                self.scroll_offset = 0;
                self.update_status();
            }
            InputAction::Quit => {
                self.should_quit = true;
            }
            InputAction::ToggleSidebar => {
                self.sidebar_visible = !self.sidebar_visible;
            }
            InputAction::ToggleBell(ref target) => {
                match target.as_deref() {
                    None => {
                        // Toggle both together
                        let new_state = !(self.notify_direct && self.notify_group);
                        self.notify_direct = new_state;
                        self.notify_group = new_state;
                        let state = if new_state { "on" } else { "off" };
                        self.status_message = format!("notifications {state}");
                    }
                    Some("direct" | "dm" | "1:1") => {
                        self.notify_direct = !self.notify_direct;
                        let state = if self.notify_direct { "on" } else { "off" };
                        self.status_message = format!("direct notifications {state}");
                    }
                    Some("group" | "groups") => {
                        self.notify_group = !self.notify_group;
                        let state = if self.notify_group { "on" } else { "off" };
                        self.status_message = format!("group notifications {state}");
                    }
                    Some(other) => {
                        self.status_message = format!("unknown bell type: {other} (use direct or group)");
                    }
                }
            }
            InputAction::ToggleMute => {
                if let Some(ref conv_id) = self.active_conversation {
                    let conv_id = conv_id.clone();
                    if self.muted_conversations.remove(&conv_id) {
                        let name = self.conversations.get(&conv_id)
                            .map(|c| c.name.as_str()).unwrap_or(&conv_id);
                        self.status_message = format!("unmuted {name}");
                        db_warn(self.db.set_muted(&conv_id, false), "set_muted");
                    } else {
                        let name = self.conversations.get(&conv_id)
                            .map(|c| c.name.as_str()).unwrap_or(&conv_id);
                        self.status_message = format!("muted {name}");
                        self.muted_conversations.insert(conv_id.clone());
                        db_warn(self.db.set_muted(&conv_id, true), "set_muted");
                    }
                } else {
                    self.status_message = "no active conversation to mute".to_string();
                }
            }
            InputAction::Settings => {
                self.show_settings = true;
                self.settings_index = 0;
            }
            InputAction::Contacts => {
                self.show_contacts = true;
                self.contacts_index = 0;
                self.contacts_filter.clear();
                self.refresh_contacts_filter();
            }
            InputAction::Help => {
                self.show_help = true;
            }
            InputAction::Unknown(msg) => {
                self.status_message = msg;
            }
        }
        None
    }

    /// Update autocomplete candidates based on current input_buffer.
    /// Called after every input change in Insert mode.
    pub fn update_autocomplete(&mut self) {
        let buf = &self.input_buffer;

        // Only show autocomplete if buffer starts with '/' and has no space yet
        if !buf.starts_with('/') || buf.contains(' ') {
            self.autocomplete_visible = false;
            self.autocomplete_candidates.clear();
            self.autocomplete_index = 0;
            return;
        }

        let prefix = buf.to_lowercase();
        let mut candidates = Vec::new();
        for (i, cmd) in COMMANDS.iter().enumerate() {
            if cmd.name.starts_with(&prefix)
                || (!cmd.alias.is_empty() && cmd.alias.starts_with(&prefix))
            {
                candidates.push(i);
            }
        }

        if candidates.is_empty() {
            self.autocomplete_visible = false;
            self.autocomplete_candidates.clear();
            self.autocomplete_index = 0;
        } else {
            self.autocomplete_visible = true;
            self.autocomplete_candidates = candidates;
            // Clamp index
            if self.autocomplete_index >= self.autocomplete_candidates.len() {
                self.autocomplete_index = 0;
            }
        }
    }

    /// Handle basic cursor/editing keys (Backspace, Delete, Left, Right, Home, End, Char).
    /// Returns true if the key was handled.
    /// Navigate up through input history (older entries).
    pub fn history_up(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        match self.history_index {
            None => {
                self.history_draft = self.input_buffer.clone();
                self.history_index = Some(self.input_history.len() - 1);
            }
            Some(idx) if idx > 0 => {
                self.history_index = Some(idx - 1);
            }
            _ => return,
        }
        self.input_buffer = self.input_history[self.history_index.unwrap()].clone();
        self.input_cursor = self.input_buffer.len();
    }

    /// Navigate down through input history (newer entries).
    pub fn history_down(&mut self) {
        let idx = match self.history_index {
            Some(idx) => idx,
            None => return,
        };
        if idx < self.input_history.len() - 1 {
            self.history_index = Some(idx + 1);
            self.input_buffer = self.input_history[idx + 1].clone();
        } else {
            self.input_buffer = self.history_draft.clone();
            self.history_index = None;
        }
        self.input_cursor = self.input_buffer.len();
    }

    pub fn apply_input_edit(&mut self, key_code: KeyCode) -> bool {
        match key_code {
            KeyCode::Backspace => {
                if self.input_cursor > 0 {
                    self.input_cursor -= 1;
                    self.input_buffer.remove(self.input_cursor);
                }
                true
            }
            KeyCode::Delete => {
                if self.input_cursor < self.input_buffer.len() {
                    self.input_buffer.remove(self.input_cursor);
                }
                true
            }
            KeyCode::Left => {
                self.input_cursor = self.input_cursor.saturating_sub(1);
                true
            }
            KeyCode::Right => {
                if self.input_cursor < self.input_buffer.len() {
                    self.input_cursor += 1;
                }
                true
            }
            KeyCode::Home => {
                self.input_cursor = 0;
                true
            }
            KeyCode::End => {
                self.input_cursor = self.input_buffer.len();
                true
            }
            KeyCode::Up => {
                self.history_up();
                true
            }
            KeyCode::Down => {
                self.history_down();
                true
            }
            KeyCode::Char(c) => {
                self.input_buffer.insert(self.input_cursor, c);
                self.input_cursor += 1;
                true
            }
            _ => false,
        }
    }

    /// Accept the currently selected autocomplete candidate.
    pub fn apply_autocomplete(&mut self) {
        if let Some(&cmd_idx) = self.autocomplete_candidates.get(self.autocomplete_index) {
            let cmd = &COMMANDS[cmd_idx];
            if cmd.args.is_empty() {
                self.input_buffer = cmd.name.to_string();
            } else {
                self.input_buffer = format!("{} ", cmd.name);
            }
            self.input_cursor = self.input_buffer.len();
            self.autocomplete_visible = false;
            self.autocomplete_candidates.clear();
            self.autocomplete_index = 0;
        }
    }

    fn join_conversation(&mut self, target: &str) {
        self.mark_read();

        // Try exact match first
        if self.conversations.contains_key(target) {
            self.active_conversation = Some(target.to_string());
            if let Some(conv) = self.conversations.get_mut(target) {
                conv.unread = 0;
            }
            self.scroll_offset = 0;
            self.update_status();
            return;
        }

        // Try matching by name (case-insensitive)
        let target_lower = target.to_lowercase();
        let found_id = self
            .conversations
            .iter()
            .find(|(_, conv)| conv.name.to_lowercase().contains(&target_lower))
            .map(|(id, _)| id.clone());

        if let Some(id) = found_id {
            self.active_conversation = Some(id.clone());
            self.scroll_offset = 0;
            if let Some(conv) = self.conversations.get_mut(&id) {
                conv.unread = 0;
            }
            self.update_status();
            return;
        }

        // Create a new 1:1 conversation if target looks like a phone number
        if target.starts_with('+') {
            self.get_or_create_conversation(target, target, false);
            self.active_conversation = Some(target.to_string());
            self.scroll_offset = 0;
            self.update_status();
        } else {
            self.status_message = format!("Conversation not found: {target}");
        }
    }

    pub fn next_conversation(&mut self) {
        if self.conversation_order.is_empty() {
            return;
        }
        self.mark_read();
        let idx = self
            .active_conversation
            .as_ref()
            .and_then(|id| self.conversation_order.iter().position(|x| x == id))
            .map(|i| (i + 1) % self.conversation_order.len())
            .unwrap_or(0);
        let new_id = self.conversation_order[idx].clone();
        self.active_conversation = Some(new_id.clone());
        if let Some(conv) = self.conversations.get_mut(&new_id) {
            conv.unread = 0;
        }
        self.scroll_offset = 0;
        self.update_status();
    }

    pub fn prev_conversation(&mut self) {
        if self.conversation_order.is_empty() {
            return;
        }
        self.mark_read();
        let len = self.conversation_order.len();
        let idx = self
            .active_conversation
            .as_ref()
            .and_then(|id| self.conversation_order.iter().position(|x| x == id))
            .map(|i| if i == 0 { len - 1 } else { i - 1 })
            .unwrap_or(0);
        let new_id = self.conversation_order[idx].clone();
        self.active_conversation = Some(new_id.clone());
        if let Some(conv) = self.conversations.get_mut(&new_id) {
            conv.unread = 0;
        }
        self.scroll_offset = 0;
        self.update_status();
    }


    fn update_status(&mut self) {
        if let Some(ref id) = self.active_conversation {
            if let Some(conv) = self.conversations.get(id) {
                let prefix = if conv.is_group { "#" } else { "" };
                self.status_message = format!("connected | {}{}", prefix, conv.name);
            }
        } else {
            self.status_message = "connected | no conversation selected".to_string();
        }
    }

    pub fn set_connected(&mut self) {
        self.connected = true;
        self.status_message = "connected | no conversation selected".to_string();
    }

    /// Total unread count across all conversations
    pub fn total_unread(&self) -> usize {
        self.conversations.values().map(|c| c.unread).sum()
    }

    /// Get the message at the current scroll position.
    /// Returns the message at the bottom of the visible viewport.
    /// scroll_offset=0 means the newest message; higher values go older.
    fn selected_message(&self) -> Option<&DisplayMessage> {
        let conv_id = self.active_conversation.as_ref()?;
        let conv = self.conversations.get(conv_id)?;
        let total = conv.messages.len();
        if total == 0 {
            return None;
        }
        let index = total.saturating_sub(1).saturating_sub(self.scroll_offset);
        conv.messages.get(index)
    }

    /// Copy the selected message text to the system clipboard.
    /// If `full_line` is true, copies "[HH:MM] <sender> body"; otherwise just the body.
    pub fn copy_selected_message(&mut self, full_line: bool) {
        let text = match self.selected_message() {
            Some(msg) if msg.is_system => Some(msg.body.clone()),
            Some(msg) => {
                if full_line {
                    Some(format!("[{}] <{}> {}", msg.format_time(), msg.sender, msg.body))
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
}

/// Shorten a phone number for display: +15551234567 -> +1***4567
fn short_name(number: &str) -> String {
    if number.len() > 6 {
        let last4 = &number[number.len() - 4..];
        let prefix = &number[..2];
        format!("{prefix}***{last4}")
    } else {
        number.to_string()
    }
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

/// Extract a local file path from a file:/// URI string (which may have trailing text).
fn file_uri_to_path(uri: &str) -> String {
    let uri = uri.trim();
    let stripped = uri.strip_prefix("file:///").unwrap_or(
        uri.strip_prefix("file://").unwrap_or(uri),
    );
    stripped.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::signal::types::{Contact, Group, SignalEvent, SignalMessage};

    fn test_app() -> App {
        let db = Database::open_in_memory().unwrap();
        let mut app = App::new("+10000000000".to_string(), db);
        app.set_connected();
        app
    }

    // --- Contacts/groups only populate the name lookup, not the sidebar ---

    #[test]
    fn contact_list_does_not_create_conversations() {
        let mut app = test_app();
        assert!(app.conversations.is_empty());

        app.handle_signal_event(SignalEvent::ContactList(vec![
            Contact { number: "+1".to_string(), name: Some("Alice".to_string()) },
            Contact { number: "+2".to_string(), name: Some("Bob".to_string()) },
        ]));

        // No conversations created — only name lookup populated
        assert!(app.conversations.is_empty());
        assert!(app.conversation_order.is_empty());
        assert_eq!(app.contact_names["+1"], "Alice");
        assert_eq!(app.contact_names["+2"], "Bob");
    }

    #[test]
    fn group_list_creates_conversations() {
        let mut app = test_app();

        app.handle_signal_event(SignalEvent::GroupList(vec![
            Group { id: "g1".to_string(), name: "Family".to_string(), members: vec![] },
            Group { id: "g2".to_string(), name: "Work".to_string(), members: vec![] },
        ]));

        // Groups always create conversations (you're a member)
        assert_eq!(app.conversations.len(), 2);
        assert_eq!(app.conversations["g1"].name, "Family");
        assert_eq!(app.conversations["g2"].name, "Work");
        assert!(app.conversations["g1"].is_group);
        assert_eq!(app.contact_names["g1"], "Family");
    }

    // --- Contact names enrich existing conversations ---

    #[test]
    fn contact_name_updates_existing_conversation() {
        let mut app = test_app();

        // A message arrives first with just a phone number
        let msg = SignalMessage {
            source: "+15551234567".to_string(),
            source_name: None,
            timestamp: chrono::Utc::now(),
            body: Some("hey".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert_eq!(app.conversations["+15551234567"].name, "+15551234567");

        // Contact list arrives with a proper name — updates existing conv
        app.handle_signal_event(SignalEvent::ContactList(vec![
            Contact { number: "+15551234567".to_string(), name: Some("Alice".to_string()) },
        ]));

        assert_eq!(app.conversations["+15551234567"].name, "Alice");
    }

    #[test]
    fn contact_without_name_does_not_overwrite_existing_name() {
        let mut app = test_app();

        // Create conversation with a name already
        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: Some("Alice".to_string()),
            timestamp: chrono::Utc::now(),
            body: Some("hi".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert_eq!(app.conversations["+1"].name, "Alice");

        // Contact arrives with no name — should NOT overwrite
        app.handle_signal_event(SignalEvent::ContactList(vec![
            Contact { number: "+1".to_string(), name: None },
        ]));

        assert_eq!(app.conversations["+1"].name, "Alice");
    }

    // --- Name lookup used when creating conversations from messages ---

    #[test]
    fn message_uses_contact_name_lookup() {
        let mut app = test_app();

        // Contacts loaded first (no conversations created)
        app.handle_signal_event(SignalEvent::ContactList(vec![
            Contact { number: "+1".to_string(), name: Some("Alice".to_string()) },
        ]));
        assert!(app.conversations.is_empty());

        // Message arrives with no source_name — should use lookup
        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: None,
            timestamp: chrono::Utc::now(),
            body: Some("hello!".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        assert_eq!(app.conversations.len(), 1);
        assert_eq!(app.conversations["+1"].name, "Alice");
        assert_eq!(app.conversations["+1"].messages[0].sender, "Alice");
    }

    #[test]
    fn message_in_known_group_uses_name_lookup() {
        let mut app = test_app();

        // Groups loaded — conversation created
        app.handle_signal_event(SignalEvent::GroupList(vec![
            Group { id: "g1".to_string(), name: "Family".to_string(), members: vec![] },
        ]));
        assert_eq!(app.conversations.len(), 1);

        // Message arrives in that group (no group_name in metadata)
        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: Some("Alice".to_string()),
            timestamp: chrono::Utc::now(),
            body: Some("hey family".to_string()),
            attachments: vec![],
            group_id: Some("g1".to_string()),
            group_name: None,
            is_outgoing: false,
            destination: None,
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        // Still 1 conversation, name preserved from group list
        assert_eq!(app.conversations.len(), 1);
        assert_eq!(app.conversations["g1"].name, "Family");
        assert_eq!(app.conversations["g1"].messages.len(), 1);
    }

    // --- No duplicate conversations ---

    #[test]
    fn no_duplicate_on_repeated_messages() {
        let mut app = test_app();

        app.handle_signal_event(SignalEvent::ContactList(vec![
            Contact { number: "+1".to_string(), name: Some("Alice".to_string()) },
        ]));

        for _ in 0..3 {
            let msg = SignalMessage {
                source: "+1".to_string(),
                source_name: Some("Alice".to_string()),
                timestamp: chrono::Utc::now(),
                body: Some("msg".to_string()),
                attachments: vec![],
                group_id: None,
                group_name: None,
                is_outgoing: false,
                destination: None,
            };
            app.handle_signal_event(SignalEvent::MessageReceived(msg));
        }

        assert_eq!(app.conversations.len(), 1);
        assert_eq!(app.conversation_order.len(), 1);
        assert_eq!(app.conversations["+1"].messages.len(), 3);
    }

    // --- Autocomplete tests ---

    #[test]
    fn autocomplete_slash_prefix() {
        let mut app = test_app();
        app.input_buffer = "/".to_string();
        app.update_autocomplete();
        assert!(app.autocomplete_visible);
        assert!(!app.autocomplete_candidates.is_empty());
    }

    #[test]
    fn autocomplete_prefix_filtering() {
        let mut app = test_app();
        app.input_buffer = "/jo".to_string();
        app.update_autocomplete();
        assert!(app.autocomplete_visible);
        // Only /join should match
        assert_eq!(app.autocomplete_candidates.len(), 1);
        assert_eq!(COMMANDS[app.autocomplete_candidates[0]].name, "/join");
    }

    #[test]
    fn autocomplete_non_slash_hidden() {
        let mut app = test_app();
        app.input_buffer = "hello".to_string();
        app.update_autocomplete();
        assert!(!app.autocomplete_visible);
        assert!(app.autocomplete_candidates.is_empty());
    }

    #[test]
    fn autocomplete_space_hides() {
        let mut app = test_app();
        app.input_buffer = "/join ".to_string();
        app.update_autocomplete();
        assert!(!app.autocomplete_visible);
    }

    #[test]
    fn autocomplete_no_match() {
        let mut app = test_app();
        app.input_buffer = "/zzz".to_string();
        app.update_autocomplete();
        assert!(!app.autocomplete_visible);
        assert!(app.autocomplete_candidates.is_empty());
    }

    #[test]
    fn apply_autocomplete_trailing_space_for_arg_command() {
        let mut app = test_app();
        app.input_buffer = "/jo".to_string();
        app.update_autocomplete();
        app.apply_autocomplete();
        // /join takes args, so buffer should end with a space
        assert_eq!(app.input_buffer, "/join ");
        assert_eq!(app.input_cursor, 6);
    }

    #[test]
    fn apply_autocomplete_no_space_for_no_arg_command() {
        let mut app = test_app();
        app.input_buffer = "/pa".to_string();
        app.update_autocomplete();
        app.apply_autocomplete();
        // /part takes no args, no trailing space
        assert_eq!(app.input_buffer, "/part");
        assert_eq!(app.input_cursor, 5);
    }

    #[test]
    fn apply_autocomplete_index_clamped() {
        let mut app = test_app();
        app.input_buffer = "/".to_string();
        app.update_autocomplete();
        let len = app.autocomplete_candidates.len();
        app.autocomplete_index = len + 5; // way out of bounds
        app.update_autocomplete(); // should clamp
        assert!(app.autocomplete_index < app.autocomplete_candidates.len());
    }

    // --- apply_input_edit tests ---

    #[test]
    fn input_edit_char_insert() {
        let mut app = test_app();
        assert!(app.apply_input_edit(KeyCode::Char('a')));
        assert!(app.apply_input_edit(KeyCode::Char('b')));
        assert_eq!(app.input_buffer, "ab");
        assert_eq!(app.input_cursor, 2);
    }

    #[test]
    fn input_edit_backspace() {
        let mut app = test_app();
        app.input_buffer = "abc".to_string();
        app.input_cursor = 3;
        assert!(app.apply_input_edit(KeyCode::Backspace));
        assert_eq!(app.input_buffer, "ab");
        assert_eq!(app.input_cursor, 2);
    }

    #[test]
    fn input_edit_delete() {
        let mut app = test_app();
        app.input_buffer = "abc".to_string();
        app.input_cursor = 1;
        assert!(app.apply_input_edit(KeyCode::Delete));
        assert_eq!(app.input_buffer, "ac");
        assert_eq!(app.input_cursor, 1);
    }

    #[test]
    fn input_edit_left_right() {
        let mut app = test_app();
        app.input_buffer = "abc".to_string();
        app.input_cursor = 2;
        assert!(app.apply_input_edit(KeyCode::Left));
        assert_eq!(app.input_cursor, 1);
        assert!(app.apply_input_edit(KeyCode::Right));
        assert_eq!(app.input_cursor, 2);
    }

    #[test]
    fn input_edit_home_end() {
        let mut app = test_app();
        app.input_buffer = "abc".to_string();
        app.input_cursor = 1;
        assert!(app.apply_input_edit(KeyCode::Home));
        assert_eq!(app.input_cursor, 0);
        assert!(app.apply_input_edit(KeyCode::End));
        assert_eq!(app.input_cursor, 3);
    }

    #[test]
    fn input_edit_unhandled_key() {
        let mut app = test_app();
        assert!(!app.apply_input_edit(KeyCode::F(1)));
    }

    // --- Input history tests ---

    #[test]
    fn history_up_empty_is_noop() {
        let mut app = test_app();
        app.input_buffer = "draft".to_string();
        app.history_up();
        assert_eq!(app.input_buffer, "draft");
        assert_eq!(app.history_index, None);
    }

    #[test]
    fn history_down_without_browsing_is_noop() {
        let mut app = test_app();
        app.input_buffer = "draft".to_string();
        app.history_down();
        assert_eq!(app.input_buffer, "draft");
        assert_eq!(app.history_index, None);
    }

    #[test]
    fn history_up_recalls_last_entry() {
        let mut app = test_app();
        app.input_history = vec!["hello".to_string(), "world".to_string()];
        app.input_buffer = "draft".to_string();
        app.input_cursor = 5;

        app.history_up();
        assert_eq!(app.input_buffer, "world");
        assert_eq!(app.history_index, Some(1));
        assert_eq!(app.history_draft, "draft");
        assert_eq!(app.input_cursor, 5); // cursor at end of "world"
    }

    #[test]
    fn history_up_walks_to_oldest() {
        let mut app = test_app();
        app.input_history = vec!["first".to_string(), "second".to_string(), "third".to_string()];
        app.input_buffer = String::new();

        app.history_up(); // -> "third"
        assert_eq!(app.input_buffer, "third");
        assert_eq!(app.history_index, Some(2));

        app.history_up(); // -> "second"
        assert_eq!(app.input_buffer, "second");
        assert_eq!(app.history_index, Some(1));

        app.history_up(); // -> "first"
        assert_eq!(app.input_buffer, "first");
        assert_eq!(app.history_index, Some(0));

        // At oldest — stays put
        app.history_up();
        assert_eq!(app.input_buffer, "first");
        assert_eq!(app.history_index, Some(0));
    }

    #[test]
    fn history_down_walks_forward_and_restores_draft() {
        let mut app = test_app();
        app.input_history = vec!["aaa".to_string(), "bbb".to_string()];
        app.input_buffer = "my draft".to_string();

        // Go to oldest
        app.history_up(); // -> "bbb"
        app.history_up(); // -> "aaa"
        assert_eq!(app.input_buffer, "aaa");
        assert_eq!(app.history_index, Some(0));

        // Walk forward
        app.history_down(); // -> "bbb"
        assert_eq!(app.input_buffer, "bbb");
        assert_eq!(app.history_index, Some(1));

        // Past newest restores draft
        app.history_down();
        assert_eq!(app.input_buffer, "my draft");
        assert_eq!(app.history_index, None);
    }

    #[test]
    fn history_cursor_moves_to_end() {
        let mut app = test_app();
        app.input_history = vec!["short".to_string(), "a longer entry".to_string()];
        app.input_buffer = String::new();
        app.input_cursor = 0;

        app.history_up(); // -> "a longer entry"
        assert_eq!(app.input_cursor, 14);

        app.history_up(); // -> "short"
        assert_eq!(app.input_cursor, 5);

        app.history_down(); // -> "a longer entry"
        assert_eq!(app.input_cursor, 14);

        app.history_down(); // -> draft ""
        assert_eq!(app.input_cursor, 0);
    }

    #[test]
    fn handle_input_saves_to_history() {
        let mut app = test_app();
        // Need an active conversation for SendText to work
        app.get_or_create_conversation("+1", "Alice", false);
        app.active_conversation = Some("+1".to_string());

        app.input_buffer = "hello".to_string();
        app.input_cursor = 5;
        app.handle_input();
        assert_eq!(app.input_history, vec!["hello".to_string()]);
        assert_eq!(app.history_index, None);

        app.input_buffer = "world".to_string();
        app.input_cursor = 5;
        app.handle_input();
        assert_eq!(app.input_history, vec!["hello".to_string(), "world".to_string()]);
    }

    #[test]
    fn handle_input_trims_and_skips_empty() {
        let mut app = test_app();
        app.get_or_create_conversation("+1", "Alice", false);
        app.active_conversation = Some("+1".to_string());

        // Whitespace-only input should not be saved
        app.input_buffer = "   ".to_string();
        app.handle_input();
        assert!(app.input_history.is_empty());

        // Input with surrounding whitespace should be trimmed
        app.input_buffer = "  hello  ".to_string();
        app.input_cursor = 9;
        app.handle_input();
        assert_eq!(app.input_history, vec!["hello".to_string()]);
    }

    #[test]
    fn handle_input_resets_history_index() {
        let mut app = test_app();
        app.get_or_create_conversation("+1", "Alice", false);
        app.active_conversation = Some("+1".to_string());

        app.input_history = vec!["old".to_string()];
        app.history_index = Some(0);
        app.input_buffer = "new".to_string();
        app.input_cursor = 3;
        app.handle_input();

        assert_eq!(app.history_index, None);
    }

    #[test]
    fn apply_input_edit_up_down_routes_to_history() {
        let mut app = test_app();
        app.input_history = vec!["recalled".to_string()];
        app.input_buffer = "draft".to_string();

        assert!(app.apply_input_edit(KeyCode::Up));
        assert_eq!(app.input_buffer, "recalled");

        assert!(app.apply_input_edit(KeyCode::Down));
        assert_eq!(app.input_buffer, "draft");
    }

    // --- Receipt handling tests ---

    #[test]
    fn receipt_upgrades_outgoing_message_status() {
        let mut app = test_app();

        // Create a conversation with an outgoing message
        let conv_id = "+1";
        app.get_or_create_conversation(conv_id, "Alice", false);
        let ts_ms = 1700000000000_i64;
        if let Some(conv) = app.conversations.get_mut(conv_id) {
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
            });
        }

        // Delivery receipt
        app.handle_signal_event(SignalEvent::ReceiptReceived {
            sender: conv_id.to_string(),
            receipt_type: "DELIVERY".to_string(),
            timestamps: vec![ts_ms],
        });
        assert_eq!(
            app.conversations[conv_id].messages[0].status,
            Some(MessageStatus::Delivered)
        );

        // Read receipt — should upgrade
        app.handle_signal_event(SignalEvent::ReceiptReceived {
            sender: conv_id.to_string(),
            receipt_type: "READ".to_string(),
            timestamps: vec![ts_ms],
        });
        assert_eq!(
            app.conversations[conv_id].messages[0].status,
            Some(MessageStatus::Read)
        );
    }

    #[test]
    fn receipt_does_not_downgrade_status() {
        let mut app = test_app();

        let conv_id = "+1";
        app.get_or_create_conversation(conv_id, "Alice", false);
        let ts_ms = 1700000000000_i64;
        if let Some(conv) = app.conversations.get_mut(conv_id) {
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
            });
        }

        // Delivery receipt after Read — should NOT downgrade
        app.handle_signal_event(SignalEvent::ReceiptReceived {
            sender: conv_id.to_string(),
            receipt_type: "DELIVERY".to_string(),
            timestamps: vec![ts_ms],
        });
        assert_eq!(
            app.conversations[conv_id].messages[0].status,
            Some(MessageStatus::Read)
        );
    }

    #[test]
    fn send_timestamp_upgrades_sending_to_sent() {
        let mut app = test_app();

        let conv_id = "+1";
        app.get_or_create_conversation(conv_id, "Alice", false);
        let local_ts = 1700000000000_i64;
        let server_ts = 1700000000123_i64;

        if let Some(conv) = app.conversations.get_mut(conv_id) {
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
            });
        }

        // Register pending send
        app.pending_sends.insert("rpc-1".to_string(), (conv_id.to_string(), local_ts));

        app.handle_signal_event(SignalEvent::SendTimestamp {
            rpc_id: "rpc-1".to_string(),
            server_ts,
        });

        let msg = &app.conversations[conv_id].messages[0];
        assert_eq!(msg.status, Some(MessageStatus::Sent));
        assert_eq!(msg.timestamp_ms, server_ts);
    }

    #[test]
    fn send_failed_sets_failed_status() {
        let mut app = test_app();

        let conv_id = "+1";
        app.get_or_create_conversation(conv_id, "Alice", false);
        let local_ts = 1700000000000_i64;

        if let Some(conv) = app.conversations.get_mut(conv_id) {
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
            });
        }

        app.pending_sends.insert("rpc-1".to_string(), (conv_id.to_string(), local_ts));

        app.handle_signal_event(SignalEvent::SendFailed {
            rpc_id: "rpc-1".to_string(),
        });

        assert_eq!(
            app.conversations[conv_id].messages[0].status,
            Some(MessageStatus::Failed)
        );
    }

    #[test]
    fn incoming_messages_have_no_status() {
        let mut app = test_app();

        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: Some("Alice".to_string()),
            timestamp: chrono::Utc::now(),
            body: Some("hello".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        assert_eq!(app.conversations["+1"].messages[0].status, None);
    }

    #[test]
    fn receipt_before_send_timestamp_is_buffered_and_replayed() {
        let mut app = test_app();

        let conv_id = "+1";
        app.get_or_create_conversation(conv_id, "Alice", false);
        let local_ts = 1700000000000_i64;
        let server_ts = 1700000000123_i64;

        // Create outgoing message with local timestamp (Sending state)
        if let Some(conv) = app.conversations.get_mut(conv_id) {
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
            });
        }

        app.pending_sends.insert("rpc-1".to_string(), (conv_id.to_string(), local_ts));

        // Receipt arrives BEFORE SendTimestamp (references server_ts which we don't know yet)
        app.handle_signal_event(SignalEvent::ReceiptReceived {
            sender: conv_id.to_string(),
            receipt_type: "DELIVERY".to_string(),
            timestamps: vec![server_ts],
        });

        // Receipt should be buffered, message still Sending
        assert_eq!(
            app.conversations[conv_id].messages[0].status,
            Some(MessageStatus::Sending)
        );
        assert_eq!(app.pending_receipts.len(), 1);

        // Now SendTimestamp arrives — updates timestamp_ms and replays buffered receipts
        app.handle_signal_event(SignalEvent::SendTimestamp {
            rpc_id: "rpc-1".to_string(),
            server_ts,
        });

        // Message should now be Delivered (Sending → Sent by SendTimestamp, then → Delivered by replayed receipt)
        assert_eq!(
            app.conversations[conv_id].messages[0].status,
            Some(MessageStatus::Delivered)
        );
        assert!(app.pending_receipts.is_empty());
    }

    // --- Reaction tests ---

    #[test]
    fn handle_reaction_adds_to_message() {
        let mut app = test_app();
        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: Some("Alice".to_string()),
            timestamp: chrono::Utc::now(),
            body: Some("hello".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        let ts_ms = app.conversations["+1"].messages[0].timestamp_ms;

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

        let reactions = &app.conversations["+1"].messages[0].reactions;
        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0].emoji, "\u{1f44d}");
        // Sender should be resolved to display name
        assert_eq!(reactions[0].sender, "Bob");
    }

    #[test]
    fn handle_reaction_replaces_existing_from_same_sender() {
        let mut app = test_app();
        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: Some("Alice".to_string()),
            timestamp: chrono::Utc::now(),
            body: Some("hello".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        let ts_ms = app.conversations["+1"].messages[0].timestamp_ms;

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

        let reactions = &app.conversations["+1"].messages[0].reactions;
        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0].emoji, "\u{2764}\u{fe0f}");
    }

    #[test]
    fn handle_reaction_remove() {
        let mut app = test_app();
        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: Some("Alice".to_string()),
            timestamp: chrono::Utc::now(),
            body: Some("hello".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        let ts_ms = app.conversations["+1"].messages[0].timestamp_ms;

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
        assert_eq!(app.conversations["+1"].messages[0].reactions.len(), 1);

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
        assert_eq!(app.conversations["+1"].messages[0].reactions.len(), 0);
    }

    #[test]
    fn handle_reaction_on_own_message() {
        let mut app = test_app();
        // Send a message (outgoing) — simulate by creating conversation and pushing directly
        let conv_id = "+1";
        app.get_or_create_conversation(conv_id, "Alice", false);
        let ts_ms = 1700000000000_i64;
        if let Some(conv) = app.conversations.get_mut(conv_id) {
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

        let reactions = &app.conversations[conv_id].messages[0].reactions;
        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0].sender, "Alice");
    }

    #[test]
    fn handle_reaction_unknown_message_persists_to_db() {
        let mut app = test_app();
        app.get_or_create_conversation("+1", "Alice", false);

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
        assert!(app.conversations["+1"].messages.is_empty());
        // But it was persisted to DB
        let db_reactions = app.db.load_reactions("+1").unwrap();
        assert_eq!(db_reactions.len(), 1);
    }
}
