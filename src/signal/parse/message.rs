//! Message-shaped parsers: incoming data messages, outgoing sync messages,
//! reactions (both directions), and edits. Shared body-extraction helpers
//! (`parse_common_message_fields`, `parse_data_payload_event`) live here
//! since they only run in this module's call chain.

use chrono::DateTime;

use crate::signal::types::*;

use super::envelope::{envelope_source, sent_destination};
use super::helpers::{
    format_expiration, parse_attachment, parse_link_previews, parse_mentions, parse_text_styles,
};
use super::poll::{parse_poll_create, parse_poll_terminate, parse_poll_vote};

pub(super) fn parse_data_message(
    envelope: &serde_json::Value,
    download_dir: &std::path::Path,
) -> Option<SignalEvent> {
    let data = match envelope.get("dataMessage") {
        Some(d) => d,
        None => {
            // Catch-all: envelope type we don't handle yet — surface it for diagnostics
            let keys: Vec<&str> = envelope
                .as_object()
                .map(|obj| obj.keys().map(|k| k.as_str()).collect())
                .unwrap_or_default();
            let interesting: Vec<&&str> = keys
                .iter()
                .filter(|k| {
                    !matches!(
                        **k,
                        "source"
                            | "sourceNumber"
                            | "sourceName"
                            | "sourceUuid"
                            | "sourceDevice"
                            | "timestamp"
                            | "serverReceivedTimestamp"
                            | "serverDeliveredTimestamp"
                            | "relay"
                    )
                })
                .collect();
            if !interesting.is_empty() {
                return Some(SignalEvent::Error(format!(
                    "unhandled envelope type: {}",
                    interesting
                        .iter()
                        .map(|k| **k)
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            return None;
        }
    };

    // Reactions on incoming messages have a dedicated parser (sent-sync uses parse_reaction_sync).
    if let Some(reaction) = data.get("reaction") {
        let group_id = data
            .get("groupInfo")
            .and_then(|g| g.get("groupId"))
            .and_then(|v| v.as_str());
        return parse_reaction(envelope, reaction, group_id);
    }

    if let Some(poll_create) = data.get("pollCreate") {
        return parse_poll_create(envelope, data, poll_create, None);
    }
    if let Some(poll_vote) = data.get("pollVote") {
        return parse_poll_vote(envelope, data, poll_vote, None);
    }
    if let Some(poll_terminate) = data.get("pollTerminate") {
        return parse_poll_terminate(envelope, data, poll_terminate, None);
    }

    // Shared early-return events (pin, unpin, remoteDelete, expirationUpdate, group UPDATE).
    if let Some(event) = parse_data_payload_event(envelope, data, None) {
        return Some(event);
    }

    let source = envelope_source(envelope);
    let source_name = envelope
        .get("sourceName")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let source_uuid = envelope
        .get("sourceUuid")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let common = parse_common_message_fields(data, download_dir);

    Some(SignalEvent::MessageReceived(SignalMessage {
        source,
        source_name,
        source_uuid,
        timestamp: common.timestamp,
        body: common.body,
        attachments: common.attachments,
        group_id: common.group_id,
        group_name: common.group_name,
        is_outgoing: false,
        destination: None,
        mentions: common.mentions,
        text_styles: common.text_styles,
        quote: common.quote,
        expires_in_seconds: common.expires_in_seconds,
        previews: common.previews,
    }))
}

pub(super) fn parse_sent_sync(
    envelope: &serde_json::Value,
    sent: &serde_json::Value,
    download_dir: &std::path::Path,
) -> Option<SignalEvent> {
    // Reaction sync has its own parser (incoming uses parse_reaction).
    if let Some(reaction) = sent.get("reaction") {
        return parse_reaction_sync(envelope, sent, reaction);
    }

    // Poll parsers need the destination: for a sync envelope the source is
    // this account, and without the fallback a 1:1 poll event is filed
    // under your own number (#485).
    if let Some(poll_create) = sent.get("pollCreate") {
        return parse_poll_create(envelope, sent, poll_create, sent_destination(sent));
    }
    if let Some(poll_vote) = sent.get("pollVote") {
        return parse_poll_vote(envelope, sent, poll_vote, sent_destination(sent));
    }
    if let Some(poll_terminate) = sent.get("pollTerminate") {
        return parse_poll_terminate(envelope, sent, poll_terminate, sent_destination(sent));
    }

    let destination = sent_destination(sent);

    // Shared early-return events. Pass destination so conv_id falls back through
    // group_id -> destination -> sender (vs. group_id -> sender for receive).
    if let Some(event) = parse_data_payload_event(envelope, sent, destination.clone()) {
        return Some(event);
    }

    let source = envelope_source(envelope);
    let common = parse_common_message_fields(sent, download_dir);

    Some(SignalEvent::MessageReceived(SignalMessage {
        source,
        source_name: None,
        source_uuid: None,
        timestamp: common.timestamp,
        body: common.body,
        attachments: common.attachments,
        group_id: common.group_id,
        group_name: common.group_name,
        is_outgoing: true,
        destination,
        mentions: common.mentions,
        text_styles: common.text_styles,
        quote: common.quote,
        expires_in_seconds: common.expires_in_seconds,
        previews: common.previews,
    }))
}

/// Fields extracted from the body of a `dataMessage` / `sentMessage` payload.
/// Identical between incoming and outgoing-sync; the wrapping function adds
/// variant-specific fields (source/source_name vs. destination/is_outgoing).
struct CommonMessageFields {
    body: Option<String>,
    attachments: Vec<crate::signal::types::Attachment>,
    previews: Vec<crate::signal::types::LinkPreview>,
    group_id: Option<String>,
    group_name: Option<String>,
    mentions: Vec<Mention>,
    text_styles: Vec<TextStyle>,
    quote: Option<(i64, String, String)>,
    expires_in_seconds: i64,
    timestamp: DateTime<chrono::Utc>,
}

fn parse_common_message_fields(
    data: &serde_json::Value,
    download_dir: &std::path::Path,
) -> CommonMessageFields {
    let timestamp_ms = data.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
    let timestamp = DateTime::from_timestamp_millis(timestamp_ms).unwrap_or_default();

    // Stickers render as inline images when signal-cli has the pack cached
    // locally (#610); otherwise fall back to the [Sticker: emoji] placeholder.
    let sticker = data.get("sticker");
    let sticker_emoji = sticker
        .and_then(|s| s.get("emoji"))
        .and_then(|v| v.as_str());
    let sticker_attachment = sticker.and_then(|s| {
        let pack_id = s.get("packId").and_then(|v| v.as_str())?;
        let sticker_id = s.get("stickerId").and_then(|v| v.as_i64())?;
        let local_path = super::helpers::sticker_local_path(pack_id, sticker_id)?;
        Some(crate::signal::types::Attachment {
            id: format!("sticker:{pack_id}:{sticker_id}"),
            content_type: "image/webp".to_string(),
            filename: Some(match sticker_emoji {
                Some(emoji) => format!("sticker {emoji}"),
                None => "sticker".to_string(),
            }),
            local_path: Some(local_path),
        })
    });
    let sticker_body = if sticker_attachment.is_some() {
        None
    } else {
        sticker.map(|_| match sticker_emoji {
            Some(emoji) => format!("[Sticker: {emoji}]"),
            None => "[Sticker]".to_string(),
        })
    };

    let mut body = data
        .get("message")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or(sticker_body);

    let group_id = data
        .get("groupInfo")
        .and_then(|g| g.get("groupId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let group_name = data
        .get("groupInfo")
        .and_then(|g| g.get("groupName"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut attachments: Vec<crate::signal::types::Attachment> = data
        .get("attachments")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| parse_attachment(a, download_dir))
                .collect()
        })
        .unwrap_or_default();
    // A locally cached sticker joins the attachments so the existing image
    // pipeline (halfblock / native protocols) renders it (#610). Placed
    // before the view-once check so ephemeral stickers stay suppressed.
    attachments.extend(sticker_attachment);

    let mut previews = parse_link_previews(data, download_dir);

    // View-once messages: replace content with placeholder.
    if data
        .get("viewOnce")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        body = Some("[View-once message]".to_string());
        attachments = Vec::new();
        previews = Vec::new();
    }

    let mentions = parse_mentions(data);
    let text_styles = parse_text_styles(data);

    // Strip U+FFFC mention placeholders from quote text.
    let quote = data.get("quote").and_then(|q| {
        let q_ts = q.get("id").and_then(|v| v.as_i64())?;
        let q_author = q.get("authorNumber").and_then(|v| v.as_str())?.to_string();
        let q_body = q
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .replace('\u{FFFC}', "")
            .to_string();
        Some((q_ts, q_author, q_body))
    });

    let expires_in_seconds = data
        .get("expiresInSeconds")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    CommonMessageFields {
        body,
        attachments,
        previews,
        group_id,
        group_name,
        mentions,
        text_styles,
        quote,
        expires_in_seconds,
        timestamp,
    }
}

/// Match the shared early-return event types from a `dataMessage` /
/// `sentMessage` payload: pinMessage, unpinMessage, remoteDelete,
/// isExpirationUpdate, and group UPDATE-without-body.
///
/// `destination` is the precomputed `sent_destination` for the sync path
/// (or `None` for the receive path). It is inserted into the conv_id
/// fallback chain between `group_id` and `sender`, matching the pre-refactor
/// behaviour of parse_sent_sync.
fn parse_data_payload_event(
    envelope: &serde_json::Value,
    data: &serde_json::Value,
    destination: Option<String>,
) -> Option<SignalEvent> {
    let group_id = data
        .get("groupInfo")
        .and_then(|g| g.get("groupId"))
        .and_then(|v| v.as_str());

    let resolve_conv_id = |sender: &str| -> String {
        group_id
            .map(|g| g.to_string())
            .or_else(|| destination.clone())
            .unwrap_or_else(|| sender.to_string())
    };

    if let Some(pin) = data.get("pinMessage") {
        let target_author = pin
            .get("targetAuthor")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let target_timestamp = pin
            .get("targetSentTimestamp")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let sender = envelope_source(envelope);
        let sender_name = envelope
            .get("sourceName")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        return Some(SignalEvent::PinReceived {
            conv_id: resolve_conv_id(&sender),
            sender,
            sender_name,
            target_author,
            target_timestamp,
        });
    }

    if let Some(unpin) = data.get("unpinMessage") {
        let target_author = unpin
            .get("targetAuthor")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let target_timestamp = unpin
            .get("targetSentTimestamp")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let sender = envelope_source(envelope);
        let sender_name = envelope
            .get("sourceName")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        return Some(SignalEvent::UnpinReceived {
            conv_id: resolve_conv_id(&sender),
            sender,
            sender_name,
            target_author,
            target_timestamp,
        });
    }

    if let Some(remote_delete) = data.get("remoteDelete") {
        let target_timestamp = remote_delete.get("timestamp").and_then(|v| v.as_i64())?;
        let sender = envelope_source(envelope);
        return Some(SignalEvent::RemoteDeleteReceived {
            conv_id: resolve_conv_id(&sender),
            sender,
            target_timestamp,
        });
    }

    if data
        .get("isExpirationUpdate")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let sender = envelope_source(envelope);
        let conv_id = resolve_conv_id(&sender);
        let seconds = data
            .get("expiresInSeconds")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let timestamp_ms = data.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
        let timestamp = DateTime::from_timestamp_millis(timestamp_ms).unwrap_or_default();
        return Some(SignalEvent::ExpirationTimerChanged {
            conv_id,
            seconds,
            body: format_expiration(seconds),
            timestamp,
            timestamp_ms,
        });
    }

    // Group update with no body/reaction/remoteDelete -> system message.
    // conv_id here always comes from the group_id directly (never sender or
    // destination) since we are inside the groupInfo branch.
    if let Some(group_info) = data.get("groupInfo") {
        let group_type = group_info
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if group_type == "UPDATE"
            && data.get("message").and_then(|v| v.as_str()).is_none()
            && data.get("reaction").is_none()
            && data.get("remoteDelete").is_none()
        {
            let conv_id = group_info
                .get("groupId")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let timestamp_ms = data.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
            let timestamp = DateTime::from_timestamp_millis(timestamp_ms).unwrap_or_default();
            return Some(SignalEvent::SystemMessage {
                conv_id,
                body: "Group updated".to_string(),
                timestamp,
                timestamp_ms,
            });
        }
    }

    None
}

fn parse_reaction(
    envelope: &serde_json::Value,
    reaction: &serde_json::Value,
    group_id: Option<&str>,
) -> Option<SignalEvent> {
    let emoji = reaction.get("emoji").and_then(|v| v.as_str())?.to_string();
    let target_author = reaction
        .get("targetAuthor")
        .and_then(|v| v.as_str())?
        .to_string();
    let target_timestamp = reaction
        .get("targetSentTimestamp")
        .and_then(|v| v.as_i64())?;
    let is_remove = reaction
        .get("isRemove")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let sender = envelope_source(envelope);
    let sender_name = envelope
        .get("sourceName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let conv_id = group_id
        .map(|g| g.to_string())
        .unwrap_or_else(|| sender.clone());

    Some(SignalEvent::ReactionReceived {
        conv_id,
        emoji,
        sender,
        sender_name,
        target_author,
        target_timestamp,
        is_remove,
    })
}

pub(super) fn parse_reaction_sync(
    envelope: &serde_json::Value,
    sent: &serde_json::Value,
    reaction: &serde_json::Value,
) -> Option<SignalEvent> {
    let emoji = reaction.get("emoji").and_then(|v| v.as_str())?.to_string();
    let target_author = reaction
        .get("targetAuthor")
        .and_then(|v| v.as_str())?
        .to_string();
    let target_timestamp = reaction
        .get("targetSentTimestamp")
        .and_then(|v| v.as_i64())?;
    let is_remove = reaction
        .get("isRemove")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let sender = envelope_source(envelope);

    let group_id = sent
        .get("groupInfo")
        .and_then(|g| g.get("groupId"))
        .and_then(|v| v.as_str());

    let conv_id = group_id
        .map(|g| g.to_string())
        .or_else(|| {
            sent.get("destinationNumber")
                .or_else(|| sent.get("destination"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| sender.clone());

    Some(SignalEvent::ReactionReceived {
        conv_id,
        emoji,
        sender,
        sender_name: None,
        target_author,
        target_timestamp,
        is_remove,
    })
}

pub(super) fn parse_edit_message(
    envelope: &serde_json::Value,
    edit_msg: &serde_json::Value,
    is_outgoing: bool,
    destination: Option<&str>,
) -> Option<SignalEvent> {
    let target_timestamp = edit_msg
        .get("targetSentTimestamp")
        .and_then(|v| v.as_i64())?;
    let data = edit_msg.get("dataMessage")?;
    let new_body = data.get("message").and_then(|v| v.as_str())?.to_string();
    let new_timestamp = data.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);

    let sender = envelope_source(envelope);
    let sender_name = envelope
        .get("sourceName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let group_id = data
        .get("groupInfo")
        .and_then(|g| g.get("groupId"))
        .and_then(|v| v.as_str());

    let conv_id = group_id.map(|g| g.to_string()).or_else(|| {
        if is_outgoing {
            // For outgoing sync edits, use destination (recipient) as conv_id
            destination.map(|d| d.to_string())
        } else {
            Some(sender.clone())
        }
    })?;

    Some(SignalEvent::EditReceived {
        conv_id,
        sender,
        sender_name,
        target_timestamp,
        new_body,
        new_timestamp,
        is_outgoing,
    })
}
