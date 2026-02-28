use anyhow::{Context, Result};
use chrono::DateTime;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::config::Config;
use crate::signal::types::*;

pub struct SignalClient {
    child: Child,
    stdin_tx: mpsc::Sender<String>,
    pub event_rx: mpsc::Receiver<SignalEvent>,
    account: String,
    pending_requests: Arc<Mutex<HashMap<String, String>>>,
}

impl SignalClient {
    pub async fn spawn(config: &Config) -> Result<Self> {
        let mut cmd = Command::new(&config.signal_cli_path);
        if !config.account.is_empty() {
            cmd.arg("-a").arg(&config.account);
        }
        cmd.arg("jsonRpc");
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn().with_context(|| {
            format!(
                "Failed to spawn signal-cli at '{}'. Is it installed and in PATH?",
                config.signal_cli_path
            )
        })?;

        let stdout = child.stdout.take().context("Failed to capture stdout")?;
        let stdin = child.stdin.take().context("Failed to capture stdin")?;

        let (event_tx, event_rx) = mpsc::channel::<SignalEvent>(256);
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(64);

        let download_dir = config.download_dir.clone();
        let pending_requests: Arc<Mutex<HashMap<String, String>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_clone = Arc::clone(&pending_requests);

        // Stdout reader task — parse JSON-RPC messages from signal-cli
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }

                match serde_json::from_str::<JsonRpcResponse>(&line) {
                    Ok(resp) => {
                        // Check if this is a response to a pending request
                        let pending_method = resp.id.as_ref().and_then(|id| {
                            pending_clone.lock().ok().and_then(|mut map| map.remove(id))
                        });

                        let event = if let Some(method) = pending_method {
                            resp.result
                                .as_ref()
                                .and_then(|result| parse_rpc_result(&method, result))
                        } else {
                            parse_signal_event(&resp, &download_dir)
                        };

                        if let Some(event) = event {
                            if event_tx.send(event).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = event_tx
                            .send(SignalEvent::Error(format!("JSON parse error: {e}")))
                            .await;
                    }
                }
            }
        });

        // Stdin writer task — send JSON-RPC requests to signal-cli
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(msg) = stdin_rx.recv().await {
                if stdin.write_all(msg.as_bytes()).await.is_err() {
                    break;
                }
                if stdin.write_all(b"\n").await.is_err() {
                    break;
                }
                if stdin.flush().await.is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            child,
            stdin_tx,
            event_rx,
            account: config.account.clone(),
            pending_requests,
        })
    }

    pub async fn send_message(
        &self,
        recipient: &str,
        body: &str,
        is_group: bool,
    ) -> Result<()> {
        let id = Uuid::new_v4().to_string();

        let params = if is_group {
            serde_json::json!({
                "groupId": recipient,
                "message": body,
                "account": self.account,
            })
        } else {
            serde_json::json!({
                "recipient": [recipient],
                "message": body,
                "account": self.account,
            })
        };

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "send".to_string(),
            id,
            params: Some(params),
        };

        let json = serde_json::to_string(&request)?;
        self.stdin_tx
            .send(json)
            .await
            .context("Failed to send to signal-cli stdin")?;
        Ok(())
    }

    pub async fn list_groups(&self) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), "listGroups".to_string());
        }
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "listGroups".to_string(),
            id,
            params: Some(serde_json::json!({ "account": self.account })),
        };
        let json = serde_json::to_string(&request)?;
        self.stdin_tx.send(json).await.context("Failed to send")?;
        Ok(())
    }

    pub async fn list_contacts(&self) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), "listContacts".to_string());
        }
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "listContacts".to_string(),
            id,
            params: Some(serde_json::json!({ "account": self.account })),
        };
        let json = serde_json::to_string(&request)?;
        self.stdin_tx.send(json).await.context("Failed to send")?;
        Ok(())
    }

    pub async fn send_sync_request(&self) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "sendSyncRequest".to_string(),
            id,
            params: Some(serde_json::json!({ "account": self.account })),
        };
        let json = serde_json::to_string(&request)?;
        self.stdin_tx.send(json).await.context("Failed to send")?;
        Ok(())
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        let _ = self.child.kill().await;
        Ok(())
    }
}

fn parse_rpc_result(method: &str, result: &serde_json::Value) -> Option<SignalEvent> {
    match method {
        "listContacts" => {
            let arr = result.as_array()?;
            let contacts: Vec<Contact> = arr
                .iter()
                .filter_map(|obj| {
                    let number = obj.get("number").and_then(|v| v.as_str())?;
                    let name = obj
                        .get("profileName")
                        .and_then(|v| v.as_str())
                        .or_else(|| obj.get("contactName").and_then(|v| v.as_str()))
                        .or_else(|| obj.get("name").and_then(|v| v.as_str()))
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    Some(Contact {
                        number: number.to_string(),
                        name,
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
                    let members = obj
                        .get("members")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|m| m.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(Group {
                        id: id.to_string(),
                        name,
                        members,
                    })
                })
                .collect();
            Some(SignalEvent::GroupList(groups))
        }
        _ => None,
    }
}

fn parse_signal_event(
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

fn parse_receive_event(
    params: &serde_json::Value,
    download_dir: &std::path::Path,
) -> Option<SignalEvent> {
    let envelope = params.get("envelope")?;

    // Typing indicator
    if let Some(typing) = envelope.get("typingMessage") {
        let sender = envelope
            .get("sourceNumber")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let is_typing = typing
            .get("action")
            .and_then(|v| v.as_str())
            .map(|a| a == "STARTED")
            .unwrap_or(false);
        return Some(SignalEvent::TypingIndicator { sender, is_typing });
    }

    // Receipt
    if let Some(_receipt) = envelope.get("receiptMessage") {
        let sender = envelope
            .get("sourceNumber")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let timestamp = envelope
            .get("timestamp")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        return Some(SignalEvent::ReceiptReceived { sender, timestamp });
    }

    // Data message (actual text/attachments)
    let data = envelope.get("dataMessage")?;

    let source = envelope
        .get("sourceNumber")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let source_name = envelope
        .get("sourceName")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let timestamp_ms = data
        .get("timestamp")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let timestamp = DateTime::from_timestamp_millis(timestamp_ms)
        .unwrap_or_default();

    let body = data
        .get("message")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

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

    let attachments = data
        .get("attachments")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| parse_attachment(a, download_dir))
                .collect()
        })
        .unwrap_or_default();

    Some(SignalEvent::MessageReceived(SignalMessage {
        source,
        source_name,
        timestamp,
        body,
        attachments,
        group_id,
        group_name,
        is_outgoing: false,
    }))
}

fn parse_attachment(
    value: &serde_json::Value,
    download_dir: &std::path::Path,
) -> Option<Attachment> {
    let id = value.get("id").and_then(|v| v.as_str())?.to_string();
    let content_type = value
        .get("contentType")
        .and_then(|v| v.as_str())
        .unwrap_or("application/octet-stream")
        .to_string();
    let filename = value
        .get("filename")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // If signal-cli provides a file path, attempt to copy to download dir
    let local_path = value
        .get("file")
        .and_then(|v| v.as_str())
        .and_then(|src_path| {
            let src = std::path::Path::new(src_path);
            if !src.exists() {
                return None;
            }
            let dest_name = filename.as_deref().unwrap_or(&id);
            let dest = download_dir.join(dest_name);
            let _ = std::fs::create_dir_all(download_dir);
            match std::fs::copy(src, &dest) {
                Ok(_) => Some(dest.to_string_lossy().to_string()),
                Err(_) => Some(src_path.to_string()),
            }
        });

    Some(Attachment {
        id,
        content_type,
        filename,
        local_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- Test 2: listContacts parsing populates contacts ---

    #[test]
    fn parse_list_contacts_basic() {
        let result = json!([
            {"number": "+15551234567", "profileName": "Alice"},
            {"number": "+15559876543", "contactName": "Bob"}
        ]);
        let event = parse_rpc_result("listContacts", &result).unwrap();
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
        let event = parse_rpc_result("listContacts", &result).unwrap();
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
        let event = parse_rpc_result("listContacts", &result).unwrap();
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
        let event = parse_rpc_result("listContacts", &result).unwrap();
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
        let result = json!([
            {"id": "group1", "name": "Family", "members": ["+1", "+2"]},
            {"id": "group2", "name": "Work"}
        ]);
        let event = parse_rpc_result("listGroups", &result).unwrap();
        match event {
            SignalEvent::GroupList(groups) => {
                assert_eq!(groups.len(), 2);
                assert_eq!(groups[0].id, "group1");
                assert_eq!(groups[0].name, "Family");
                assert_eq!(groups[0].members, vec!["+1", "+2"]);
                assert_eq!(groups[1].id, "group2");
                assert_eq!(groups[1].name, "Work");
                assert!(groups[1].members.is_empty());
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
        let event = parse_rpc_result("listGroups", &result).unwrap();
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
        assert!(parse_rpc_result("unknownMethod", &result).is_none());
    }

    #[test]
    fn parse_rpc_result_non_array_returns_none() {
        let result = json!({"not": "an array"});
        assert!(parse_rpc_result("listContacts", &result).is_none());
        assert!(parse_rpc_result("listGroups", &result).is_none());
    }

    #[test]
    fn parse_list_contacts_empty_array() {
        let result = json!([]);
        let event = parse_rpc_result("listContacts", &result).unwrap();
        match event {
            SignalEvent::ContactList(contacts) => assert!(contacts.is_empty()),
            _ => panic!("Expected ContactList"),
        }
    }

    #[test]
    fn parse_list_groups_empty_array() {
        let result = json!([]);
        let event = parse_rpc_result("listGroups", &result).unwrap();
        match event {
            SignalEvent::GroupList(groups) => assert!(groups.is_empty()),
            _ => panic!("Expected GroupList"),
        }
    }
}
