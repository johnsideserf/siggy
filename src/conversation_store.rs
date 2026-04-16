use chrono::{DateTime, Local, Utc};
use ratatui::text::Line;
use std::collections::{HashMap, HashSet};

use crate::db::Database;
use crate::signal::types::{
    Group, LinkPreview, Mention, MessageStatus, PollData, PollVote, Reaction, StyleType, TextStyle,
};

/// Log a database error via debug_log (no-op when --debug is off).
pub(crate) fn db_warn<T>(result: Result<T, impl std::fmt::Display>, context: &str) {
    if let Err(e) = result {
        crate::debug_log::logf(format_args!("db {context}: {e}"));
    }
}

/// Shorten a phone number for display: +15551234567 -> +1***4567
pub(crate) fn short_name(number: &str) -> String {
    let chars: Vec<char> = number.chars().collect();
    if chars.len() > 6 {
        let prefix: String = chars[..2].iter().collect();
        let last4: String = chars[chars.len() - 4..].iter().collect();
        format!("{prefix}***{last4}")
    } else {
        number.to_string()
    }
}

/// Quoted reply context attached to a message.
#[derive(Debug, Clone)]
pub struct Quote {
    pub author: String,
    pub body: String,
    pub timestamp_ms: i64,
    /// Original phone number / account ID for wire protocol (not resolved to display name)
    pub author_id: String,
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
    /// Byte ranges of @mentions in body (for styling)
    pub mention_ranges: Vec<(usize, usize)>,
    /// Byte ranges + style type for text styling (bold, italic, etc.)
    pub style_ranges: Vec<(usize, usize, StyleType)>,
    /// Quoted reply context
    pub quote: Option<Quote>,
    /// Whether this message has been edited
    pub is_edited: bool,
    /// Whether this message has been remotely deleted
    pub is_deleted: bool,
    /// Whether this message is pinned
    pub is_pinned: bool,
    /// Phone number / ID of the sender (for wire protocol; "you" for outgoing)
    pub sender_id: String,
    /// Disappearing message timer (seconds, 0 = no expiration)
    pub expires_in_seconds: i64,
    /// When the expiration countdown started (epoch ms, 0 = not started)
    pub expiration_start_ms: i64,
    /// Poll data (for poll-create messages)
    pub poll_data: Option<PollData>,
    /// Votes received for this poll
    pub poll_votes: Vec<PollVote>,
    /// Link preview metadata
    pub preview: Option<LinkPreview>,
    /// Pre-rendered halfblock image lines for link preview thumbnail
    pub preview_image_lines: Option<Vec<Line<'static>>>,
    /// Local filesystem path for native protocol link preview thumbnail
    pub preview_image_path: Option<String>,
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
    /// Disappearing message timer in seconds (0 = off)
    pub expiration_timer: i64,
    /// Whether this conversation has been accepted (message requests are unaccepted)
    pub accepted: bool,
}

impl Conversation {
    /// Whether this conversation is stale and should be hidden from the default sidebar view.
    /// A conversation is stale if it has no messages AND has no meaningful name
    /// (e.g. empty/abandoned groups, or contacts with only a UUID hash).
    pub fn is_stale(&self) -> bool {
        if !self.messages.is_empty() {
            return false;
        }
        if self.is_group {
            // Group with no messages and no resolved name (name is the raw group ID)
            self.name.is_empty() || self.name == self.id
        } else {
            // 1:1 contact with no messages and no usable name:
            // keep if name is a phone number (+...), hide if name is just the raw ID
            // (a UUID hash or "..." with no real identity)
            !self.name.starts_with('+') && self.name == self.id
        }
    }

    /// Binary-search for a message by timestamp (messages are sorted by `timestamp_ms`).
    pub fn find_msg_idx(&self, ts: i64) -> Option<usize> {
        let end = self.messages.partition_point(|m| m.timestamp_ms <= ts);
        if end > 0 && self.messages[end - 1].timestamp_ms == ts {
            Some(end - 1)
        } else {
            None
        }
    }
}

/// Owns all conversation data: conversations, ordering, contact names, groups, and read markers.
pub struct ConversationStore {
    /// All conversations keyed by phone number (1:1) or group ID (groups).
    pub conversations: HashMap<String, Conversation>,
    /// Ordered list of conversation IDs for sidebar display.
    pub conversation_order: Vec<String>,
    /// Contact/group name lookup (number/id → display name).
    pub contact_names: HashMap<String, String>,
    /// UUID → display name mapping (built from contact list).
    pub uuid_to_name: HashMap<String, String>,
    /// Phone number → UUID mapping (for sending mentions).
    pub number_to_uuid: HashMap<String, String>,
    /// Last-read message index per conversation (for unread marker).
    pub last_read_index: HashMap<String, usize>,
    /// Groups indexed by group_id (with member lists for @mention autocomplete).
    pub groups: HashMap<String, Group>,
    /// Conversations that have more messages in the database to load.
    pub has_more_messages: HashSet<String>,
}

impl ConversationStore {
    pub fn new() -> Self {
        Self {
            conversations: HashMap::new(),
            conversation_order: Vec::new(),
            contact_names: HashMap::new(),
            uuid_to_name: HashMap::new(),
            number_to_uuid: HashMap::new(),
            last_read_index: HashMap::new(),
            groups: HashMap::new(),
            has_more_messages: HashSet::new(),
        }
    }

    /// Ensure a conversation exists; create it if not. Returns a mutable ref.
    pub fn get_or_create_conversation(
        &mut self,
        id: &str,
        name: &str,
        is_group: bool,
        db: &Database,
    ) -> &mut Conversation {
        if !self.conversations.contains_key(id) {
            // New conversation — always persist
            db_warn(
                db.upsert_conversation(id, name, is_group),
                "upsert_conversation",
            );
            self.conversations.insert(
                id.to_string(),
                Conversation {
                    name: name.to_string(),
                    id: id.to_string(),
                    messages: Vec::new(),
                    unread: 0,
                    is_group,
                    expiration_timer: 0,
                    accepted: true,
                },
            );
            self.conversation_order.push(id.to_string());
        } else if name != id {
            // Existing conversation — only update if we have a real display name
            let conv = self.conversations.get_mut(id).unwrap();
            if conv.name != name {
                conv.name = name.to_string();
                db_warn(
                    db.upsert_conversation(id, name, is_group),
                    "upsert_conversation",
                );
            }
        }
        self.conversations.get_mut(id).unwrap()
    }

    /// Move a conversation to the top of the sidebar order.
    /// Returns `true` if the conversation was actually reordered.
    pub fn move_conversation_to_top(&mut self, id: &str) -> bool {
        let pos = match self.conversation_order.iter().position(|c| c == id) {
            Some(pos) => pos,
            None => return false,
        };

        self.conversation_order.remove(pos);
        self.conversation_order.insert(0, id.to_string());
        true
    }

    /// Total unread count across all conversations.
    pub fn total_unread(&self) -> usize {
        self.conversations.values().map(|c| c.unread).sum()
    }

    /// Resolve U+FFFC placeholders in a message body using bodyRanges mentions.
    /// Returns (resolved_body, mention_byte_ranges) where mention_byte_ranges are
    /// (start, end) byte offsets of each `@Name` in the resolved body.
    pub fn resolve_mentions(
        &self,
        body: &str,
        mentions: &[Mention],
    ) -> (String, Vec<(usize, usize)>) {
        if mentions.is_empty() {
            return (body.to_string(), Vec::new());
        }

        // Sort mentions by start descending so replacements don't shift earlier offsets
        let mut sorted: Vec<&Mention> = mentions.iter().collect();
        sorted.sort_by_key(|b| std::cmp::Reverse(b.start));

        // Convert body to UTF-16 for offset mapping
        let utf16: Vec<u16> = body.encode_utf16().collect();
        let mut result_utf16 = utf16.clone();
        for mention in &sorted {
            if mention.start >= result_utf16.len() {
                continue;
            }
            let name = self
                .uuid_to_name
                .get(&mention.uuid)
                .cloned()
                .unwrap_or_else(|| {
                    // Truncated UUID fallback
                    let short = if mention.uuid.len() > 8 {
                        &mention.uuid[..8]
                    } else {
                        &mention.uuid
                    };
                    short.to_string()
                });
            let replacement = format!("@{name}");
            let replacement_utf16: Vec<u16> = replacement.encode_utf16().collect();
            let end = (mention.start + mention.length).min(result_utf16.len());
            result_utf16.splice(mention.start..end, replacement_utf16);
        }

        let resolved = String::from_utf16_lossy(&result_utf16);

        // Compute byte ranges for each @Name in the resolved string
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        let mut sorted_fwd: Vec<&Mention> = mentions.iter().collect();
        sorted_fwd.sort_by_key(|m| m.start);

        // Re-build with forward pass to get accurate byte offsets
        let resolved_utf16: Vec<u16> = resolved.encode_utf16().collect();
        let mut byte_pos = 0;
        let resolved_bytes = resolved.as_bytes();

        // Build utf16_offset -> byte_offset mapping
        let mut utf16_to_byte: Vec<usize> = Vec::with_capacity(resolved_utf16.len() + 1);
        for ch in resolved.chars() {
            let utf16_len = ch.len_utf16();
            let utf8_len = ch.len_utf8();
            for _ in 0..utf16_len {
                utf16_to_byte.push(byte_pos);
            }
            byte_pos += utf8_len;
        }
        utf16_to_byte.push(byte_pos); // sentinel for end

        // Calculate where each mention ended up after all replacements
        let mut offset_shift: i64 = 0;
        for mention in &sorted_fwd {
            let adjusted_start = (mention.start as i64 + offset_shift) as usize;
            let name = self
                .uuid_to_name
                .get(&mention.uuid)
                .cloned()
                .unwrap_or_else(|| {
                    let short = if mention.uuid.len() > 8 {
                        &mention.uuid[..8]
                    } else {
                        &mention.uuid
                    };
                    short.to_string()
                });
            let replacement_utf16_len = format!("@{name}").encode_utf16().count();
            let byte_start = utf16_to_byte
                .get(adjusted_start)
                .copied()
                .unwrap_or(resolved_bytes.len());
            let byte_end = utf16_to_byte
                .get(adjusted_start + replacement_utf16_len)
                .copied()
                .unwrap_or(resolved_bytes.len());
            ranges.push((byte_start, byte_end));
            // This mention replaced `mention.length` UTF-16 units with `replacement_utf16_len`
            offset_shift += replacement_utf16_len as i64 - mention.length as i64;
        }

        (resolved, ranges)
    }

    /// Convert text style ranges from UTF-16 offsets (on the original body) to byte offsets
    /// on the resolved body (after mention replacement). Mentions may change the body length,
    /// so we need to account for the offset shift caused by mention replacements.
    pub fn resolve_text_styles(
        &self,
        resolved_body: &str,
        text_styles: &[TextStyle],
        mentions: &[Mention],
    ) -> Vec<(usize, usize, StyleType)> {
        if text_styles.is_empty() {
            return Vec::new();
        }

        // Calculate how mention replacements shift UTF-16 offsets.
        let mut mention_shifts: Vec<(usize, i64)> = Vec::new();
        if !mentions.is_empty() {
            let mut sorted_mentions: Vec<&Mention> = mentions.iter().collect();
            sorted_mentions.sort_by_key(|m| m.start);
            let mut cumulative: i64 = 0;
            for m in &sorted_mentions {
                let name = self.uuid_to_name.get(&m.uuid).cloned().unwrap_or_else(|| {
                    let short = if m.uuid.len() > 8 {
                        &m.uuid[..8]
                    } else {
                        &m.uuid
                    };
                    short.to_string()
                });
                let replacement_utf16_len = format!("@{name}").encode_utf16().count() as i64;
                let original_len = m.length as i64;
                cumulative += replacement_utf16_len - original_len;
                mention_shifts.push((m.start + m.length, cumulative));
            }
        }

        // For a given original UTF-16 offset, compute the shifted offset after mention replacements
        let shift_offset = |orig: usize| -> usize {
            let mut shift: i64 = 0;
            for &(boundary, cum_shift) in &mention_shifts {
                if orig >= boundary {
                    shift = cum_shift;
                } else {
                    break;
                }
            }
            (orig as i64 + shift) as usize
        };

        // Build UTF-16 to byte offset mapping for the resolved body
        let mut utf16_to_byte: Vec<usize> = Vec::new();
        let mut byte_pos = 0;
        for ch in resolved_body.chars() {
            for _ in 0..ch.len_utf16() {
                utf16_to_byte.push(byte_pos);
            }
            byte_pos += ch.len_utf8();
        }
        utf16_to_byte.push(byte_pos); // sentinel

        let body_byte_len = resolved_body.len();

        text_styles
            .iter()
            .filter_map(|ts| {
                let shifted_start = shift_offset(ts.start);
                let shifted_end = shift_offset(ts.start + ts.length);
                let byte_start = utf16_to_byte
                    .get(shifted_start)
                    .copied()
                    .unwrap_or(body_byte_len);
                let byte_end = utf16_to_byte
                    .get(shifted_end)
                    .copied()
                    .unwrap_or(body_byte_len);
                if byte_start < byte_end && byte_end <= body_byte_len {
                    Some((byte_start, byte_end, ts.style))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Re-resolve reaction sender names and quote authors using the latest contact_names.
    pub fn resolve_stored_names(&mut self, account: &str) {
        // Build phone→name lookup from message sender_id fields
        let mut phone_to_name: HashMap<String, String> = HashMap::new();
        for conv in self.conversations.values() {
            for msg in &conv.messages {
                if !msg.sender_id.is_empty()
                    && msg.sender_id != "you"
                    && !msg.sender.is_empty()
                    && msg.sender != msg.sender_id
                {
                    phone_to_name.insert(msg.sender_id.clone(), msg.sender.clone());
                }
            }
        }

        // Merge contact_names on top (takes priority)
        for (phone, name) in &self.contact_names {
            phone_to_name.insert(phone.clone(), name.clone());
        }

        // Resolve reaction senders and quote authors
        for conv in self.conversations.values_mut() {
            for msg in &mut conv.messages {
                // Resolve reaction senders
                for reaction in &mut msg.reactions {
                    if reaction.sender == "you" {
                        continue;
                    }
                    if reaction.sender == account {
                        reaction.sender = "you".to_string();
                    } else if let Some(name) = phone_to_name.get(&reaction.sender) {
                        reaction.sender = name.clone();
                    }
                }
                // Resolve quote author
                if let Some(ref mut quote) = msg.quote {
                    if quote.author == account {
                        quote.author = "you".to_string();
                    } else if let Some(name) = phone_to_name.get(&quote.author) {
                        quote.author = name.clone();
                    }
                }
            }
        }
    }
}
