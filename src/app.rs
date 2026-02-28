use chrono::{DateTime, Local, Utc};
use std::collections::HashMap;
use std::time::Instant;

use crate::db::Database;
use crate::input::{self, InputAction, HELP_TEXT};
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
    /// Whether sidebar is visible
    pub sidebar_visible: bool,
    /// Scroll offset for messages (0 = bottom)
    pub scroll_offset: usize,
    /// Status bar message
    pub status_message: String,
    /// Whether the app should quit
    pub should_quit: bool,
    /// Our own account number for identifying outgoing messages
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
}

impl App {
    pub fn new(account: String, db: Database) -> Self {
        Self {
            conversations: HashMap::new(),
            conversation_order: Vec::new(),
            active_conversation: None,
            input_buffer: String::new(),
            input_cursor: 0,
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
            self.conversations.insert(id.clone(), conv);
            // Derive last_read_index from unread count
            if msg_count > 0 {
                let read_index = msg_count.saturating_sub(unread);
                self.last_read_index.insert(id, read_index);
            }
        }

        self.conversation_order = order;
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
            SignalEvent::TypingIndicator { sender, is_typing } => {
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
            // Outgoing 1:1 — conversation is keyed by recipient, but we don't
            // know the recipient from the event alone; skip for now.
            return;
        } else {
            msg.source.clone()
        };

        // Ensure conversation exists (drop the mutable ref immediately)
        self.get_or_create_conversation(
            &conv_id,
            msg.group_name
                .as_deref()
                .or(msg.source_name.as_deref())
                .unwrap_or(&conv_id),
            msg.group_id.is_some(),
        );

        let sender_display = if msg.is_outgoing {
            "you".to_string()
        } else {
            msg.source_name
                .clone()
                .unwrap_or_else(|| short_name(&msg.source))
        };

        let ts_rfc3339 = msg.timestamp.to_rfc3339();

        // Add text body
        if let Some(ref body) = msg.body {
            if let Some(conv) = self.conversations.get_mut(&conv_id) {
                conv.messages.push(DisplayMessage {
                    sender: sender_display.clone(),
                    timestamp: msg.timestamp,
                    body: body.clone(),
                    is_system: false,
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
            let path_info = att
                .local_path
                .as_deref()
                .map(|p| format!(" -> {p}"))
                .unwrap_or_default();
            let att_body = format!("[attachment: {label}]{path_info}");
            if let Some(conv) = self.conversations.get_mut(&conv_id) {
                conv.messages.push(DisplayMessage {
                    sender: sender_display.clone(),
                    timestamp: msg.timestamp,
                    body: att_body.clone(),
                    is_system: false,
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

        let is_active = self
            .active_conversation
            .as_ref()
            .map(|a| a == &conv_id)
            .unwrap_or(false);

        if !is_active {
            if let Some(c) = self.conversations.get_mut(&conv_id) {
                c.unread += 1;
            }
        }
    }

    fn handle_contact_list(&mut self, contacts: Vec<Contact>) {
        for contact in contacts {
            let name = contact.name.as_deref().unwrap_or(&contact.number);
            let conv = self.get_or_create_conversation(&contact.number, name, false);
            // Update name if contact provides a better one
            if let Some(ref contact_name) = contact.name {
                if !contact_name.is_empty() && conv.name != *contact_name {
                    conv.name = contact_name.clone();
                    let _ = self.db.upsert_conversation(&contact.number, contact_name, false);
                }
            }
        }
    }

    fn handle_group_list(&mut self, groups: Vec<Group>) {
        for group in groups {
            let conv = self.get_or_create_conversation(&group.id, &group.name, true);
            // Update name if group provides a better one
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
            InputAction::Help => {
                self.add_system_message(HELP_TEXT);
            }
            InputAction::Unknown(msg) => {
                self.status_message = msg;
            }
        }
        None
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

    fn add_system_message(&mut self, text: &str) {
        if let Some(ref conv_id) = self.active_conversation {
            if let Some(conv) = self.conversations.get_mut(conv_id) {
                conv.messages.push(DisplayMessage {
                    sender: String::new(),
                    timestamp: Utc::now(),
                    body: text.to_string(),
                    is_system: true,
                });
            }
        } else {
            // No active conversation — show in status
            self.status_message = text.lines().next().unwrap_or("").to_string();
        }
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

    // --- Test 2: Sidebar populates with contacts and groups on startup ---

    #[test]
    fn contact_list_creates_conversations() {
        let mut app = test_app();
        assert!(app.conversations.is_empty());

        app.handle_signal_event(SignalEvent::ContactList(vec![
            Contact { number: "+1".to_string(), name: Some("Alice".to_string()) },
            Contact { number: "+2".to_string(), name: Some("Bob".to_string()) },
        ]));

        assert_eq!(app.conversations.len(), 2);
        assert_eq!(app.conversation_order.len(), 2);
        assert_eq!(app.conversations["+1"].name, "Alice");
        assert_eq!(app.conversations["+2"].name, "Bob");
        assert!(!app.conversations["+1"].is_group);
    }

    #[test]
    fn group_list_creates_conversations() {
        let mut app = test_app();

        app.handle_signal_event(SignalEvent::GroupList(vec![
            Group { id: "g1".to_string(), name: "Family".to_string(), members: vec![] },
            Group { id: "g2".to_string(), name: "Work".to_string(), members: vec![] },
        ]));

        assert_eq!(app.conversations.len(), 2);
        assert_eq!(app.conversations["g1"].name, "Family");
        assert_eq!(app.conversations["g2"].name, "Work");
        assert!(app.conversations["g1"].is_group);
        assert!(app.conversations["g2"].is_group);
    }

    // --- Test 3: Existing conversations with messages still appear first ---

    #[test]
    fn existing_conversations_appear_before_new_contacts() {
        let mut app = test_app();

        // Simulate a pre-existing conversation (as if loaded from DB)
        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: Some("Alice".to_string()),
            timestamp: chrono::Utc::now(),
            body: Some("hello".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert_eq!(app.conversation_order, vec!["+1"]);

        // Now contacts arrive — new ones appended after existing
        app.handle_signal_event(SignalEvent::ContactList(vec![
            Contact { number: "+1".to_string(), name: Some("Alice".to_string()) },
            Contact { number: "+2".to_string(), name: Some("Bob".to_string()) },
        ]));

        // +1 should still be first (already existed), +2 appended
        assert_eq!(app.conversation_order, vec!["+1", "+2"]);
    }

    // --- Test 4: Contact names display correctly ---

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
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert_eq!(app.conversations["+15551234567"].name, "+15551234567");

        // Contact list arrives with a proper name
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
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));
        assert_eq!(app.conversations["+1"].name, "Alice");

        // Contact arrives with no name — should NOT overwrite
        app.handle_signal_event(SignalEvent::ContactList(vec![
            Contact { number: "+1".to_string(), name: None },
        ]));

        assert_eq!(app.conversations["+1"].name, "Alice");
    }

    // --- Test 5: Groups have is_group=true (UI renders # prefix based on this) ---

    #[test]
    fn groups_are_marked_as_groups() {
        let mut app = test_app();

        app.handle_signal_event(SignalEvent::GroupList(vec![
            Group { id: "g1".to_string(), name: "Family".to_string(), members: vec![] },
        ]));
        app.handle_signal_event(SignalEvent::ContactList(vec![
            Contact { number: "+1".to_string(), name: Some("Alice".to_string()) },
        ]));

        assert!(app.conversations["g1"].is_group);
        assert!(!app.conversations["+1"].is_group);
    }

    // --- Test 6: Receiving a message from a pre-loaded contact works (no duplicates) ---

    #[test]
    fn message_from_preloaded_contact_no_duplicate_conversation() {
        let mut app = test_app();

        // Contacts arrive first
        app.handle_signal_event(SignalEvent::ContactList(vec![
            Contact { number: "+1".to_string(), name: Some("Alice".to_string()) },
        ]));
        assert_eq!(app.conversations.len(), 1);
        assert_eq!(app.conversation_order.len(), 1);

        // Then a message arrives from the same contact
        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: Some("Alice".to_string()),
            timestamp: chrono::Utc::now(),
            body: Some("hello!".to_string()),
            attachments: vec![],
            group_id: None,
            group_name: None,
            is_outgoing: false,
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        // Still only 1 conversation, not 2
        assert_eq!(app.conversations.len(), 1);
        assert_eq!(app.conversation_order.len(), 1);
        // Message was added to the existing conversation
        assert_eq!(app.conversations["+1"].messages.len(), 1);
        assert_eq!(app.conversations["+1"].messages[0].body, "hello!");
    }

    #[test]
    fn message_from_preloaded_group_no_duplicate() {
        let mut app = test_app();

        // Group loaded first
        app.handle_signal_event(SignalEvent::GroupList(vec![
            Group { id: "g1".to_string(), name: "Family".to_string(), members: vec!["+1".to_string()] },
        ]));
        assert_eq!(app.conversations.len(), 1);

        // Message arrives in that group
        let msg = SignalMessage {
            source: "+1".to_string(),
            source_name: Some("Alice".to_string()),
            timestamp: chrono::Utc::now(),
            body: Some("hey family".to_string()),
            attachments: vec![],
            group_id: Some("g1".to_string()),
            group_name: Some("Family".to_string()),
            is_outgoing: false,
        };
        app.handle_signal_event(SignalEvent::MessageReceived(msg));

        assert_eq!(app.conversations.len(), 1);
        assert_eq!(app.conversations["g1"].messages.len(), 1);
    }
}
