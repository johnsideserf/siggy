//! Outgoing-request value type.
//!
//! [`SendRequest`] is the message the input/overlay handlers hand back to the
//! main loop, which routes each variant through the active backend
//! (`Backend::dispatch` in `src/backend/`). It is pure data with no `App`
//! coupling, so it lives in `domain` as a leaf type that `app`, `handlers`,
//! and the main loop import from.

use std::path::PathBuf;

use crate::signal::types::{LinkPreview, StyleType};

/// A request from the UI to the main loop to send something.
// Under the native feature only the U9 stub adapter exists, which ignores
// request payloads, so field-read analysis flags most of the vocabulary.
// Every field is read again when U12 implements native dispatch; drop the
// allow then.
#[cfg_attr(feature = "native-backend", allow(dead_code))]
pub enum SendRequest {
    Message {
        recipient: String,
        body: String,
        is_group: bool,
        local_ts_ms: i64,
        mentions: Vec<(usize, String)>,
        /// UTF-16 (start, length, style) ranges for signal-cli's textStyle param.
        text_styles: Vec<(usize, usize, StyleType)>,
        attachment: Option<PathBuf>,
        /// Sender-generated link preview from /preview (#267).
        preview: Option<LinkPreview>,
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
        /// UTF-16 (start, length, style) ranges for signal-cli's textStyle param.
        text_styles: Vec<(usize, usize, StyleType)>,
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
    /// Resolve a Signal username (`name.123`, no `@`) to an account uuid via
    /// getUserStatus, for `/join @handle` on unknown handles (#612).
    ResolveUsername {
        username: String,
    },
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
