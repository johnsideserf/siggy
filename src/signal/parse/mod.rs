//! JSON-RPC frame parsing for signal-cli notifications and responses.
//!
//! All free `parse_*` functions are split across submodules:
//! - [`envelope`] -- top-level dispatch, typing/receipt/read-sync routing
//! - [`message`] -- data messages, sent-sync, reactions, edits
//! - [`poll`] -- poll create / vote / terminate
//! - [`rpc`] -- correlated RPC response shape converters
//! - [`helpers`] -- attachments, mentions, styles, expiration formatting
//!
//! Two parsers are public to the [`super::client`] module:
//! [`parse_rpc_result`] handles correlated RPC responses, and
//! [`parse_signal_event`] handles unsolicited notifications. The rest are
//! private to the submodule tree.
//!
//! The parsers deliberately defend against signal-cli's quirky, version-
//! dependent JSON shape: many of them fall through multiple field names
//! (e.g. `profileName` -> `contactName` -> `name`) or accept both object
//! and bare-string member representations.

pub(super) mod envelope;
pub(super) mod helpers;
pub(super) mod message;
pub(super) mod poll;
pub(super) mod rpc;

pub use envelope::parse_signal_event;
pub use rpc::parse_rpc_result;

#[cfg(test)]
mod tests {
    use super::helpers::parse_link_previews;
    use super::*;
    use crate::signal::types::*;
    use rstest::rstest;
    use serde_json::json;

    fn make_resp(params: serde_json::Value) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("receive".to_string()),
            params: Some(params),
        }
    }

    // --- Test 2: listContacts parsing populates contacts ---

    #[test]
    fn parse_list_contacts_basic() {
        let result = json!([
            {"number": "+15551234567", "profileName": "Alice"},
            {"number": "+15559876543", "contactName": "Bob"}
        ]);
        let event = parse_rpc_result("listContacts", &result, None).unwrap();
        match event {
            SignalEvent::ContactList(contacts) => {
                assert_eq!(contacts.len(), 2);
                assert_eq!(contacts[0].number, "+15551234567");
                assert_eq!(contacts[0].name.as_deref(), Some("Alice"));
                assert_eq!(contacts[1].number, "+15559876543");
                assert_eq!(contacts[1].name.as_deref(), Some("Bob"));
            }
            _ => panic!("Expected ContactList"),
        }
    }

    // --- Test 4: Contact names resolve correctly (profileName > contactName > name) ---

    #[test]
    fn parse_list_contacts_name_priority() {
        let result = json!([
            {"number": "+1", "profileName": "Profile", "contactName": "Contact", "name": "Name"},
            {"number": "+2", "contactName": "Contact", "name": "Name"},
            {"number": "+3", "name": "Name"},
            {"number": "+4"}
        ]);
        let event = parse_rpc_result("listContacts", &result, None).unwrap();
        match event {
            SignalEvent::ContactList(contacts) => {
                assert_eq!(contacts.len(), 4);
                assert_eq!(contacts[0].name.as_deref(), Some("Profile"));
                assert_eq!(contacts[1].name.as_deref(), Some("Contact"));
                assert_eq!(contacts[2].name.as_deref(), Some("Name"));
                assert_eq!(contacts[3].name, None); // no name fields
            }
            _ => panic!("Expected ContactList"),
        }
    }

    #[test]
    fn parse_list_contacts_skips_no_number() {
        let result = json!([
            {"profileName": "Ghost"},
            {"number": "+1", "profileName": "Valid"}
        ]);
        let event = parse_rpc_result("listContacts", &result, None).unwrap();
        match event {
            SignalEvent::ContactList(contacts) => {
                assert_eq!(contacts.len(), 1);
                assert_eq!(contacts[0].number, "+1");
            }
            _ => panic!("Expected ContactList"),
        }
    }

    #[test]
    fn parse_list_contacts_empty_name_becomes_none() {
        let result = json!([
            {"number": "+1", "profileName": ""}
        ]);
        let event = parse_rpc_result("listContacts", &result, None).unwrap();
        match event {
            SignalEvent::ContactList(contacts) => {
                assert_eq!(contacts[0].name, None);
            }
            _ => panic!("Expected ContactList"),
        }
    }

    // --- Test 5: Groups parse with id, name, members ---

    #[test]
    fn parse_list_groups_basic() {
        // signal-cli returns members as objects with number/uuid fields
        let result = json!([
            {"id": "group1", "name": "Family", "members": [
                {"number": "+1", "uuid": "uuid-1"},
                {"number": "+2", "uuid": "uuid-2"}
            ]},
            {"id": "group2", "name": "Work"}
        ]);
        let event = parse_rpc_result("listGroups", &result, None).unwrap();
        match event {
            SignalEvent::GroupList(groups) => {
                assert_eq!(groups.len(), 2);
                assert_eq!(groups[0].id, "group1");
                assert_eq!(groups[0].name, "Family");
                assert_eq!(groups[0].members, vec!["+1", "+2"]);
                assert_eq!(
                    groups[0].member_uuids,
                    vec![
                        ("+1".to_string(), "uuid-1".to_string()),
                        ("+2".to_string(), "uuid-2".to_string()),
                    ]
                );
                assert_eq!(groups[1].id, "group2");
                assert_eq!(groups[1].name, "Work");
                assert!(groups[1].members.is_empty());
                assert!(groups[1].member_uuids.is_empty());
            }
            _ => panic!("Expected GroupList"),
        }
    }

    #[test]
    fn parse_list_groups_skips_no_id() {
        let result = json!([
            {"name": "No ID group"},
            {"id": "valid", "name": "Has ID"}
        ]);
        let event = parse_rpc_result("listGroups", &result, None).unwrap();
        match event {
            SignalEvent::GroupList(groups) => {
                assert_eq!(groups.len(), 1);
                assert_eq!(groups[0].id, "valid");
            }
            _ => panic!("Expected GroupList"),
        }
    }

    #[test]
    fn parse_rpc_result_unknown_method_returns_none() {
        let result = json!([]);
        assert!(parse_rpc_result("unknownMethod", &result, None).is_none());
    }

    #[test]
    fn parse_rpc_result_non_array_returns_none() {
        let result = json!({"not": "an array"});
        assert!(parse_rpc_result("listContacts", &result, None).is_none());
        assert!(parse_rpc_result("listGroups", &result, None).is_none());
    }

    #[test]
    fn parse_list_contacts_empty_array() {
        let result = json!([]);
        let event = parse_rpc_result("listContacts", &result, None).unwrap();
        match event {
            SignalEvent::ContactList(contacts) => assert!(contacts.is_empty()),
            _ => panic!("Expected ContactList"),
        }
    }

    #[test]
    fn parse_list_groups_empty_array() {
        let result = json!([]);
        let event = parse_rpc_result("listGroups", &result, None).unwrap();
        match event {
            SignalEvent::GroupList(groups) => assert!(groups.is_empty()),
            _ => panic!("Expected GroupList"),
        }
    }

    #[test]
    fn parse_send_result_extracts_timestamp() {
        let result = json!({"timestamp": 1700000000123_i64});
        let event = parse_rpc_result("send", &result, Some("rpc-42")).unwrap();
        match event {
            SignalEvent::SendTimestamp { rpc_id, server_ts } => {
                assert_eq!(rpc_id, "rpc-42");
                assert_eq!(server_ts, 1700000000123);
            }
            _ => panic!("Expected SendTimestamp"),
        }
    }

    #[test]
    fn parse_send_result_without_id_returns_none() {
        let result = json!({"timestamp": 1700000000123_i64});
        assert!(parse_rpc_result("send", &result, None).is_none());
    }

    #[rstest]
    #[case(true, false, false, "DELIVERY", 2)]
    #[case(false, true, false, "READ", 1)]
    fn parse_receipt_event(
        #[case] is_delivery: bool,
        #[case] is_read: bool,
        #[case] is_viewed: bool,
        #[case] expected_type: &str,
        #[case] expected_count: usize,
    ) {
        let mut timestamps = vec![json!(1700000000001_i64)];
        if expected_count == 2 {
            timestamps.push(json!(1700000000002_i64));
        }
        let resp = make_resp(json!({
            "envelope": {
                "sourceNumber": "+15551234567",
                "timestamp": 1700000000000_i64,
                "receiptMessage": {
                    "when": 1700000000000_i64,
                    "isDelivery": is_delivery,
                    "isRead": is_read,
                    "isViewed": is_viewed,
                    "timestamps": timestamps
                }
            }
        }));
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::ReceiptReceived {
                sender,
                receipt_type,
                timestamps,
            } => {
                assert_eq!(sender, "+15551234567");
                assert_eq!(receipt_type, expected_type);
                assert_eq!(timestamps.len(), expected_count);
                assert_eq!(timestamps[0], 1700000000001);
                if expected_count == 2 {
                    assert_eq!(timestamps[1], 1700000000002);
                }
            }
            _ => panic!("Expected ReceiptReceived, got {:?}", event),
        }
    }

    #[test]
    fn parse_reaction_incoming() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("receive".to_string()),
            params: Some(json!({
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "sourceName": "Alice",
                    "timestamp": 1700000000000_i64,
                    "dataMessage": {
                        "timestamp": 1700000000000_i64,
                        "reaction": {
                            "emoji": "👍",
                            "targetAuthor": "+15559876543",
                            "targetSentTimestamp": 1699999999000_i64,
                            "isRemove": false
                        }
                    }
                }
            })),
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::ReactionReceived {
                conv_id,
                emoji,
                sender,
                sender_name,
                target_author,
                target_timestamp,
                is_remove,
            } => {
                assert_eq!(conv_id, "+15551234567");
                assert_eq!(emoji, "👍");
                assert_eq!(sender, "+15551234567");
                assert_eq!(sender_name.as_deref(), Some("Alice"));
                assert_eq!(target_author, "+15559876543");
                assert_eq!(target_timestamp, 1699999999000);
                assert!(!is_remove);
            }
            _ => panic!("Expected ReactionReceived, got {:?}", event),
        }
    }

    #[test]
    fn parse_reaction_remove() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("receive".to_string()),
            params: Some(json!({
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "timestamp": 1700000000000_i64,
                    "dataMessage": {
                        "timestamp": 1700000000000_i64,
                        "reaction": {
                            "emoji": "👍",
                            "targetAuthor": "+15559876543",
                            "targetSentTimestamp": 1699999999000_i64,
                            "isRemove": true
                        }
                    }
                }
            })),
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::ReactionReceived { is_remove, .. } => {
                assert!(is_remove);
            }
            _ => panic!("Expected ReactionReceived, got {:?}", event),
        }
    }

    #[test]
    fn parse_reaction_group() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("receive".to_string()),
            params: Some(json!({
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "sourceName": "Alice",
                    "timestamp": 1700000000000_i64,
                    "dataMessage": {
                        "timestamp": 1700000000000_i64,
                        "groupInfo": {
                            "groupId": "group123",
                            "groupName": "Family"
                        },
                        "reaction": {
                            "emoji": "❤️",
                            "targetAuthor": "+15559876543",
                            "targetSentTimestamp": 1699999999000_i64,
                            "isRemove": false
                        }
                    }
                }
            })),
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::ReactionReceived { conv_id, .. } => {
                assert_eq!(conv_id, "group123");
            }
            _ => panic!("Expected ReactionReceived, got {:?}", event),
        }
    }

    #[test]
    fn parse_reaction_sync() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("receive".to_string()),
            params: Some(json!({
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "timestamp": 1700000000000_i64,
                    "syncMessage": {
                        "sentMessage": {
                            "timestamp": 1700000000000_i64,
                            "destinationNumber": "+15559876543",
                            "reaction": {
                                "emoji": "😂",
                                "targetAuthor": "+15559876543",
                                "targetSentTimestamp": 1699999999000_i64,
                                "isRemove": false
                            }
                        }
                    }
                }
            })),
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::ReactionReceived {
                conv_id,
                emoji,
                sender,
                target_author,
                ..
            } => {
                assert_eq!(conv_id, "+15559876543");
                assert_eq!(emoji, "😂");
                assert_eq!(sender, "+15551234567");
                assert_eq!(target_author, "+15559876543");
            }
            _ => panic!("Expected ReactionReceived, got {:?}", event),
        }
    }

    #[test]
    fn parse_data_message_with_mentions() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("receive".to_string()),
            params: Some(json!({
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "sourceName": "Alice",
                    "timestamp": 1700000000000_i64,
                    "dataMessage": {
                        "timestamp": 1700000000000_i64,
                        "message": "\u{FFFC} check this out",
                        "mentions": [
                            {"start": 0, "length": 1, "uuid": "abc-def-123"}
                        ]
                    }
                }
            })),
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::MessageReceived(msg) => {
                assert_eq!(msg.mentions.len(), 1);
                assert_eq!(msg.mentions[0].start, 0);
                assert_eq!(msg.mentions[0].length, 1);
                assert_eq!(msg.mentions[0].uuid, "abc-def-123");
                assert!(msg.body.unwrap().contains('\u{FFFC}'));
            }
            _ => panic!("Expected MessageReceived, got {:?}", event),
        }
    }

    #[test]
    fn parse_sent_sync_with_mentions() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("receive".to_string()),
            params: Some(json!({
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "timestamp": 1700000000000_i64,
                    "syncMessage": {
                        "sentMessage": {
                            "timestamp": 1700000000000_i64,
                            "destinationNumber": "+15559876543",
                            "message": "Hey \u{FFFC}!",
                            "mentions": [
                                {"start": 4, "length": 1, "uuid": "xyz-456"}
                            ]
                        }
                    }
                }
            })),
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::MessageReceived(msg) => {
                assert!(msg.is_outgoing);
                assert_eq!(msg.mentions.len(), 1);
                assert_eq!(msg.mentions[0].start, 4);
                assert_eq!(msg.mentions[0].uuid, "xyz-456");
            }
            _ => panic!("Expected MessageReceived, got {:?}", event),
        }
    }

    #[test]
    fn parse_no_mentions_backward_compat() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("receive".to_string()),
            params: Some(json!({
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "timestamp": 1700000000000_i64,
                    "dataMessage": {
                        "timestamp": 1700000000000_i64,
                        "message": "Hello world"
                    }
                }
            })),
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::MessageReceived(msg) => {
                assert!(msg.mentions.is_empty());
                assert_eq!(msg.body.unwrap(), "Hello world");
            }
            _ => panic!("Expected MessageReceived, got {:?}", event),
        }
    }

    #[test]
    fn parse_message_from_uuid_only_contact() {
        // Regression test for #315: contacts with phone-number privacy enabled
        // arrive with no sourceNumber, only sourceUuid. The conv_id should fall
        // back to the UUID (not "unknown") so replies route correctly through
        // signal-cli's UUID-accepting recipient field.
        let uuid = "abcdef12-3456-7890-abcd-ef1234567890";
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("receive".to_string()),
            params: Some(json!({
                "envelope": {
                    "sourceUuid": uuid,
                    "sourceName": "Privacy Fan",
                    "timestamp": 1700000000000_i64,
                    "dataMessage": {
                        "timestamp": 1700000000000_i64,
                        "message": "hi"
                    }
                }
            })),
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::MessageReceived(msg) => {
                assert_eq!(
                    msg.source, uuid,
                    "expected source to fall back to UUID when sourceNumber is missing"
                );
                assert_eq!(msg.source_uuid.as_deref(), Some(uuid));
            }
            _ => panic!("Expected MessageReceived, got {:?}", event),
        }
    }

    #[test]
    fn parse_list_contacts_with_uuid() {
        let result = json!([
            {"number": "+15551234567", "profileName": "Alice", "uuid": "abc-def-123"},
            {"number": "+15559876543", "contactName": "Bob"}
        ]);
        let event = parse_rpc_result("listContacts", &result, None).unwrap();
        match event {
            SignalEvent::ContactList(contacts) => {
                assert_eq!(contacts[0].uuid.as_deref(), Some("abc-def-123"));
                assert_eq!(contacts[1].uuid, None);
            }
            _ => panic!("Expected ContactList"),
        }
    }

    // --- System message tests ---

    #[rstest]
    #[case("AUDIO_CALL", "Missed voice call")]
    #[case("VIDEO_CALL", "Missed video call")]
    fn parse_call_message(#[case] call_type: &str, #[case] expected_body: &str) {
        let resp = make_resp(json!({
            "envelope": {
                "sourceNumber": "+15551234567",
                "sourceName": "Alice",
                "timestamp": 1700000000000_i64,
                "callMessage": {
                    "offerMessage": {
                        "type": call_type,
                        "id": 12345
                    }
                }
            }
        }));
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::SystemMessage { conv_id, body, .. } => {
                assert_eq!(conv_id, "+15551234567");
                assert_eq!(body, expected_body);
            }
            _ => panic!("Expected SystemMessage, got {:?}", event),
        }
    }

    #[test]
    fn parse_call_message_ignores_hangup() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("receive".to_string()),
            params: Some(json!({
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "timestamp": 1700000000000_i64,
                    "callMessage": {
                        "hangupMessage": {
                            "id": 12345,
                            "type": "NORMAL"
                        }
                    }
                }
            })),
        };
        assert!(parse_signal_event(&resp, std::path::Path::new("/tmp")).is_none());
    }

    #[test]
    fn parse_untrusted_identity() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("receive".to_string()),
            params: Some(json!({
                "exception": {
                    "type": "UntrustedIdentityException",
                    "message": "Untrusted identity for +15551234567"
                },
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "timestamp": 1700000000000_i64
                }
            })),
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::SystemMessage { conv_id, body, .. } => {
                assert_eq!(conv_id, "+15551234567");
                assert!(body.contains("Safety number changed"));
            }
            _ => panic!("Expected SystemMessage, got {:?}", event),
        }
    }

    #[test]
    fn parse_group_update() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("receive".to_string()),
            params: Some(json!({
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "timestamp": 1700000000000_i64,
                    "dataMessage": {
                        "timestamp": 1700000000000_i64,
                        "groupInfo": {
                            "groupId": "group123",
                            "type": "UPDATE"
                        }
                    }
                }
            })),
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::SystemMessage { conv_id, body, .. } => {
                assert_eq!(conv_id, "group123");
                assert_eq!(body, "Group updated");
            }
            _ => panic!("Expected SystemMessage, got {:?}", event),
        }
    }

    #[rstest]
    #[case(604800, "Disappearing messages set to 1 week")]
    #[case(0, "Disappearing messages disabled")]
    fn parse_expiration(#[case] expire_seconds: i64, #[case] expected_body: &str) {
        let resp = make_resp(json!({
            "envelope": {
                "sourceNumber": "+15551234567",
                "timestamp": 1700000000000_i64,
                "dataMessage": {
                    "timestamp": 1700000000000_i64,
                    "isExpirationUpdate": true,
                    "expiresInSeconds": expire_seconds
                }
            }
        }));
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::ExpirationTimerChanged {
                conv_id,
                seconds,
                body,
                ..
            } => {
                assert_eq!(conv_id, "+15551234567");
                assert_eq!(seconds, expire_seconds);
                assert_eq!(body, expected_body);
            }
            _ => panic!("Expected ExpirationTimerChanged, got {:?}", event),
        }
    }

    #[test]
    fn parse_read_sync_basic() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("receive".to_string()),
            params: Some(json!({
                "envelope": {
                    "sourceNumber": "+10000000000",
                    "timestamp": 1700000000000_i64,
                    "syncMessage": {
                        "readMessages": [
                            {"sender": "+15551234567", "timestamp": 1700000000001_i64},
                            {"sender": "+15559876543", "timestamp": 1700000000002_i64}
                        ]
                    }
                }
            })),
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::ReadSyncReceived { read_messages } => {
                assert_eq!(read_messages.len(), 2);
                assert_eq!(
                    read_messages[0],
                    ("+15551234567".to_string(), 1700000000001)
                );
                assert_eq!(
                    read_messages[1],
                    ("+15559876543".to_string(), 1700000000002)
                );
            }
            _ => panic!("Expected ReadSyncReceived, got {:?}", event),
        }
    }

    #[test]
    fn parse_read_sync_empty_array_returns_none() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("receive".to_string()),
            params: Some(json!({
                "envelope": {
                    "sourceNumber": "+10000000000",
                    "timestamp": 1700000000000_i64,
                    "syncMessage": {
                        "readMessages": []
                    }
                }
            })),
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp"));
        assert!(event.is_none());
    }

    // --- Sticker message tests ---

    #[rstest]
    #[case(Some("\u{1F602}"), false, "[Sticker: \u{1F602}]", false)]
    #[case(None, false, "[Sticker]", false)]
    #[case(Some("\u{1F602}"), true, "[Sticker: \u{1F602}]", true)]
    fn parse_sticker_message(
        #[case] emoji: Option<&str>,
        #[case] is_sync: bool,
        #[case] expected_body: &str,
        #[case] expected_outgoing: bool,
    ) {
        let mut sticker = json!({
            "packId": "abc123",
            "stickerId": 5
        });
        if let Some(e) = emoji {
            sticker["emoji"] = json!(e);
        }
        let resp = if is_sync {
            make_resp(json!({
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "timestamp": 1700000000000_i64,
                    "syncMessage": {
                        "sentMessage": {
                            "timestamp": 1700000000000_i64,
                            "destinationNumber": "+15559876543",
                            "sticker": sticker
                        }
                    }
                }
            }))
        } else {
            make_resp(json!({
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "sourceName": "Alice",
                    "timestamp": 1700000000000_i64,
                    "dataMessage": {
                        "timestamp": 1700000000000_i64,
                        "sticker": sticker
                    }
                }
            }))
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::MessageReceived(msg) => {
                assert_eq!(msg.body.as_deref(), Some(expected_body));
                assert_eq!(msg.is_outgoing, expected_outgoing);
                if is_sync {
                    assert_eq!(msg.destination.as_deref(), Some("+15559876543"));
                }
            }
            _ => panic!("Expected MessageReceived, got {:?}", event),
        }
    }

    // --- View-once message tests ---

    #[rstest]
    #[case(true, false, "[View-once message]")]
    #[case(false, false, "normal text")]
    #[case(true, true, "[View-once message]")]
    fn parse_view_once(
        #[case] view_once: bool,
        #[case] is_sync: bool,
        #[case] expected_body: &str,
    ) {
        let resp = if is_sync {
            make_resp(json!({
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "timestamp": 1700000000000_i64,
                    "syncMessage": {
                        "sentMessage": {
                            "timestamp": 1700000000000_i64,
                            "destinationNumber": "+15559876543",
                            "message": "secret outgoing",
                            "viewOnce": view_once,
                            "attachments": [
                                {"contentType": "image/png", "filename": "secret.png", "size": 999}
                            ]
                        }
                    }
                }
            }))
        } else if view_once {
            make_resp(json!({
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "sourceName": "Alice",
                    "timestamp": 1700000000000_i64,
                    "dataMessage": {
                        "timestamp": 1700000000000_i64,
                        "message": "secret text",
                        "viewOnce": true,
                        "attachments": [
                            {"contentType": "image/jpeg", "filename": "photo.jpg", "size": 12345}
                        ]
                    }
                }
            }))
        } else {
            make_resp(json!({
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "sourceName": "Alice",
                    "timestamp": 1700000000000_i64,
                    "dataMessage": {
                        "timestamp": 1700000000000_i64,
                        "message": "normal text",
                        "viewOnce": false
                    }
                }
            }))
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::MessageReceived(msg) => {
                assert_eq!(msg.body.as_deref(), Some(expected_body));
                if view_once {
                    assert!(msg.attachments.is_empty());
                }
                if is_sync {
                    assert!(msg.is_outgoing);
                }
            }
            _ => panic!("Expected MessageReceived, got {:?}", event),
        }
    }

    // --- Text style parsing tests ---

    #[test]
    fn parse_text_styles_basic() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("receive".to_string()),
            params: Some(json!({
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "sourceName": "Alice",
                    "timestamp": 1700000000000_i64,
                    "dataMessage": {
                        "timestamp": 1700000000000_i64,
                        "message": "hello bold world",
                        "textStyles": [
                            {"start": 6, "length": 4, "style": "BOLD"},
                            {"start": 11, "length": 5, "style": "ITALIC"}
                        ]
                    }
                }
            })),
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::MessageReceived(msg) => {
                assert_eq!(msg.text_styles.len(), 2);
                assert_eq!(msg.text_styles[0].start, 6);
                assert_eq!(msg.text_styles[0].length, 4);
                assert_eq!(msg.text_styles[0].style, StyleType::Bold);
                assert_eq!(msg.text_styles[1].start, 11);
                assert_eq!(msg.text_styles[1].length, 5);
                assert_eq!(msg.text_styles[1].style, StyleType::Italic);
            }
            _ => panic!("Expected MessageReceived, got {:?}", event),
        }
    }

    #[test]
    fn parse_text_styles_empty_or_missing() {
        // No textStyles array at all
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("receive".to_string()),
            params: Some(json!({
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "timestamp": 1700000000000_i64,
                    "dataMessage": {
                        "timestamp": 1700000000000_i64,
                        "message": "plain text"
                    }
                }
            })),
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::MessageReceived(msg) => {
                assert!(msg.text_styles.is_empty());
            }
            _ => panic!("Expected MessageReceived, got {:?}", event),
        }

        // Empty textStyles array
        let resp2 = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("receive".to_string()),
            params: Some(json!({
                "envelope": {
                    "sourceNumber": "+15551234567",
                    "timestamp": 1700000000000_i64,
                    "dataMessage": {
                        "timestamp": 1700000000000_i64,
                        "message": "plain text",
                        "textStyles": []
                    }
                }
            })),
        };
        let event2 = parse_signal_event(&resp2, std::path::Path::new("/tmp")).unwrap();
        match event2 {
            SignalEvent::MessageReceived(msg) => {
                assert!(msg.text_styles.is_empty());
            }
            _ => panic!("Expected MessageReceived, got {:?}", event2),
        }
    }

    #[test]
    fn parse_poll_create_basic() {
        let resp = make_resp(json!({
            "envelope": {
                "sourceNumber": "+15551234567",
                "sourceName": "Alice",
                "timestamp": 1700000000000i64,
                "dataMessage": {
                    "timestamp": 1700000000000i64,
                    "pollCreate": {
                        "question": "What for lunch?",
                        "allowMultiple": true,
                        "options": [
                            {"id": 0, "optionText": "Pizza"},
                            {"id": 1, "optionText": "Sushi"},
                            {"id": 2, "optionText": "Tacos"}
                        ]
                    }
                }
            }
        }));
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::PollCreated {
                conv_id,
                timestamp,
                poll_data,
            } => {
                assert_eq!(conv_id, "+15551234567");
                assert_eq!(timestamp, 1700000000000);
                assert_eq!(poll_data.question, "What for lunch?");
                assert!(poll_data.allow_multiple);
                assert_eq!(poll_data.options.len(), 3);
                assert_eq!(poll_data.options[0].text, "Pizza");
                assert_eq!(poll_data.options[2].text, "Tacos");
                assert!(!poll_data.closed);
            }
            _ => panic!("Expected PollCreated, got {event:?}"),
        }
    }

    #[test]
    fn parse_poll_vote_basic() {
        let resp = make_resp(json!({
            "envelope": {
                "sourceNumber": "+15559876543",
                "sourceName": "Bob",
                "timestamp": 1700000001000i64,
                "dataMessage": {
                    "timestamp": 1700000001000i64,
                    "pollVote": {
                        "authorNumber": "+15559876543",
                        "targetSentTimestamp": 1700000000000i64,
                        "optionIndexes": [0, 2],
                        "voteCount": 1
                    }
                }
            }
        }));
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::PollVoteReceived {
                conv_id,
                target_timestamp,
                voter,
                option_indexes,
                vote_count,
                ..
            } => {
                assert_eq!(conv_id, "+15559876543");
                assert_eq!(target_timestamp, 1700000000000);
                assert_eq!(voter, "+15559876543");
                assert_eq!(option_indexes, vec![0, 2]);
                assert_eq!(vote_count, 1);
            }
            _ => panic!("Expected PollVoteReceived, got {event:?}"),
        }
    }

    #[test]
    fn parse_poll_terminate_basic() {
        let resp = make_resp(json!({
            "envelope": {
                "sourceNumber": "+15551234567",
                "sourceName": "Alice",
                "timestamp": 1700000002000i64,
                "dataMessage": {
                    "timestamp": 1700000002000i64,
                    "pollTerminate": {
                        "targetSentTimestamp": 1700000000000i64
                    }
                }
            }
        }));
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::PollTerminated {
                conv_id,
                target_timestamp,
            } => {
                assert_eq!(conv_id, "+15551234567");
                assert_eq!(target_timestamp, 1700000000000);
            }
            _ => panic!("Expected PollTerminated, got {event:?}"),
        }
    }

    // --- Link preview parsing ---

    #[test]
    fn parse_link_preview_basic() {
        let data = json!({
            "previews": [{
                "url": "https://example.com/article",
                "title": "Example Article",
                "description": "An interesting article",
                "image": {
                    "id": "abc123",
                    "contentType": "image/jpeg"
                }
            }]
        });
        let previews = parse_link_previews(&data, std::path::Path::new("/tmp"));
        assert_eq!(previews.len(), 1);
        assert_eq!(previews[0].url, "https://example.com/article");
        assert_eq!(previews[0].title.as_deref(), Some("Example Article"));
        assert_eq!(
            previews[0].description.as_deref(),
            Some("An interesting article")
        );
    }

    #[test]
    fn parse_link_preview_missing() {
        let data = json!({"body": "hello"});
        let previews = parse_link_previews(&data, std::path::Path::new("/tmp"));
        assert!(previews.is_empty());
    }

    #[test]
    fn parse_link_preview_singular_key() {
        // signal-cli may use "preview" (singular) instead of "previews"
        let data = json!({
            "preview": [{
                "url": "https://example.com",
                "title": "Test"
            }]
        });
        let previews = parse_link_previews(&data, std::path::Path::new("/tmp"));
        assert_eq!(previews.len(), 1);
        assert_eq!(previews[0].url, "https://example.com");
        assert_eq!(previews[0].title.as_deref(), Some("Test"));
        assert!(previews[0].description.is_none());
        assert!(previews[0].image_path.is_none());
    }

    #[test]
    fn parse_identity_list() {
        let result = json!([
            {
                "number": "+15551234567",
                "uuid": "uuid-alice",
                "fingerprint": "05ab12cd",
                "safetyNumber": "123456789012345678901234567890123456789012345678901234567890",
                "trustLevel": "TRUSTED_VERIFIED",
                "addedTimestamp": 1700000000000_i64
            },
            {
                "number": "+15559876543",
                "uuid": "uuid-bob",
                "fingerprint": "05ef34ab",
                "safetyNumber": "098765432109876543210987654321098765432109876543210987654321",
                "trustLevel": "UNTRUSTED",
                "addedTimestamp": 1700000001000_i64
            },
            {
                "number": "+15550001111",
                "trustLevel": "TRUSTED_UNVERIFIED"
            }
        ]);
        let event = parse_rpc_result("listIdentities", &result, None).unwrap();
        match event {
            SignalEvent::IdentityList(identities) => {
                assert_eq!(identities.len(), 3);
                assert_eq!(identities[0].number.as_deref(), Some("+15551234567"));
                assert_eq!(identities[0].uuid.as_deref(), Some("uuid-alice"));
                assert_eq!(identities[0].trust_level, TrustLevel::TrustedVerified);
                assert_eq!(identities[0].fingerprint, "05ab12cd");
                assert_eq!(identities[1].trust_level, TrustLevel::Untrusted);
                assert_eq!(identities[2].trust_level, TrustLevel::TrustedUnverified);
                assert_eq!(identities[2].fingerprint, "");
                assert_eq!(identities[2].safety_number, "");
            }
            _ => panic!("Expected IdentityList"),
        }
    }

    // --- Typing indicator parsing ---

    #[test]
    fn parse_typing_indicator_group_carries_group_id() {
        // When a typing event arrives for a group message, the parsed event
        // must include the group's ID so the app can key it correctly.
        let params = json!({
            "envelope": {
                "sourceNumber": "+15551234567",
                "sourceName": "Alice",
                "typingMessage": {
                    "action": "STARTED",
                    "groupId": "group-abc"
                }
            }
        });
        let event = parse_signal_event(&make_resp(params), std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::TypingIndicator {
                sender,
                group_id,
                is_typing,
                ..
            } => {
                assert_eq!(sender, "+15551234567");
                assert_eq!(group_id, Some("group-abc".to_string()));
                assert!(is_typing);
            }
            _ => panic!("Expected TypingIndicator"),
        }
    }

    #[test]
    fn parse_typing_indicator_direct_message_has_no_group_id() {
        let params = json!({
            "envelope": {
                "sourceNumber": "+15551234567",
                "typingMessage": {
                    "action": "STARTED"
                }
            }
        });
        let event = parse_signal_event(&make_resp(params), std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::TypingIndicator { group_id, .. } => {
                assert_eq!(group_id, None);
            }
            _ => panic!("Expected TypingIndicator"),
        }
    }
}
