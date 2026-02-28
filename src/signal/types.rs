use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Events received from signal-cli
#[derive(Debug, Clone)]
pub enum SignalEvent {
    MessageReceived(SignalMessage),
    ReceiptReceived {
        sender: String,
        timestamp: i64,
    },
    TypingIndicator {
        sender: String,
        is_typing: bool,
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
}

/// An attachment on a message
#[derive(Debug, Clone)]
pub struct Attachment {
    pub id: String,
    pub content_type: String,
    pub filename: Option<String>,
    pub local_path: Option<String>,
}

/// Commands sent to signal-cli
#[derive(Debug, Clone)]
pub enum UserCommand {
    SendMessage {
        recipient: String,
        body: String,
        is_group: bool,
    },
    ListGroups,
    ListContacts,
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
    pub jsonrpc: String,
    pub id: Option<String>,
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
    pub method: Option<String>,
    pub params: Option<serde_json::Value>,
}

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
    pub members: Vec<String>,
}
