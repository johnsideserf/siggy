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
pub enum SignalEvent {
    MessageReceived(SignalMessage),
    ReceiptReceived {
        sender: String,
        receipt_type: String,
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
        #[allow(dead_code)]
        sender: String,
        #[allow(dead_code)]
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
    Error(String),
}

/// A message from Signal
#[derive(Debug, Clone)]
pub struct SignalMessage {
    pub source: String,
    pub source_name: Option<String>,
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
#[derive(Debug, Clone)]
pub struct Mention {
    pub start: usize,  // UTF-16 offset in body
    pub length: usize,  // Always 1 (U+FFFC)
    pub uuid: String,   // ACI UUID of mentioned user
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

/// Contact info from signal-cli
#[derive(Debug, Clone)]
pub struct Contact {
    pub number: String,
    pub name: Option<String>,
    pub uuid: Option<String>,
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
