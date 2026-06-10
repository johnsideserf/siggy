//! Poll parsers: create, vote, terminate. Each takes the envelope plus the
//! poll-specific subobject and produces the corresponding SignalEvent.

use crate::signal::types::*;

use super::envelope::envelope_source;

pub(super) fn parse_poll_create(
    envelope: &serde_json::Value,
    data: &serde_json::Value,
    poll_create: &serde_json::Value,
    destination: Option<String>,
) -> Option<SignalEvent> {
    let question = poll_create
        .get("question")
        .and_then(|v| v.as_str())?
        .to_string();
    let allow_multiple = poll_create
        .get("allowMultiple")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let options: Vec<crate::signal::types::PollOption> = poll_create
        .get("options")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .enumerate()
                .filter_map(|(i, opt)| {
                    let text = opt.get("optionText").and_then(|v| v.as_str())?.to_string();
                    let id = opt.get("id").and_then(|v| v.as_i64()).unwrap_or(i as i64);
                    Some(crate::signal::types::PollOption { id, text })
                })
                .collect()
        })
        .unwrap_or_default();

    let group_id = data
        .get("groupInfo")
        .and_then(|g| g.get("groupId"))
        .and_then(|v| v.as_str());
    // Conversation fallback chain: group_id -> destination -> sender. For a
    // sync envelope the source is this account, so without the destination
    // a 1:1 poll event would be filed under your own number (#485).
    let sender = envelope_source(envelope);
    let conv_id = group_id
        .map(|g| g.to_string())
        .or(destination)
        .unwrap_or_else(|| sender.clone());
    let timestamp = data.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);

    let poll_data = crate::signal::types::PollData {
        question,
        options,
        allow_multiple,
        closed: false,
    };

    // Also emit the message itself (with body = question) so it appears in chat
    Some(SignalEvent::PollCreated {
        conv_id,
        timestamp,
        poll_data,
    })
}

pub(super) fn parse_poll_vote(
    envelope: &serde_json::Value,
    data: &serde_json::Value,
    poll_vote: &serde_json::Value,
    destination: Option<String>,
) -> Option<SignalEvent> {
    let target_timestamp = poll_vote
        .get("targetSentTimestamp")
        .and_then(|v| v.as_i64())?;
    let voter = poll_vote
        .get("authorNumber")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| envelope_source(envelope));
    let voter_name = envelope
        .get("sourceName")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let option_indexes: Vec<i64> = poll_vote
        .get("optionIndexes")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
        .unwrap_or_default();
    let vote_count = poll_vote
        .get("voteCount")
        .and_then(|v| v.as_i64())
        .unwrap_or(1);

    let group_id = data
        .get("groupInfo")
        .and_then(|g| g.get("groupId"))
        .and_then(|v| v.as_str());
    // Conversation fallback chain: group_id -> destination -> sender. For a
    // sync envelope the source is this account, so without the destination
    // a 1:1 poll event would be filed under your own number (#485).
    let sender = envelope_source(envelope);
    let conv_id = group_id
        .map(|g| g.to_string())
        .or(destination)
        .unwrap_or_else(|| sender.clone());

    Some(SignalEvent::PollVoteReceived {
        conv_id,
        target_timestamp,
        voter,
        voter_name,
        option_indexes,
        vote_count,
    })
}

pub(super) fn parse_poll_terminate(
    envelope: &serde_json::Value,
    data: &serde_json::Value,
    poll_terminate: &serde_json::Value,
    destination: Option<String>,
) -> Option<SignalEvent> {
    let target_timestamp = poll_terminate
        .get("targetSentTimestamp")
        .and_then(|v| v.as_i64())?;
    let group_id = data
        .get("groupInfo")
        .and_then(|g| g.get("groupId"))
        .and_then(|v| v.as_str());
    // Conversation fallback chain: group_id -> destination -> sender. For a
    // sync envelope the source is this account, so without the destination
    // a 1:1 poll event would be filed under your own number (#485).
    let sender = envelope_source(envelope);
    let conv_id = group_id
        .map(|g| g.to_string())
        .or(destination)
        .unwrap_or_else(|| sender.clone());

    Some(SignalEvent::PollTerminated {
        conv_id,
        target_timestamp,
    })
}
