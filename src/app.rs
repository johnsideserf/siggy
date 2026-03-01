use chrono::{DateTime, Local, Utc};
use crossterm::event::KeyCode;
use ratatui::text::Line;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use crate::db::Database;
use crate::image_render;
use crate::input::{self, InputAction, COMMANDS};
use crate::signal::types::{Contact, Group, SignalEvent, SignalMessage};

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
    /// Show inline halfblock image previews in chat
    pub inline_images: bool,
    /// Link regions detected in the last rendered frame (for OSC 8 injection)
    pub link_regions: Vec<crate::ui::LinkRegion>,
    /// Incognito mode — in-memory DB, no local persistence
    pub incognito: bool,
}

pub const SETTINGS_ITEMS: &[&str] = &[
    "Direct message notifications",
    "Group message notifications",
    "Sidebar visible",
    "Inline image previews",
];

impl App {
    pub fn toggle_setting(&mut self, index: usize) {
        match index {
            0 => self.notify_direct = !self.notify_direct,
            1 => self.notify_group = !self.notify_group,
            2 => self.sidebar_visible = !self.sidebar_visible,
            3 => self.inline_images = !self.inline_images,
            _ => {}
        }
    }

    pub fn setting_value(&self, index: usize) -> bool {
        match index {
            0 => self.notify_direct,
            1 => self.notify_group,
            2 => self.sidebar_visible,
            3 => self.inline_images,
            _ => false,
        }
    }

    /// Handle a key press while the settings overlay is open.
    pub fn handle_settings_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.settings_index < SETTINGS_ITEMS.len() - 1 {
                    self.settings_index += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.settings_index = self.settings_index.saturating_sub(1);
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                self.toggle_setting(self.settings_index);
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.show_settings = false;
            }
            _ => {}
        }
    }

    /// Handle a key press while the autocomplete popup is visible.
    /// Returns `Some((recipient, body, is_group))` when the user submits a command
    /// that requires sending a message. Returns `None` otherwise.
    pub fn handle_autocomplete_key(&mut self, code: KeyCode) -> Option<(String, String, bool)> {
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
            inline_images: true,
            link_regions: Vec::new(),
            incognito: false,
        }
    }

    /// Load conversations and messages from the database on startup
    pub fn load_from_db(&mut self) -> anyhow::Result<()> {
        let conv_data = self.db.load_conversations(500)?;
        let order = self.db.load_conversation_order()?;

        for (mut conv, unread) in conv_data {
            let id = conv.id.clone();
            let msg_count = conv.messages.len();
            conv.unread = unread;

            // Re-render image previews from stored paths
            for msg in &mut conv.messages {
                if msg.body.starts_with("[image:") {
                    let path_str = if let Some(uri_pos) = msg.body.find("file:///") {
                        Some(file_uri_to_path(&msg.body[uri_pos..]))
                    } else if let Some(arrow_pos) = msg.body.find(" -> ") {
                        Some(msg.body[arrow_pos + 4..].trim_end_matches(']').to_string())
                    } else {
                        None
                    };
                    if let Some(p) = path_str {
                        let path = Path::new(&p);
                        if self.inline_images && path.exists() {
                            msg.image_lines = image_render::render_image(path, 40);
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
                let _ = self.db.save_read_marker(&conv_id, rowid);
            }
        }
    }

    /// Remove typing indicators older than 5 seconds
    pub fn cleanup_typing(&mut self) {
        let now = Instant::now();
        self.typing_indicators
            .retain(|_, ts| now.duration_since(*ts).as_secs() < 5);
    }

    /// Handle an event from signal-cli
    pub fn handle_signal_event(&mut self, event: SignalEvent) {
        match event {
            SignalEvent::MessageReceived(msg) => self.handle_message(msg),
            SignalEvent::ReceiptReceived { .. } => {}
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

        // Add text body
        if let Some(ref body) = msg.body {
            if let Some(conv) = self.conversations.get_mut(&conv_id) {
                conv.messages.push(DisplayMessage {
                    sender: sender_display.clone(),
                    timestamp: msg.timestamp,
                    body: body.clone(),
                    is_system: false,
                    image_lines: None,
                });
            }
            let _ = self.db.insert_message(
                &conv_id,
                &sender_display,
                &ts_rfc3339,
                body,
                false,
            );
        }

        // Add attachment notices
        for att in &msg.attachments {
            let label = att
                .filename
                .as_deref()
                .unwrap_or(&att.content_type);

            let is_image = matches!(
                att.content_type.as_str(),
                "image/jpeg" | "image/png" | "image/gif" | "image/webp"
            );

            if is_image {
                // Try to render inline image preview (only when enabled)
                let rendered = if self.inline_images {
                    att.local_path.as_deref().and_then(|p| {
                        image_render::render_image(Path::new(p), 40)
                    })
                } else {
                    None
                };

                let path_info = att.local_path.as_deref()
                    .map(|p| format!(" {}", path_to_file_uri(p)))
                    .unwrap_or_default();

                let att_body = if rendered.is_some() {
                    format!("[image: {label}]{path_info}")
                } else {
                    // Render failed — show path as fallback
                    format!("[image: {label}]{path_info}")
                };

                if let Some(conv) = self.conversations.get_mut(&conv_id) {
                    conv.messages.push(DisplayMessage {
                        sender: sender_display.clone(),
                        timestamp: msg.timestamp,
                        body: att_body.clone(),
                        is_system: false,
                        image_lines: rendered,
                    });
                }
                let _ = self.db.insert_message(
                    &conv_id,
                    &sender_display,
                    &ts_rfc3339,
                    &att_body,
                    false,
                );
            } else {
                let path_info = att.local_path.as_deref()
                    .map(|p| format!(" {}", path_to_file_uri(p)))
                    .unwrap_or_default();
                let att_body = format!("[attachment: {label}]{path_info}");
                if let Some(conv) = self.conversations.get_mut(&conv_id) {
                    conv.messages.push(DisplayMessage {
                        sender: sender_display.clone(),
                        timestamp: msg.timestamp,
                        body: att_body.clone(),
                        is_system: false,
                        image_lines: None,
                    });
                }
                let _ = self.db.insert_message(
                    &conv_id,
                    &sender_display,
                    &ts_rfc3339,
                    &att_body,
                    false,
                );
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
                        let _ = self.db.upsert_conversation(&contact.number, contact_name, false);
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
                let _ = self.db.upsert_conversation(&group.id, &group.name, true);
            }
        }
    }

    fn get_or_create_conversation(
        &mut self,
        id: &str,
        name: &str,
        is_group: bool,
    ) -> &mut Conversation {
        let _ = self.db.upsert_conversation(id, name, is_group);
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

    /// Handle a line of user input; returns Some(command) if we need to send a message
    pub fn handle_input(&mut self) -> Option<(String, String, bool)> {
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
                    if let Some(conv) = self.conversations.get_mut(&conv_id) {
                        conv.messages.push(DisplayMessage {
                            sender: "you".to_string(),
                            timestamp: now,
                            body: text.clone(),
                            is_system: false,
                            image_lines: None,
                        });
                    }
                    let _ = self.db.insert_message(
                        &conv_id,
                        "you",
                        &now.to_rfc3339(),
                        &text,
                        false,
                    );
                    self.scroll_offset = 0;
                    return Some((conv_id, text, is_group));
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
                        let _ = self.db.set_muted(&conv_id, false);
                    } else {
                        let name = self.conversations.get(&conv_id)
                            .map(|c| c.name.as_str()).unwrap_or(&conv_id);
                        self.status_message = format!("muted {name}");
                        self.muted_conversations.insert(conv_id.clone());
                        let _ = self.db.set_muted(&conv_id, true);
                    }
                } else {
                    self.status_message = "no active conversation to mute".to_string();
                }
            }
            InputAction::Settings => {
                self.show_settings = true;
                self.settings_index = 0;
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
}
