//! Top-level envelope dispatch: routes signal-cli notifications to the
//! appropriate parser based on the envelope shape (dataMessage,
//! syncMessage, typingMessage, receiptMessage, etc).

use chrono::DateTime;

use crate::signal::types::*;

use super::message::{parse_data_message, parse_edit_message, parse_sent_sync};

pub fn parse_signal_event(
    resp: &JsonRpcResponse,
    download_dir: &std::path::Path,
) -> Option<SignalEvent> {
    // signal-cli sends notifications as JSON-RPC requests with a method field
    let method = resp.method.as_deref()?;
    let params = resp.params.as_ref()?;

    match method {
        "receive" => parse_receive_event(params, download_dir),
        _ => None,
    }
}

/// Extract the canonical sender identifier from a signal-cli envelope.
/// Prefers `sourceNumber` (phone), falls back to `sourceUuid` for contacts
/// with phone-number privacy enabled, and finally to "unknown" if neither is
/// present. Returning a phone or UUID means conversations keyed off this
/// identifier route through signal-cli's recipient field correctly (it
/// accepts both formats). (#315)
pub(super) fn envelope_source(envelope: &serde_json::Value) -> String {
    envelope
        .get("sourceNumber")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| envelope.get("sourceUuid").and_then(|v| v.as_str()))
        .unwrap_or("unknown")
        .to_string()
}

/// Extract the canonical destination identifier from a sent (sync) message.
/// Prefers `destinationNumber`, falls back to `destination`, then
/// `destinationUuid`. Returns None for group sends or messages with no
/// resolvable recipient.
pub(super) fn sent_destination(sent: &serde_json::Value) -> Option<String> {
    sent.get("destinationNumber")
        .or_else(|| sent.get("destination"))
        .or_else(|| sent.get("destinationUuid"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn parse_receive_event(
    params: &serde_json::Value,
    download_dir: &std::path::Path,
) -> Option<SignalEvent> {
    // signal-cli reports exceptions for messages it can't parse (e.g. 1:1 sent sync)
    if let Some(exc) = params.get("exception") {
        let msg = exc
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        if msg.contains("SyncMessage missing destination") {
            return None; // Known signal-cli bug — silently ignore
        }
        // Safety number change → system message instead of generic error
        let exc_type = exc.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if exc_type == "UntrustedIdentityException" {
            let envelope = params.get("envelope");
            let conv_id = envelope
                .map(envelope_source)
                .filter(|s| s != "unknown")
                .or_else(|| {
                    exc.get("sender")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| "unknown".to_string());
            let timestamp_ms = envelope
                .and_then(|e| e.get("timestamp"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let timestamp = DateTime::from_timestamp_millis(timestamp_ms).unwrap_or_default();
            return Some(SignalEvent::SystemMessage {
                conv_id,
                body: "\u{26A0} Safety number changed".to_string(),
                timestamp,
                timestamp_ms,
            });
        }
        return Some(SignalEvent::Error(format!("signal-cli: {msg}")));
    }

    let envelope = params.get("envelope")?;

    if envelope.get("typingMessage").is_some() {
        return parse_typing_indicator(envelope);
    }
    if envelope.get("receiptMessage").is_some() {
        return parse_receipt_message(envelope);
    }
    // Call messages (missed calls)
    if let Some(call_msg) = envelope.get("callMessage") {
        if call_msg.get("offerMessage").is_some() {
            let call_type = call_msg
                .get("offerMessage")
                .and_then(|o| o.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("AUDIO_CALL");
            let kind = if call_type == "VIDEO_CALL" {
                "video"
            } else {
                "voice"
            };
            let conv_id = envelope_source(envelope);
            let timestamp_ms = envelope
                .get("timestamp")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let timestamp = DateTime::from_timestamp_millis(timestamp_ms).unwrap_or_default();
            return Some(SignalEvent::SystemMessage {
                conv_id,
                body: format!("Missed {kind} call"),
                timestamp,
                timestamp_ms,
            });
        }
        // Ignore ICE candidates, hangup, busy (call signaling noise)
        return None;
    }
    // Check for editMessage (top-level envelope field) before dataMessage
    if let Some(edit_msg) = envelope.get("editMessage") {
        return parse_edit_message(envelope, edit_msg, false, None);
    }

    if let Some(sync) = envelope.get("syncMessage") {
        if let Some(sent) = sync.get("sentMessage") {
            // Check for edit in sync
            if let Some(edit_msg) = sent.get("editMessage") {
                let dest = sent_destination(sent);
                return parse_edit_message(envelope, edit_msg, true, dest.as_deref());
            }
            return parse_sent_sync(envelope, sent, download_dir);
        }
        if let Some(event) = parse_read_sync(sync) {
            return Some(event);
        }
        return None;
    }

    parse_data_message(envelope, download_dir)
}

fn parse_read_sync(sync: &serde_json::Value) -> Option<SignalEvent> {
    let read_messages = sync.get("readMessages")?.as_array()?;
    if read_messages.is_empty() {
        return None;
    }
    let entries: Vec<(String, i64)> = read_messages
        .iter()
        .filter_map(|entry| {
            let sender = entry.get("sender").and_then(|v| v.as_str())?.to_string();
            let timestamp = entry.get("timestamp").and_then(|v| v.as_i64())?;
            Some((sender, timestamp))
        })
        .collect();
    if entries.is_empty() {
        return None;
    }
    Some(SignalEvent::ReadSyncReceived {
        read_messages: entries,
    })
}

fn parse_typing_indicator(envelope: &serde_json::Value) -> Option<SignalEvent> {
    let typing = envelope.get("typingMessage")?;
    let sender = envelope_source(envelope);
    let sender_name = envelope
        .get("sourceName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let is_typing = typing
        .get("action")
        .and_then(|v| v.as_str())
        .map(|a| a == "STARTED")
        .unwrap_or(false);
    let group_id = typing
        .get("groupId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Some(SignalEvent::TypingIndicator {
        sender,
        sender_name,
        is_typing,
        group_id,
    })
}

fn parse_receipt_message(envelope: &serde_json::Value) -> Option<SignalEvent> {
    let receipt = envelope.get("receiptMessage")?;
    let sender = envelope_source(envelope);
    // signal-cli uses boolean fields: isDelivery, isRead, isViewed
    let receipt_type = if receipt
        .get("isRead")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        "READ"
    } else if receipt
        .get("isViewed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        "VIEWED"
    } else if receipt
        .get("isDelivery")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        "DELIVERY"
    } else {
        // Fallback: try "type" string field (older signal-cli versions)
        receipt
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("UNKNOWN")
    }
    .to_string();
    let timestamps: Vec<i64> = receipt
        .get("timestamps")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
        .unwrap_or_default();
    Some(SignalEvent::ReceiptReceived {
        sender,
        receipt_type,
        timestamps,
    })
}
