//! U2 characterization fixtures: golden signal-cli JSON-RPC frames.
//!
//! Each test embeds one complete JSON-RPC frame as a raw string literal,
//! exactly as signal-cli emits it on stdout (one frame per line in
//! production), deserializes it with `serde_json::from_str::<JsonRpcResponse>`,
//! and pushes it through the real entry points: [`parse_signal_event`] for
//! method notifications and [`parse_rpc_result`] for correlated RPC results
//! (the correlation map in `signal::client` supplies the method name, which
//! the helpers here mirror). Assertions lock the CURRENT signal-cli
//! backend's observable behavior field by field.
//!
//! These tests are the regression gate for the #640 backend-boundary
//! refactor (plan: docs/superpowers/plans/2026-07-07-native-backend-presage-plan.md,
//! unit U2). They MAY NOT be weakened, loosened, or deleted to make a
//! refactor pass: if one fails, the refactor changed observable behavior
//! and the refactor is what must change.
//!
//! Fallback wire shapes (profileName -> contactName -> name priority,
//! bodyRanges mentions, bare-string group members, the older receipt "type"
//! string field) are already covered by the tests in `super::tests`; the
//! goldens here pin the primary modern signal-cli 0.13/0.14 shape.

use super::*;
use crate::signal::types::*;

const ALICE_NUMBER: &str = "+15551234567";
const ALICE_UUID: &str = "abcdef12-3456-7890-abcd-ef1234567890";
const BOB_NUMBER: &str = "+15559876543";
const BOB_UUID: &str = "0f0f0f0f-1111-2222-3333-444444444444";
const ACCOUNT: &str = "+15550000000";
/// Base64 group id as signal-cli renders it, trailing "=" padding included.
const GROUP_ID: &str = "dGVzdGdyb3VwaWQ=";

/// Deserialize a raw stdout frame and route it as a notification, the way
/// `signal::client::handle_stdout_line` does for frames without a pending id.
fn parse_notification(frame: &str) -> SignalEvent {
    let resp: JsonRpcResponse = serde_json::from_str(frame).expect("valid JSON-RPC frame");
    assert_eq!(resp.jsonrpc, "2.0");
    assert_eq!(resp.method.as_deref(), Some("receive"));
    parse_signal_event(&resp, std::path::Path::new("/tmp")).expect("frame produces an event")
}

/// Deserialize a raw stdout frame and route it as a correlated RPC result.
/// The method name comes from the `pending_requests` correlation map in
/// production; tests supply it directly, mirroring `handle_stdout_line`.
fn parse_correlated(frame: &str, method: &str) -> SignalEvent {
    let resp: JsonRpcResponse = serde_json::from_str(frame).expect("valid JSON-RPC frame");
    assert_eq!(resp.jsonrpc, "2.0");
    assert!(
        resp.method.is_none(),
        "correlated RPC results carry no method field"
    );
    let result = resp.result.as_ref().expect("result field present");
    parse_rpc_result(method, result, resp.id.as_deref()).expect("result produces an event")
}

// --- Notifications (method: "receive") ---

// Pins: incoming 1:1 dataMessage with body, quote, attachment, and
// expiresInSeconds maps onto SignalMessage field by field.
#[test]
fn golden_incoming_1to1_data_message_full() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551234567","sourceNumber":"+15551234567","sourceUuid":"abcdef12-3456-7890-abcd-ef1234567890","sourceName":"Alice","sourceDevice":2,"timestamp":1700000000000,"serverReceivedTimestamp":1700000000050,"serverDeliveredTimestamp":1700000000100,"dataMessage":{"timestamp":1700000000000,"message":"check this out","expiresInSeconds":3600,"viewOnce":false,"quote":{"id":1699999990000,"author":"+15559876543","authorNumber":"+15559876543","authorUuid":"0f0f0f0f-1111-2222-3333-444444444444","text":"original text"},"attachments":[{"contentType":"image/jpeg","filename":"photo.jpg","id":"4374172498791931559","size":42842,"width":1280,"height":960,"uploadTimestamp":1699999999500}]}},"account":"+15550000000","subscription":0}}"#;
    match parse_notification(frame) {
        SignalEvent::MessageReceived(msg) => {
            assert_eq!(msg.source, ALICE_NUMBER);
            assert_eq!(msg.source_name.as_deref(), Some("Alice"));
            assert_eq!(msg.source_uuid.as_deref(), Some(ALICE_UUID));
            assert_eq!(msg.timestamp.timestamp_millis(), 1700000000000);
            assert_eq!(msg.body.as_deref(), Some("check this out"));
            assert_eq!(msg.expires_in_seconds, 3600);
            assert_eq!(
                msg.quote,
                Some((
                    1699999990000,
                    BOB_NUMBER.to_string(),
                    "original text".to_string()
                ))
            );
            assert_eq!(msg.attachments.len(), 1);
            assert_eq!(msg.attachments[0].id, "4374172498791931559");
            assert_eq!(msg.attachments[0].content_type, "image/jpeg");
            assert_eq!(msg.attachments[0].filename.as_deref(), Some("photo.jpg"));
            // local_path depends on files present on the host; not asserted.
            assert!(!msg.is_outgoing);
            assert_eq!(msg.destination, None);
            assert_eq!(msg.group_id, None);
        }
        other => panic!("Expected MessageReceived, got {other:?}"),
    }
}

// Pins: incoming group dataMessage keys on the base64 groupId and carries
// the group name from groupInfo.
#[test]
fn golden_incoming_group_data_message() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551234567","sourceNumber":"+15551234567","sourceUuid":"abcdef12-3456-7890-abcd-ef1234567890","sourceName":"Alice","sourceDevice":2,"timestamp":1700000010000,"dataMessage":{"timestamp":1700000010000,"message":"hello group","expiresInSeconds":0,"viewOnce":false,"groupInfo":{"groupId":"dGVzdGdyb3VwaWQ=","groupName":"Test Group","revision":7,"type":"DELIVER"}}},"account":"+15550000000","subscription":0}}"#;
    match parse_notification(frame) {
        SignalEvent::MessageReceived(msg) => {
            assert_eq!(msg.group_id.as_deref(), Some(GROUP_ID));
            assert_eq!(msg.group_name.as_deref(), Some("Test Group"));
            assert_eq!(msg.source, ALICE_NUMBER);
            assert_eq!(msg.body.as_deref(), Some("hello group"));
            assert!(!msg.is_outgoing);
        }
        other => panic!("Expected MessageReceived, got {other:?}"),
    }
}

// Pins: 1:1 sent-sync (syncMessage.sentMessage) becomes an outgoing
// MessageReceived routed to destinationNumber.
#[test]
fn golden_sent_sync_1to1() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15550000000","sourceNumber":"+15550000000","sourceUuid":"99999999-8888-7777-6666-555555555555","sourceDevice":1,"timestamp":1700000020000,"syncMessage":{"sentMessage":{"destination":"+15559876543","destinationNumber":"+15559876543","destinationUuid":"0f0f0f0f-1111-2222-3333-444444444444","timestamp":1700000020000,"message":"outbound hello","expiresInSeconds":0,"viewOnce":false}}},"account":"+15550000000","subscription":0}}"#;
    match parse_notification(frame) {
        SignalEvent::MessageReceived(msg) => {
            assert!(msg.is_outgoing);
            assert_eq!(msg.destination.as_deref(), Some(BOB_NUMBER));
            assert_eq!(msg.source, ACCOUNT);
            assert_eq!(msg.source_name, None);
            assert_eq!(msg.body.as_deref(), Some("outbound hello"));
            assert_eq!(msg.timestamp.timestamp_millis(), 1700000020000);
            assert_eq!(msg.group_id, None);
        }
        other => panic!("Expected MessageReceived, got {other:?}"),
    }
}

// Pins: group sent-sync carries the base64 group id and no 1:1 destination.
#[test]
fn golden_sent_sync_group() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15550000000","sourceNumber":"+15550000000","sourceUuid":"99999999-8888-7777-6666-555555555555","sourceDevice":1,"timestamp":1700000021000,"syncMessage":{"sentMessage":{"timestamp":1700000021000,"message":"outbound to group","expiresInSeconds":0,"viewOnce":false,"groupInfo":{"groupId":"dGVzdGdyb3VwaWQ=","type":"DELIVER"}}}},"account":"+15550000000","subscription":0}}"#;
    match parse_notification(frame) {
        SignalEvent::MessageReceived(msg) => {
            assert!(msg.is_outgoing);
            assert_eq!(msg.group_id.as_deref(), Some(GROUP_ID));
            assert_eq!(msg.destination, None);
            assert_eq!(msg.source, ACCOUNT);
            assert_eq!(msg.body.as_deref(), Some("outbound to group"));
        }
        other => panic!("Expected MessageReceived, got {other:?}"),
    }
}

// Pins: receipts are boolean fields (isDelivery / isRead / isViewed), NOT a
// "type" string; a delivery receipt maps to ReceiptKind::Delivery.
#[test]
fn golden_receipt_delivery_booleans() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15559876543","sourceNumber":"+15559876543","sourceUuid":"0f0f0f0f-1111-2222-3333-444444444444","sourceDevice":3,"timestamp":1700000030000,"receiptMessage":{"when":1700000030000,"isDelivery":true,"isRead":false,"isViewed":false,"timestamps":[1700000020000]}},"account":"+15550000000","subscription":0}}"#;
    match parse_notification(frame) {
        SignalEvent::ReceiptReceived {
            sender,
            receipt_type,
            timestamps,
        } => {
            assert_eq!(sender, BOB_NUMBER);
            assert_eq!(receipt_type, ReceiptKind::Delivery);
            assert_eq!(timestamps, vec![1700000020000]);
        }
        other => panic!("Expected ReceiptReceived, got {other:?}"),
    }
}

// Pins: isRead=true wins over isDelivery=false and batches multiple
// message timestamps in one receipt frame.
#[test]
fn golden_receipt_read_booleans() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15559876543","sourceNumber":"+15559876543","sourceUuid":"0f0f0f0f-1111-2222-3333-444444444444","sourceDevice":3,"timestamp":1700000031000,"receiptMessage":{"when":1700000031000,"isDelivery":false,"isRead":true,"isViewed":false,"timestamps":[1700000020000,1700000021000]}},"account":"+15550000000","subscription":0}}"#;
    match parse_notification(frame) {
        SignalEvent::ReceiptReceived {
            sender,
            receipt_type,
            timestamps,
        } => {
            assert_eq!(sender, BOB_NUMBER);
            assert_eq!(receipt_type, ReceiptKind::Read);
            assert_eq!(timestamps, vec![1700000020000, 1700000021000]);
        }
        other => panic!("Expected ReceiptReceived, got {other:?}"),
    }
}

// Pins: 1:1 typing STARTED frame maps to is_typing=true with no group key.
#[test]
fn golden_typing_started_1to1() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551234567","sourceNumber":"+15551234567","sourceUuid":"abcdef12-3456-7890-abcd-ef1234567890","sourceName":"Alice","sourceDevice":2,"timestamp":1700000040000,"typingMessage":{"action":"STARTED","timestamp":1700000040000}},"account":"+15550000000","subscription":0}}"#;
    match parse_notification(frame) {
        SignalEvent::TypingIndicator {
            sender,
            sender_name,
            is_typing,
            group_id,
        } => {
            assert_eq!(sender, ALICE_NUMBER);
            assert_eq!(sender_name.as_deref(), Some("Alice"));
            assert!(is_typing);
            assert_eq!(group_id, None);
        }
        other => panic!("Expected TypingIndicator, got {other:?}"),
    }
}

// Pins: group typing STOPPED frame maps to is_typing=false keyed by the
// base64 group id inside typingMessage.
#[test]
fn golden_typing_stopped_group() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551234567","sourceNumber":"+15551234567","sourceUuid":"abcdef12-3456-7890-abcd-ef1234567890","sourceDevice":2,"timestamp":1700000041000,"typingMessage":{"action":"STOPPED","timestamp":1700000041000,"groupId":"dGVzdGdyb3VwaWQ="}},"account":"+15550000000","subscription":0}}"#;
    match parse_notification(frame) {
        SignalEvent::TypingIndicator {
            sender,
            is_typing,
            group_id,
            ..
        } => {
            assert_eq!(sender, ALICE_NUMBER);
            assert!(!is_typing);
            assert_eq!(group_id.as_deref(), Some(GROUP_ID));
        }
        other => panic!("Expected TypingIndicator, got {other:?}"),
    }
}

// Pins: incoming reaction (dataMessage.reaction) with targetAuthor and
// targetSentTimestamp; 1:1 conv keys by the reacting sender.
#[test]
fn golden_reaction_incoming() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551234567","sourceNumber":"+15551234567","sourceUuid":"abcdef12-3456-7890-abcd-ef1234567890","sourceName":"Alice","sourceDevice":2,"timestamp":1700000050000,"dataMessage":{"timestamp":1700000050000,"expiresInSeconds":0,"viewOnce":false,"reaction":{"emoji":"👍","targetAuthor":"+15550000000","targetAuthorNumber":"+15550000000","targetAuthorUuid":"99999999-8888-7777-6666-555555555555","targetSentTimestamp":1700000020000,"isRemove":false}}},"account":"+15550000000","subscription":0}}"#;
    match parse_notification(frame) {
        SignalEvent::ReactionReceived {
            conv_id,
            emoji,
            sender,
            sender_name,
            target_author,
            target_timestamp,
            is_remove,
        } => {
            assert_eq!(conv_id, ALICE_NUMBER);
            assert_eq!(emoji, "👍");
            assert_eq!(sender, ALICE_NUMBER);
            assert_eq!(sender_name.as_deref(), Some("Alice"));
            assert_eq!(target_author, ACCOUNT);
            assert_eq!(target_timestamp, 1700000020000);
            assert!(!is_remove);
        }
        other => panic!("Expected ReactionReceived, got {other:?}"),
    }
}

// Pins: reaction sent from another of our devices arrives as
// syncMessage.sentMessage.reaction and routes to the destination conv.
#[test]
fn golden_reaction_sent_sync() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15550000000","sourceNumber":"+15550000000","sourceUuid":"99999999-8888-7777-6666-555555555555","sourceDevice":1,"timestamp":1700000051000,"syncMessage":{"sentMessage":{"destination":"+15559876543","destinationNumber":"+15559876543","destinationUuid":"0f0f0f0f-1111-2222-3333-444444444444","timestamp":1700000051000,"expiresInSeconds":0,"viewOnce":false,"reaction":{"emoji":"❤️","targetAuthor":"+15559876543","targetSentTimestamp":1700000010000,"isRemove":false}}}},"account":"+15550000000","subscription":0}}"#;
    match parse_notification(frame) {
        SignalEvent::ReactionReceived {
            conv_id,
            emoji,
            sender,
            sender_name,
            target_author,
            target_timestamp,
            is_remove,
        } => {
            assert_eq!(conv_id, BOB_NUMBER);
            assert_eq!(emoji, "❤️");
            assert_eq!(sender, ACCOUNT);
            assert_eq!(sender_name, None);
            assert_eq!(target_author, BOB_NUMBER);
            assert_eq!(target_timestamp, 1700000010000);
            assert!(!is_remove);
        }
        other => panic!("Expected ReactionReceived, got {other:?}"),
    }
}

// Pins: incoming edit is a top-level envelope editMessage whose
// targetSentTimestamp names the original and dataMessage carries the new body.
#[test]
fn golden_edit_incoming() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551234567","sourceNumber":"+15551234567","sourceUuid":"abcdef12-3456-7890-abcd-ef1234567890","sourceName":"Alice","sourceDevice":2,"timestamp":1700000060000,"editMessage":{"targetSentTimestamp":1700000010000,"dataMessage":{"timestamp":1700000060000,"message":"fixed typo","expiresInSeconds":0,"viewOnce":false}}},"account":"+15550000000","subscription":0}}"#;
    match parse_notification(frame) {
        SignalEvent::EditReceived {
            conv_id,
            sender,
            sender_name,
            target_timestamp,
            new_body,
            new_timestamp,
            is_outgoing,
        } => {
            assert_eq!(conv_id, ALICE_NUMBER);
            assert_eq!(sender, ALICE_NUMBER);
            assert_eq!(sender_name.as_deref(), Some("Alice"));
            assert_eq!(target_timestamp, 1700000010000);
            assert_eq!(new_body, "fixed typo");
            assert_eq!(new_timestamp, 1700000060000);
            assert!(!is_outgoing);
        }
        other => panic!("Expected EditReceived, got {other:?}"),
    }
}

// Pins: editing your own message from another device syncs as
// sentMessage.editMessage and routes to the destination conversation.
#[test]
fn golden_edit_sent_sync() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15550000000","sourceNumber":"+15550000000","sourceUuid":"99999999-8888-7777-6666-555555555555","sourceDevice":1,"timestamp":1700000061000,"syncMessage":{"sentMessage":{"destination":"+15559876543","destinationNumber":"+15559876543","destinationUuid":"0f0f0f0f-1111-2222-3333-444444444444","timestamp":1700000061000,"expiresInSeconds":0,"viewOnce":false,"editMessage":{"targetSentTimestamp":1700000020000,"dataMessage":{"timestamp":1700000061000,"message":"edited from phone","expiresInSeconds":0,"viewOnce":false}}}}},"account":"+15550000000","subscription":0}}"#;
    match parse_notification(frame) {
        SignalEvent::EditReceived {
            conv_id,
            sender,
            target_timestamp,
            new_body,
            is_outgoing,
            ..
        } => {
            assert_eq!(conv_id, BOB_NUMBER);
            assert_eq!(sender, ACCOUNT);
            assert_eq!(target_timestamp, 1700000020000);
            assert_eq!(new_body, "edited from phone");
            assert!(is_outgoing);
        }
        other => panic!("Expected EditReceived, got {other:?}"),
    }
}

// Pins: remote delete arrives as dataMessage.remoteDelete.timestamp naming
// the tombstoned message.
#[test]
fn golden_remote_delete_incoming() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551234567","sourceNumber":"+15551234567","sourceUuid":"abcdef12-3456-7890-abcd-ef1234567890","sourceName":"Alice","sourceDevice":2,"timestamp":1700000070000,"dataMessage":{"timestamp":1700000070000,"expiresInSeconds":0,"viewOnce":false,"remoteDelete":{"timestamp":1700000010000}}},"account":"+15550000000","subscription":0}}"#;
    match parse_notification(frame) {
        SignalEvent::RemoteDeleteReceived {
            conv_id,
            sender,
            target_timestamp,
        } => {
            assert_eq!(conv_id, ALICE_NUMBER);
            assert_eq!(sender, ALICE_NUMBER);
            assert_eq!(target_timestamp, 1700000010000);
        }
        other => panic!("Expected RemoteDeleteReceived, got {other:?}"),
    }
}

// Pins: a bodyless groupInfo type=UPDATE frame becomes the "Group updated"
// system message keyed by the group id.
#[test]
fn golden_group_update_system_message() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551234567","sourceNumber":"+15551234567","sourceUuid":"abcdef12-3456-7890-abcd-ef1234567890","sourceDevice":2,"timestamp":1700000080000,"dataMessage":{"timestamp":1700000080000,"expiresInSeconds":0,"viewOnce":false,"groupInfo":{"groupId":"dGVzdGdyb3VwaWQ=","type":"UPDATE"}}},"account":"+15550000000","subscription":0}}"#;
    match parse_notification(frame) {
        SignalEvent::SystemMessage {
            conv_id,
            body,
            timestamp_ms,
            ..
        } => {
            assert_eq!(conv_id, GROUP_ID);
            assert_eq!(body, "Group updated");
            assert_eq!(timestamp_ms, 1700000080000);
        }
        other => panic!("Expected SystemMessage, got {other:?}"),
    }
}

// Pins: syncMessage.readMessages maps to ReadSyncReceived as
// (sender, timestamp) pairs read from the "sender" field.
#[test]
fn golden_sync_read_messages() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15550000000","sourceNumber":"+15550000000","sourceUuid":"99999999-8888-7777-6666-555555555555","sourceDevice":1,"timestamp":1700000090000,"syncMessage":{"readMessages":[{"sender":"+15551234567","senderNumber":"+15551234567","senderUuid":"abcdef12-3456-7890-abcd-ef1234567890","timestamp":1700000010000},{"sender":"+15559876543","senderNumber":"+15559876543","senderUuid":"0f0f0f0f-1111-2222-3333-444444444444","timestamp":1700000020000}]}},"account":"+15550000000","subscription":0}}"#;
    match parse_notification(frame) {
        SignalEvent::ReadSyncReceived { read_messages } => {
            assert_eq!(
                read_messages,
                vec![
                    (ALICE_NUMBER.to_string(), 1700000010000),
                    (BOB_NUMBER.to_string(), 1700000020000),
                ]
            );
        }
        other => panic!("Expected ReadSyncReceived, got {other:?}"),
    }
}

// --- Correlated RPC results (matched by id via pending_requests) ---

// Pins: listContacts result including uuid and username fields; a
// null-number (username-only) contact is kept with its uuid key.
#[test]
fn golden_rpc_list_contacts() {
    let frame = r#"{"jsonrpc":"2.0","result":[{"number":"+15551234567","uuid":"abcdef12-3456-7890-abcd-ef1234567890","username":"alice.42","name":"Alice Cell","profileName":"Alice","givenName":"Alice","familyName":null,"isBlocked":false,"messageExpirationTime":0},{"number":null,"uuid":"0f0f0f0f-1111-2222-3333-444444444444","username":"carol.99","profileName":"Carol","isBlocked":false,"messageExpirationTime":0}],"id":"c0ffee00-0000-4000-8000-000000000010"}"#;
    match parse_correlated(frame, "listContacts") {
        SignalEvent::ContactList(contacts) => {
            assert_eq!(contacts.len(), 2);
            assert_eq!(contacts[0].number.as_deref(), Some(ALICE_NUMBER));
            assert_eq!(contacts[0].uuid.as_deref(), Some(ALICE_UUID));
            assert_eq!(contacts[0].username.as_deref(), Some("alice.42"));
            // profileName wins over name (priority pinned in super::tests).
            assert_eq!(contacts[0].name.as_deref(), Some("Alice"));
            assert_eq!(contacts[1].number, None);
            assert_eq!(contacts[1].uuid.as_deref(), Some(BOB_UUID));
            assert_eq!(contacts[1].username.as_deref(), Some("carol.99"));
            assert_eq!(contacts[1].name.as_deref(), Some("Carol"));
        }
        other => panic!("Expected ContactList, got {other:?}"),
    }
}

// Pins: listGroups result with member objects ({number, uuid}) populates
// members and member_uuids; the group id is the exact base64 string.
#[test]
fn golden_rpc_list_groups() {
    let frame = r#"{"jsonrpc":"2.0","result":[{"id":"dGVzdGdyb3VwaWQ=","name":"Test Group","description":"a fixture group","isMember":true,"isBlocked":false,"messageExpirationTime":0,"members":[{"number":"+15551234567","uuid":"abcdef12-3456-7890-abcd-ef1234567890"},{"number":"+15559876543","uuid":"0f0f0f0f-1111-2222-3333-444444444444"}],"pendingMembers":[],"requestingMembers":[],"admins":[{"number":"+15551234567","uuid":"abcdef12-3456-7890-abcd-ef1234567890"}],"groupInviteLink":null}],"id":"c0ffee00-0000-4000-8000-000000000011"}"#;
    match parse_correlated(frame, "listGroups") {
        SignalEvent::GroupList(groups) => {
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].id, GROUP_ID);
            assert_eq!(groups[0].name, "Test Group");
            assert_eq!(groups[0].members, vec![ALICE_NUMBER, BOB_NUMBER]);
            assert_eq!(
                groups[0].member_uuids,
                vec![
                    (ALICE_NUMBER.to_string(), ALICE_UUID.to_string()),
                    (BOB_NUMBER.to_string(), BOB_UUID.to_string()),
                ]
            );
        }
        other => panic!("Expected GroupList, got {other:?}"),
    }
}

// Pins: listIdentities result maps trustLevel strings and carries
// fingerprint / safetyNumber / addedTimestamp through.
#[test]
fn golden_rpc_list_identities() {
    let frame = r#"{"jsonrpc":"2.0","result":[{"number":"+15551234567","uuid":"abcdef12-3456-7890-abcd-ef1234567890","fingerprint":"05ab12cd34ef","safetyNumber":"123456789012345678901234567890123456789012345678901234567890","scannableSafetyNumber":"CjQIAg==","trustLevel":"TRUSTED_UNVERIFIED","addedTimestamp":1700000000000},{"number":"+15559876543","uuid":"0f0f0f0f-1111-2222-3333-444444444444","fingerprint":"05ef34ab12cd","safetyNumber":"098765432109876543210987654321098765432109876543210987654321","scannableSafetyNumber":"CjQIAg==","trustLevel":"UNTRUSTED","addedTimestamp":1700000001000}],"id":"c0ffee00-0000-4000-8000-000000000012"}"#;
    match parse_correlated(frame, "listIdentities") {
        SignalEvent::IdentityList(identities) => {
            assert_eq!(identities.len(), 2);
            assert_eq!(identities[0].number.as_deref(), Some(ALICE_NUMBER));
            assert_eq!(identities[0].uuid.as_deref(), Some(ALICE_UUID));
            assert_eq!(identities[0].fingerprint, "05ab12cd34ef");
            assert_eq!(
                identities[0].safety_number,
                "123456789012345678901234567890123456789012345678901234567890"
            );
            assert_eq!(identities[0].trust_level, TrustLevel::TrustedUnverified);
            assert_eq!(identities[0].added_timestamp, 1700000000000);
            assert_eq!(identities[1].trust_level, TrustLevel::Untrusted);
        }
        other => panic!("Expected IdentityList, got {other:?}"),
    }
}

// Pins: getUserStatus result echoes the queried recipient and carries
// uuid only for registered accounts.
#[test]
fn golden_rpc_get_user_status() {
    let frame = r#"{"jsonrpc":"2.0","result":[{"recipient":"carol.99","number":null,"uuid":"0f0f0f0f-1111-2222-3333-444444444444","username":"carol.99","isRegistered":true},{"recipient":"+15551230000","number":"+15551230000","isRegistered":false}],"id":"c0ffee00-0000-4000-8000-000000000013"}"#;
    match parse_correlated(frame, "getUserStatus") {
        SignalEvent::UserStatusList(statuses) => {
            assert_eq!(statuses.len(), 2);
            assert_eq!(statuses[0].recipient, "carol.99");
            assert_eq!(statuses[0].username.as_deref(), Some("carol.99"));
            assert_eq!(statuses[0].uuid.as_deref(), Some(BOB_UUID));
            assert!(statuses[0].registered);
            assert_eq!(statuses[1].recipient, "+15551230000");
            assert_eq!(statuses[1].uuid, None);
            assert!(!statuses[1].registered);
        }
        other => panic!("Expected UserStatusList, got {other:?}"),
    }
}

// Pins: the send RPC response carries result.timestamp (server-assigned ms
// epoch), correlated back to the caller by the echoed rpc id.
#[test]
fn golden_rpc_send_response_timestamp() {
    let frame = r#"{"jsonrpc":"2.0","result":{"results":[{"recipientAddress":{"uuid":"0f0f0f0f-1111-2222-3333-444444444444","number":"+15559876543"},"type":"SUCCESS"}],"timestamp":1700000000123},"id":"a1b2c3d4-0000-4000-8000-000000000001"}"#;
    match parse_correlated(frame, "send") {
        SignalEvent::SendTimestamp { token, server_ts } => {
            assert_eq!(
                token,
                SendToken::new("a1b2c3d4-0000-4000-8000-000000000001")
            );
            assert_eq!(server_ts, 1700000000123);
        }
        other => panic!("Expected SendTimestamp, got {other:?}"),
    }
}

// Pins: a send RPC error frame deserializes into JsonRpcError with code and
// message and carries no result. The routing of a tracked send error into
// SignalEvent::SendFailed lives in signal::client::handle_stdout_line and is
// locked by handle_stdout_line_send_error_yields_send_failed there.
#[test]
fn golden_rpc_send_error_shape() {
    let frame = r#"{"jsonrpc":"2.0","error":{"code":-32602,"message":"Failed to send message: Unregistered user","data":null},"id":"a1b2c3d4-0000-4000-8000-000000000002"}"#;
    let resp: JsonRpcResponse = serde_json::from_str(frame).expect("valid JSON-RPC frame");
    assert_eq!(resp.jsonrpc, "2.0");
    assert_eq!(
        resp.id.as_deref(),
        Some("a1b2c3d4-0000-4000-8000-000000000002")
    );
    assert!(resp.result.is_none());
    assert!(resp.method.is_none());
    let err = resp.error.expect("error object present");
    assert_eq!(err.code, -32602);
    assert_eq!(err.message, "Failed to send message: Unregistered user");
}

// --- Conversation-id format lock (KTD-6 boundary contract) ---
//
// These pin the KTD-6 contract from the native-backend plan
// (docs/superpowers/plans/2026-07-07-native-backend-presage-plan.md):
// conversation ids are E.164 / lowercase-hyphenated ACI uuid / base64 group
// id exactly as signal-cli renders them, and the SELECTION RULE is
// E.164-when-known-else-uuid. The native adapter must convert presage
// identities INTO these formats so siggy.db stays backend-agnostic.

// Pins the selection rule: a frame carrying BOTH sourceNumber and
// sourceUuid keys by the E.164 number.
#[test]
fn conv_id_lock_e164_wins_when_both_number_and_uuid_present() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551234567","sourceNumber":"+15551234567","sourceUuid":"abcdef12-3456-7890-abcd-ef1234567890","sourceDevice":2,"timestamp":1700000100000,"dataMessage":{"timestamp":1700000100000,"message":"hi","expiresInSeconds":0,"viewOnce":false}}}}"#;
    match parse_notification(frame) {
        SignalEvent::MessageReceived(msg) => {
            assert_eq!(msg.source, ALICE_NUMBER, "E.164 must win over uuid");
            assert_eq!(msg.source_uuid.as_deref(), Some(ALICE_UUID));
        }
        other => panic!("Expected MessageReceived, got {other:?}"),
    }
}

// Pins the selection rule: with no sourceNumber (phone-number privacy) the
// id is the ACI uuid, byte-for-byte lowercase-hyphenated.
#[test]
fn conv_id_lock_uuid_when_number_absent() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"sourceUuid":"abcdef12-3456-7890-abcd-ef1234567890","sourceName":"Privacy Fan","sourceDevice":2,"timestamp":1700000101000,"dataMessage":{"timestamp":1700000101000,"message":"hi","expiresInSeconds":0,"viewOnce":false}}}}"#;
    match parse_notification(frame) {
        SignalEvent::MessageReceived(msg) => {
            assert_eq!(msg.source, ALICE_UUID);
            assert_eq!(msg.source_uuid.as_deref(), Some(ALICE_UUID));
        }
        other => panic!("Expected MessageReceived, got {other:?}"),
    }
}

// Pins the group-id format: the base64 string signal-cli renders, with
// trailing "=" padding preserved byte-for-byte.
#[test]
fn conv_id_lock_group_base64_padding_preserved() {
    let frame = r#"{"jsonrpc":"2.0","method":"receive","params":{"envelope":{"source":"+15551234567","sourceNumber":"+15551234567","sourceUuid":"abcdef12-3456-7890-abcd-ef1234567890","sourceDevice":2,"timestamp":1700000102000,"dataMessage":{"timestamp":1700000102000,"message":"hi","expiresInSeconds":0,"viewOnce":false,"groupInfo":{"groupId":"dGVzdGdyb3VwaWQ=","type":"DELIVER"}}}}}"#;
    match parse_notification(frame) {
        SignalEvent::MessageReceived(msg) => {
            let gid = msg.group_id.as_deref().expect("group id present");
            assert_eq!(gid.as_bytes(), GROUP_ID.as_bytes());
            assert!(gid.ends_with('='), "base64 padding must be preserved");
        }
        other => panic!("Expected MessageReceived, got {other:?}"),
    }
}
