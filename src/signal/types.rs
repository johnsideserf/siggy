//! Wire types shared between [`super::client`] and the binary's `app` module.
//!
//! Defines [`SignalEvent`] (the channel payload), [`SignalMessage`],
//! [`Contact`], [`Group`], JSON-RPC framing structs, and the per-message
//! status / trust / reaction value types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Delivery/read status for outgoing messages.
/// Ordered so that PartialOrd gives natural upgrade semantics (only increase, never downgrade).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MessageStatus {
    Failed,    // send failed
    Sending,   // in transit to server
    Sent,      // server confirmed
    Delivered, // on recipient's device
    Read,      // read by recipient
    Viewed,    // viewed (voice/media)
}

impl MessageStatus {
    /// Convert to integer for DB storage.
    pub fn to_i32(self) -> i32 {
        match self {
            MessageStatus::Failed => 1,
            MessageStatus::Sending => 2,
            MessageStatus::Sent => 3,
            MessageStatus::Delivered => 4,
            MessageStatus::Read => 5,
            MessageStatus::Viewed => 6,
        }
    }

    /// Convert from DB integer. Returns None for 0 (incoming/no status).
    pub fn from_i32(v: i32) -> Option<Self> {
        match v {
            1 => Some(MessageStatus::Failed),
            2 => Some(MessageStatus::Sending),
            3 => Some(MessageStatus::Sent),
            4 => Some(MessageStatus::Delivered),
            5 => Some(MessageStatus::Read),
            6 => Some(MessageStatus::Viewed),
            _ => None,
        }
    }
}

/// Kind of delivery receipt from signal-cli. Replaces a stringly-typed
/// `receipt_type` whose conversion to [`MessageStatus`] had a silent
/// `_ => return` for any unrecognized value (#500): a casing or spelling drift
/// between the parser and the handler dropped receipts silently. The mapping to
/// `MessageStatus` is now total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptKind {
    Delivery,
    Read,
    Viewed,
}

impl ReceiptKind {
    /// The outgoing-message status this receipt upgrades to.
    pub fn status(self) -> MessageStatus {
        match self {
            Self::Delivery => MessageStatus::Delivered,
            Self::Read => MessageStatus::Read,
            Self::Viewed => MessageStatus::Viewed,
        }
    }

    /// Wire label, for logging and the older signal-cli `type` string field.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Delivery => "DELIVERY",
            Self::Read => "READ",
            Self::Viewed => "VIEWED",
        }
    }

    /// Parse the older signal-cli `type` string (case-insensitive), or `None`
    /// for an unrecognized value.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "DELIVERY" => Some(Self::Delivery),
            "READ" => Some(Self::Read),
            "VIEWED" => Some(Self::Viewed),
            _ => None,
        }
    }
}

/// Trust level for a contact's identity key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustLevel {
    Untrusted,
    TrustedUnverified,
    TrustedVerified,
}

impl TrustLevel {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "UNTRUSTED" => TrustLevel::Untrusted,
            "TRUSTED_VERIFIED" => TrustLevel::TrustedVerified,
            _ => TrustLevel::TrustedUnverified,
        }
    }
}

/// Identity key information for a contact.
#[derive(Debug, Clone)]
pub struct IdentityInfo {
    pub number: Option<String>,
    /// Only read by tests today; kept on the wire type for parity with
    /// signal-cli's identity payload.
    #[allow(dead_code)]
    pub uuid: Option<String>,
    pub fingerprint: String,
    pub safety_number: String,
    pub trust_level: TrustLevel,
    #[allow(dead_code)]
    pub added_timestamp: i64,
}

/// A single emoji reaction on a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaction {
    pub emoji: String,
    pub sender: String,
}

/// Poll data attached to a poll-create message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollData {
    pub question: String,
    pub options: Vec<PollOption>,
    pub allow_multiple: bool,
    pub closed: bool,
}

/// A single option in a poll.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollOption {
    pub id: i64,
    pub text: String,
}

/// A vote on a poll from a specific user.
#[derive(Debug, Clone)]
pub struct PollVote {
    pub voter: String,
    pub voter_name: Option<String>,
    pub option_indexes: Vec<i64>,
    pub vote_count: i64,
}

/// Events received from signal-cli
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum SignalEvent {
    MessageReceived(SignalMessage),
    ReceiptReceived {
        sender: String,
        receipt_type: ReceiptKind,
        timestamps: Vec<i64>,
    },
    SendTimestamp {
        rpc_id: String,
        server_ts: i64,
    },
    SendFailed {
        rpc_id: String,
    },
    TypingIndicator {
        sender: String,
        sender_name: Option<String>,
        is_typing: bool,
        group_id: Option<String>,
    },
    ReactionReceived {
        conv_id: String,
        emoji: String,
        sender: String,
        sender_name: Option<String>,
        target_author: String,
        target_timestamp: i64,
        is_remove: bool,
    },
    EditReceived {
        conv_id: String,
        sender: String,
        sender_name: Option<String>,
        target_timestamp: i64,
        new_body: String,
        #[allow(dead_code)]
        new_timestamp: i64,
        #[allow(dead_code)]
        is_outgoing: bool,
    },
    RemoteDeleteReceived {
        conv_id: String,
        #[allow(dead_code)]
        sender: String,
        target_timestamp: i64,
    },
    PinReceived {
        conv_id: String,
        sender: String,
        sender_name: Option<String>,
        #[allow(dead_code)]
        target_author: String,
        target_timestamp: i64,
    },
    UnpinReceived {
        conv_id: String,
        sender: String,
        sender_name: Option<String>,
        #[allow(dead_code)]
        target_author: String,
        target_timestamp: i64,
    },
    PollCreated {
        conv_id: String,
        timestamp: i64,
        poll_data: PollData,
    },
    PollVoteReceived {
        conv_id: String,
        target_timestamp: i64,
        voter: String,
        voter_name: Option<String>,
        option_indexes: Vec<i64>,
        vote_count: i64,
    },
    PollTerminated {
        conv_id: String,
        target_timestamp: i64,
    },
    SystemMessage {
        conv_id: String,
        body: String,
        timestamp: DateTime<Utc>,
        timestamp_ms: i64,
    },
    ExpirationTimerChanged {
        conv_id: String,
        seconds: i64,
        body: String,
        timestamp: DateTime<Utc>,
        timestamp_ms: i64,
    },
    ReadSyncReceived {
        read_messages: Vec<(String, i64)>,
    },
    ContactList(Vec<Contact>),
    GroupList(Vec<Group>),
    IdentityList(Vec<IdentityInfo>),
    /// getUserStatus response: registration status per queried recipient,
    /// used for username → uuid resolution (#612).
    UserStatusList(Vec<UserStatus>),
    Error(String),
    /// The signal-cli stdout reader reached EOF (the child exited). Emitted
    /// explicitly so the app can mark itself disconnected and fail any in-flight
    /// sends, instead of relying solely on the mpsc channel closing (#497).
    Disconnected,
}

impl SignalEvent {
    /// Format this event for debug logging with PII redacted.
    pub fn redacted_summary(&self) -> String {
        use crate::debug_log::{mask_body, mask_phone};
        match self {
            Self::MessageReceived(msg) => format!(
                "MessageReceived(from={}, body={}, attachments={}, group={})",
                mask_phone(&msg.source),
                msg.body.as_deref().map_or("[none]".to_string(), mask_body),
                msg.attachments.len(),
                msg.group_id.is_some(),
            ),
            Self::ReceiptReceived {
                sender,
                receipt_type,
                timestamps,
            } => format!(
                "ReceiptReceived({} from={}, count={})",
                receipt_type.as_str(),
                mask_phone(sender),
                timestamps.len(),
            ),
            Self::SendTimestamp { rpc_id, server_ts } => {
                format!("SendTimestamp(rpc={rpc_id}, ts={server_ts})",)
            }
            Self::SendFailed { rpc_id } => format!("SendFailed(rpc={rpc_id})"),
            Self::TypingIndicator {
                sender, is_typing, ..
            } => format!(
                "TypingIndicator(from={}, typing={is_typing})",
                mask_phone(sender),
            ),
            Self::ReactionReceived {
                conv_id,
                emoji,
                sender,
                target_timestamp,
                is_remove,
                ..
            } => format!(
                "ReactionReceived(conv={}, from={}, emoji={emoji}, target_ts={target_timestamp}, remove={is_remove})",
                mask_phone(conv_id),
                mask_phone(sender),
            ),
            Self::EditReceived {
                conv_id,
                target_timestamp,
                new_body,
                ..
            } => format!(
                "EditReceived(conv={}, target_ts={target_timestamp}, body={})",
                mask_phone(conv_id),
                mask_body(new_body),
            ),
            Self::RemoteDeleteReceived {
                conv_id,
                target_timestamp,
                ..
            } => format!(
                "RemoteDeleteReceived(conv={}, target_ts={target_timestamp})",
                mask_phone(conv_id),
            ),
            Self::PinReceived {
                conv_id,
                target_timestamp,
                ..
            } => format!(
                "PinReceived(conv={}, target_ts={target_timestamp})",
                mask_phone(conv_id),
            ),
            Self::UnpinReceived {
                conv_id,
                target_timestamp,
                ..
            } => format!(
                "UnpinReceived(conv={}, target_ts={target_timestamp})",
                mask_phone(conv_id),
            ),
            Self::PollCreated {
                conv_id, timestamp, ..
            } => format!("PollCreated(conv={}, ts={timestamp})", mask_phone(conv_id),),
            Self::PollVoteReceived {
                conv_id,
                target_timestamp,
                voter,
                ..
            } => format!(
                "PollVoteReceived(conv={}, target_ts={target_timestamp}, voter={})",
                mask_phone(conv_id),
                mask_phone(voter),
            ),
            Self::PollTerminated {
                conv_id,
                target_timestamp,
            } => format!(
                "PollTerminated(conv={}, target_ts={target_timestamp})",
                mask_phone(conv_id),
            ),
            Self::SystemMessage { conv_id, body, .. } => format!(
                "SystemMessage(conv={}, body={})",
                mask_phone(conv_id),
                mask_body(body),
            ),
            Self::ExpirationTimerChanged {
                conv_id, seconds, ..
            } => format!(
                "ExpirationTimerChanged(conv={}, seconds={seconds})",
                mask_phone(conv_id),
            ),
            Self::ReadSyncReceived { read_messages } => {
                format!("ReadSyncReceived(count={})", read_messages.len(),)
            }
            Self::ContactList(contacts) => format!("ContactList(count={})", contacts.len()),
            Self::GroupList(groups) => format!("GroupList(count={})", groups.len()),
            Self::IdentityList(ids) => format!("IdentityList(count={})", ids.len()),
            Self::UserStatusList(statuses) => {
                format!("UserStatusList(count={})", statuses.len())
            }
            Self::Error(e) => format!("Error({e})"),
            Self::Disconnected => "Disconnected".to_string(),
        }
    }
}

/// A message from Signal
#[derive(Debug, Clone)]
pub struct SignalMessage {
    pub source: String,
    pub source_name: Option<String>,
    pub source_uuid: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub body: Option<String>,
    pub attachments: Vec<Attachment>,
    pub group_id: Option<String>,
    pub group_name: Option<String>,
    pub is_outgoing: bool,
    /// For outgoing 1:1 messages (sync), the recipient number
    pub destination: Option<String>,
    /// Body range mentions from signal-cli (for resolving U+FFFC placeholders)
    pub mentions: Vec<Mention>,
    /// Text style ranges from signal-cli (bold, italic, etc.)
    pub text_styles: Vec<TextStyle>,
    /// Quoted reply context: (timestamp_ms, author_phone, body)
    pub quote: Option<(i64, String, String)>,
    /// Disappearing message timer (seconds, 0 = no expiration)
    pub expires_in_seconds: i64,
    /// Link previews attached to this message
    pub previews: Vec<LinkPreview>,
}

impl Default for SignalMessage {
    /// Empty-but-valid message. Used by tests via struct-update syntax:
    /// `SignalMessage { body: Some("hi".into()), source: "+1".into(), ..Default::default() }`.
    /// Production parsers fill every field explicitly via `parse_common_message_fields`.
    fn default() -> Self {
        Self {
            source: String::new(),
            source_name: None,
            source_uuid: None,
            timestamp: DateTime::<Utc>::from_timestamp(0, 0).unwrap_or_default(),
            body: None,
            attachments: Vec::new(),
            group_id: None,
            group_name: None,
            is_outgoing: false,
            destination: None,
            mentions: Vec::new(),
            text_styles: Vec::new(),
            quote: None,
            expires_in_seconds: 0,
            previews: Vec::new(),
        }
    }
}

/// Link preview metadata attached to a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkPreview {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_path: Option<String>,
}

/// An attachment on a message
#[derive(Debug, Clone)]
pub struct Attachment {
    #[allow(dead_code)]
    pub id: String,
    pub content_type: String,
    pub filename: Option<String>,
    pub local_path: Option<String>,
}

/// JSON-RPC request to signal-cli
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC response from signal-cli
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<String>,
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
    pub method: Option<String>,
    pub params: Option<serde_json::Value>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

/// A body range mention from signal-cli's bodyRanges array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mention {
    pub start: usize,  // UTF-16 offset in body
    pub length: usize, // Always 1 (U+FFFC)
    pub uuid: String,  // ACI UUID of mentioned user
}

/// A text style range from signal-cli's textStyles/bodyRanges array.
#[derive(Debug, Clone)]
pub struct TextStyle {
    pub start: usize,  // UTF-16 offset in body
    pub length: usize, // UTF-16 length
    pub style: StyleType,
}

/// Type of text styling applied to a range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleType {
    Bold,
    Italic,
    Strikethrough,
    Monospace,
    Spoiler,
}

impl StyleType {
    /// The style name signal-cli uses in `textStyle` range strings
    /// ("start:length:STYLE"), the inverse of `parse_text_styles`.
    pub fn wire_name(self) -> &'static str {
        match self {
            StyleType::Bold => "BOLD",
            StyleType::Italic => "ITALIC",
            StyleType::Strikethrough => "STRIKETHROUGH",
            StyleType::Monospace => "MONOSPACE",
            StyleType::Spoiler => "SPOILER",
        }
    }
}

/// Contact info from signal-cli
#[derive(Debug, Clone)]
pub struct Contact {
    /// E.164 phone number. `None` for username-only contacts
    /// (phone-number-privacy users) — those carry a uuid instead (#612).
    pub number: Option<String>,
    pub name: Option<String>,
    pub uuid: Option<String>,
    /// Signal username with discriminator (e.g. `alice.42`), without `@` (#612).
    pub username: Option<String>,
}

impl Contact {
    /// Stable conversation/map key for this contact: phone number when
    /// present, else the ACI uuid. `None` only for malformed entries.
    pub fn key(&self) -> Option<&str> {
        self.number.as_deref().or(self.uuid.as_deref())
    }
}

/// One entry of a getUserStatus response (#612). `recipient` echoes the
/// queried identifier (for usernames, without the `u:` prefix); `uuid` is
/// present only when the account is registered.
#[derive(Debug, Clone)]
pub struct UserStatus {
    pub recipient: String,
    pub username: Option<String>,
    pub uuid: Option<String>,
    pub registered: bool,
}

/// Group info from signal-cli
#[derive(Debug, Clone)]
pub struct Group {
    pub id: String,
    pub name: String,
    /// Phone numbers of group members
    pub members: Vec<String>,
    /// (phone, uuid) pairs for members where UUID is known
    pub member_uuids: Vec<(String, String)>,
}
