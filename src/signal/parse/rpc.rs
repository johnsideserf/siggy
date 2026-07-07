//! RPC response parser: handles correlated responses for methods we sent
//! (listContacts, listGroups, listIdentities, etc) and dispatches to the
//! per-method shape converter.

use crate::signal::types::*;

pub fn parse_rpc_result(
    method: &str,
    result: &serde_json::Value,
    rpc_id: Option<&str>,
) -> Option<SignalEvent> {
    match method {
        "send" => {
            let id = rpc_id?.to_string();
            // signal-cli send response includes result.timestamp (server-assigned ms epoch)
            let server_ts = result
                .get("timestamp")
                .and_then(|v| v.as_i64())
                .or_else(|| result.as_i64())
                .unwrap_or(0);
            Some(SignalEvent::SendTimestamp {
                token: SendToken::new(id),
                server_ts,
            })
        }
        "listContacts" => {
            let arr = result.as_array()?;
            let contacts: Vec<Contact> = arr
                .iter()
                .filter_map(|obj| {
                    let number = obj
                        .get("number")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let name = obj
                        .get("profileName")
                        .and_then(|v| v.as_str())
                        .or_else(|| obj.get("contactName").and_then(|v| v.as_str()))
                        .or_else(|| obj.get("name").and_then(|v| v.as_str()))
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    let uuid = obj
                        .get("uuid")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let username = obj
                        .get("username")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    // Username-only contacts have no number; keep them as long
                    // as some stable identifier exists (#612).
                    if number.is_none() && uuid.is_none() {
                        return None;
                    }
                    Some(Contact {
                        number,
                        name,
                        uuid,
                        username,
                    })
                })
                .collect();
            Some(SignalEvent::ContactList(contacts))
        }
        "listGroups" => {
            let arr = result.as_array()?;
            let groups: Vec<Group> = arr
                .iter()
                .filter_map(|obj| {
                    let id = obj.get("id").and_then(|v| v.as_str())?;
                    let name = obj
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let mut members = Vec::new();
                    let mut member_uuids = Vec::new();
                    if let Some(arr) = obj.get("members").and_then(|v| v.as_array()) {
                        for m in arr {
                            // signal-cli returns members as objects: {"number": "+1...", "uuid": "..."}
                            // Fall back to plain string for compatibility
                            let phone = m
                                .get("number")
                                .and_then(|v| v.as_str())
                                .or_else(|| m.as_str());
                            if let Some(phone) = phone {
                                members.push(phone.to_string());
                                if let Some(uuid) = m.get("uuid").and_then(|v| v.as_str()) {
                                    member_uuids.push((phone.to_string(), uuid.to_string()));
                                }
                            }
                        }
                    }
                    Some(Group {
                        id: id.to_string(),
                        name,
                        members,
                        member_uuids,
                    })
                })
                .collect();
            Some(SignalEvent::GroupList(groups))
        }
        "listIdentities" => {
            let arr = result.as_array()?;
            let identities: Vec<IdentityInfo> = arr
                .iter()
                .map(|obj| {
                    let number = obj
                        .get("number")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let uuid = obj
                        .get("uuid")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let fingerprint = obj
                        .get("fingerprint")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let safety_number = obj
                        .get("safetyNumber")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let trust_level = obj
                        .get("trustLevel")
                        .and_then(|v| v.as_str())
                        .map(TrustLevel::from_str)
                        .unwrap_or(TrustLevel::TrustedUnverified);
                    let added_timestamp = obj
                        .get("addedTimestamp")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    IdentityInfo {
                        number,
                        uuid,
                        fingerprint,
                        safety_number,
                        trust_level,
                        added_timestamp,
                    }
                })
                .collect();
            Some(SignalEvent::IdentityList(identities))
        }
        "getUserStatus" => {
            let arr = result.as_array()?;
            let statuses: Vec<UserStatus> = arr
                .iter()
                .filter_map(|obj| {
                    let recipient = obj.get("recipient").and_then(|v| v.as_str())?;
                    let get_str =
                        |k: &str| obj.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
                    Some(UserStatus {
                        recipient: recipient.to_string(),
                        username: get_str("username"),
                        uuid: get_str("uuid"),
                        registered: obj
                            .get("isRegistered")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                    })
                })
                .collect();
            Some(SignalEvent::UserStatusList(statuses))
        }
        "sendPollCreate" => {
            let id = rpc_id?.to_string();
            let server_ts = result
                .get("timestamp")
                .and_then(|v| v.as_i64())
                .or_else(|| result.as_i64())
                .unwrap_or(0);
            Some(SignalEvent::SendTimestamp {
                token: SendToken::new(id),
                server_ts,
            })
        }
        "sendReaction"
        | "remoteDelete"
        | "sendTypingIndicator"
        | "sendReceipt"
        | "updateContact"
        | "updateGroup"
        | "quitGroup"
        | "sendMessageRequestResponse"
        | "block"
        | "unblock"
        | "sendPinMessage"
        | "sendUnpinMessage"
        | "sendPollVote"
        | "sendPollTerminate"
        | "trust" => None, // fire-and-forget, no action needed
        _ => None,
    }
}
