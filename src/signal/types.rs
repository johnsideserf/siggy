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

/// Contact info from signal-cli
#[derive(Debug, Clone)]
pub struct Contact {
    pub number: String,
    pub name: Option<String>,
}

/// Group info from signal-cli
#[derive(Debug, Clone)]
pub struct Group {
    pub id: String,
    pub name: String,
    #[allow(dead_code)] // used in tests; will be used for @mentions
    pub members: Vec<String>,
}
