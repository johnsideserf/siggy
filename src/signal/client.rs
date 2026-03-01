use anyhow::{Context, Result};
use chrono::DateTime;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::config::Config;
use crate::signal::types::*;

/// Maximum age for pending RPC entries before they are considered stale.
const PENDING_REQUEST_TTL: Duration = Duration::from_secs(60);

pub struct SignalClient {
    child: Child,
    stdin_tx: mpsc::Sender<String>,
    pub event_rx: mpsc::Receiver<SignalEvent>,
    account: String,
    pending_requests: Arc<Mutex<HashMap<String, (String, Instant)>>>,
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
        let pending_requests: Arc<Mutex<HashMap<String, (String, Instant)>>> =
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
                        let rpc_id = resp.id.clone();
                        let pending_method = rpc_id.as_ref().and_then(|id| {
                            pending_clone.lock().ok().and_then(|mut map| {
                                let method = map.remove(id).map(|(m, _)| m);
                                // Sweep stale entries (signal-cli never responded)
                                map.retain(|_, (_, ts)| ts.elapsed() < PENDING_REQUEST_TTL);
                                method
                            })
                        });

                        let event = if let Some(method) = pending_method {
                            if resp.error.is_some() {
                                crate::debug_log::logf(format_args!("rpc error: method={method} error={:?}", resp.error));
                                // RPC error — emit SendFailed for send requests
                                if method == "send" {
                                    rpc_id.map(|id| SignalEvent::SendFailed { rpc_id: id })
                                } else {
                                    None
                                }
                            } else {
                                resp.result
                                    .as_ref()
                                    .and_then(|result| parse_rpc_result(&method, result, rpc_id.as_deref()))
                            }
                        } else {
                            parse_signal_event(&resp, &download_dir)
                        };

                        if let Some(ref event) = event {
                            crate::debug_log::logf(format_args!("event: {event:?}"));
                        }

                        if let Some(event) = event {
                            if event_tx.send(event).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        crate::debug_log::logf(format_args!("json parse error: {e}"));
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
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();

        // Track the RPC so we can correlate the response with a SendTimestamp/SendFailed event
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("send".to_string(), Instant::now()));
        }

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
            id: id.clone(),
            params: Some(params),
        };

        let json = serde_json::to_string(&request)?;
        self.stdin_tx
            .send(json)
            .await
            .context("Failed to send to signal-cli stdin")?;
        Ok(id)
    }

    pub async fn list_groups(&self) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("listGroups".to_string(), Instant::now()));
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
            map.insert(id.clone(), ("listContacts".to_string(), Instant::now()));
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

    pub async fn send_reaction(
        &self,
        recipient: &str,
        is_group: bool,
        emoji: &str,
        target_author: &str,
        target_timestamp: i64,
        remove: bool,
    ) -> Result<()> {
        let id = Uuid::new_v4().to_string();

        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("sendReaction".to_string(), Instant::now()));
        }

        let mut params = if is_group {
            serde_json::json!({
                "groupId": recipient,
                "emoji": emoji,
                "targetAuthor": target_author,
                "targetTimestamp": target_timestamp,
                "account": self.account,
            })
        } else {
            serde_json::json!({
                "recipient": recipient,
                "emoji": emoji,
                "targetAuthor": target_author,
                "targetTimestamp": target_timestamp,
                "account": self.account,
            })
        };

        if remove {
            params.as_object_mut().unwrap().insert("remove".to_string(), serde_json::json!(true));
        }

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "sendReaction".to_string(),
            id,
            params: Some(params),
        };

        let json = serde_json::to_string(&request)?;
        self.stdin_tx
            .send(json)
            .await
            .context("Failed to send reaction to signal-cli stdin")?;
        Ok(())
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        let _ = self.child.kill().await;
        Ok(())
    }
}

fn parse_rpc_result(method: &str, result: &serde_json::Value, rpc_id: Option<&str>) -> Option<SignalEvent> {
    match method {
        "send" => {
            let id = rpc_id?.to_string();
            // signal-cli send response includes result.timestamp (server-assigned ms epoch)
            let server_ts = result.get("timestamp").and_then(|v| v.as_i64())
                .or_else(|| result.as_i64())
                .unwrap_or(0);
            Some(SignalEvent::SendTimestamp { rpc_id: id, server_ts })
        }
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
        "sendReaction" => None, // reaction applied optimistically, no action needed
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
    // signal-cli reports exceptions for messages it can't parse (e.g. 1:1 sent sync)
    if let Some(exc) = params.get("exception") {
        let msg = exc.get("message").and_then(|v| v.as_str()).unwrap_or("unknown error");
        if msg.contains("SyncMessage missing destination") {
            return None; // Known signal-cli bug — silently ignore
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
    if let Some(sync) = envelope.get("syncMessage") {
        if let Some(sent) = sync.get("sentMessage") {
            return parse_sent_sync(envelope, sent, download_dir);
        }
        return None;
    }

    parse_data_message(envelope, download_dir)
}

fn parse_typing_indicator(envelope: &serde_json::Value) -> Option<SignalEvent> {
    let typing = envelope.get("typingMessage")?;
    let sender = envelope
        .get("sourceNumber")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
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
    Some(SignalEvent::TypingIndicator { sender, sender_name, is_typing })
}

fn parse_receipt_message(envelope: &serde_json::Value) -> Option<SignalEvent> {
    let receipt = envelope.get("receiptMessage")?;
    let sender = envelope
        .get("sourceNumber")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    // signal-cli uses boolean fields: isDelivery, isRead, isViewed
    let receipt_type = if receipt.get("isRead").and_then(|v| v.as_bool()).unwrap_or(false) {
        "READ"
    } else if receipt.get("isViewed").and_then(|v| v.as_bool()).unwrap_or(false) {
        "VIEWED"
    } else if receipt.get("isDelivery").and_then(|v| v.as_bool()).unwrap_or(false) {
        "DELIVERY"
    } else {
        // Fallback: try "type" string field (older signal-cli versions)
        receipt.get("type").and_then(|v| v.as_str()).unwrap_or("UNKNOWN")
    }.to_string();
    let timestamps: Vec<i64> = receipt
        .get("timestamps")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
        .unwrap_or_default();
    Some(SignalEvent::ReceiptReceived { sender, receipt_type, timestamps })
}

fn parse_data_message(
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
            let interesting: Vec<&&str> = keys.iter()
                .filter(|k| !matches!(**k,
                    "source" | "sourceNumber" | "sourceName" | "sourceUuid"
                    | "sourceDevice" | "timestamp" | "serverReceivedTimestamp"
                    | "serverDeliveredTimestamp" | "relay"
                ))
                .collect();
            if !interesting.is_empty() {
                return Some(SignalEvent::Error(
                    format!("unhandled envelope type: {}", interesting.iter()
                        .map(|k| **k)
                        .collect::<Vec<_>>()
                        .join(", "))
                ));
            }
            return None;
        }
    };

    // Check for reaction before extracting body/attachments
    if let Some(reaction) = data.get("reaction") {
        let group_id = data
            .get("groupInfo")
            .and_then(|g| g.get("groupId"))
            .and_then(|v| v.as_str());
        return parse_reaction(envelope, reaction, group_id);
    }

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
        destination: None,
    }))
}

fn parse_sent_sync(
    envelope: &serde_json::Value,
    sent: &serde_json::Value,
    download_dir: &std::path::Path,
) -> Option<SignalEvent> {
    // Check for synced reaction before extracting body/attachments
    if let Some(reaction) = sent.get("reaction") {
        return parse_reaction_sync(envelope, sent, reaction);
    }

    let source = envelope
        .get("sourceNumber")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let destination = sent
        .get("destinationNumber")
        .or_else(|| sent.get("destination"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let timestamp_ms = sent
        .get("timestamp")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let timestamp = DateTime::from_timestamp_millis(timestamp_ms).unwrap_or_default();

    let body = sent
        .get("message")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let group_id = sent
        .get("groupInfo")
        .and_then(|g| g.get("groupId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let group_name = sent
        .get("groupInfo")
        .and_then(|g| g.get("groupName"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let attachments = sent
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
        source_name: None,
        timestamp,
        body,
        attachments,
        group_id,
        group_name,
        is_outgoing: true,
        destination,
    }))
}

fn parse_reaction(
    envelope: &serde_json::Value,
    reaction: &serde_json::Value,
    group_id: Option<&str>,
) -> Option<SignalEvent> {
    let emoji = reaction.get("emoji").and_then(|v| v.as_str())?.to_string();
    let target_author = reaction.get("targetAuthor").and_then(|v| v.as_str())?.to_string();
    let target_timestamp = reaction.get("targetSentTimestamp").and_then(|v| v.as_i64())?;
    let is_remove = reaction.get("isRemove").and_then(|v| v.as_bool()).unwrap_or(false);

    let sender = envelope
        .get("sourceNumber")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
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

fn parse_reaction_sync(
    envelope: &serde_json::Value,
    sent: &serde_json::Value,
    reaction: &serde_json::Value,
) -> Option<SignalEvent> {
    let emoji = reaction.get("emoji").and_then(|v| v.as_str())?.to_string();
    let target_author = reaction.get("targetAuthor").and_then(|v| v.as_str())?.to_string();
    let target_timestamp = reaction.get("targetSentTimestamp").and_then(|v| v.as_i64())?;
    let is_remove = reaction.get("isRemove").and_then(|v| v.as_bool()).unwrap_or(false);

    let sender = envelope
        .get("sourceNumber")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

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

    // Generate a filename if signal-cli didn't provide one
    let mut effective_name = filename.clone().unwrap_or_else(|| {
        let ext = mime_to_ext(&content_type);
        // Use last 8 chars of attachment ID for uniqueness
        let short_id = if id.len() > 8 { &id[id.len() - 8..] } else { &id };
        format!("{short_id}.{ext}")
    });

    // Strip doubled extension (e.g. "photo.jpg.jpg" → "photo.jpg")
    if let Some(dot_pos) = effective_name.rfind('.') {
        let ext = &effective_name[dot_pos..]; // e.g. ".jpg"
        let base = &effective_name[..dot_pos];
        if base.ends_with(ext) {
            effective_name = base.to_string();
        }
    }

    let dest = download_dir.join(&effective_name);

    // Try to find the source file: explicit "file" field, or signal-cli's attachment dir
    let local_path = if dest.exists() {
        // Already copied previously
        Some(dest.to_string_lossy().to_string())
    } else {
        // Find source: "file" field from JSON, or signal-cli's attachment storage
        let src = value
            .get("file")
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from)
            .or_else(|| find_signal_cli_attachment(&id, &content_type));

        if let Some(src) = src.filter(|p| p.exists()) {
            let _ = std::fs::create_dir_all(download_dir);
            match std::fs::copy(&src, &dest) {
                Ok(_) => Some(dest.to_string_lossy().to_string()),
                Err(_) => Some(src.to_string_lossy().to_string()),
            }
        } else {
            None
        }
    };

    Some(Attachment {
        id,
        content_type,
        filename: Some(effective_name),
        local_path,
    })
}

/// Look for an attachment file in signal-cli's data directory by attachment ID.
/// signal-cli stores attachments as `{data_dir}/attachments/{id}.{ext}`.
///
/// Checks multiple locations since signal-cli may use platform-native data dirs
/// or POSIX-style ~/.local/share depending on how it was installed.
fn find_signal_cli_attachment(id: &str, content_type: &str) -> Option<std::path::PathBuf> {
    let mut candidates = Vec::new();
    if let Some(data_dir) = dirs::data_dir() {
        candidates.push(data_dir.join("signal-cli").join("attachments"));
    }
    // Also check ~/.local/share (POSIX-style, common on MSYS/WSL)
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local").join("share").join("signal-cli").join("attachments"));
    }

    let ext = mime_to_ext(content_type);

    for attachments_dir in &candidates {
        // Try with MIME-derived extension first
        let with_ext = attachments_dir.join(format!("{id}.{ext}"));
        if with_ext.exists() {
            return Some(with_ext);
        }

        // Scan directory for files matching the attachment ID
        if let Ok(entries) = std::fs::read_dir(attachments_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(id) {
                    return Some(entry.path());
                }
            }
        }
    }

    None
}

/// Map common MIME types to file extensions
fn mime_to_ext(mime: &str) -> &str {
    match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "video/mp4" => "mp4",
        "video/quicktime" => "mov",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "audio/aac" => "aac",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        _ => "bin",
    }
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
        let result = json!([
            {"id": "group1", "name": "Family", "members": ["+1", "+2"]},
            {"id": "group2", "name": "Work"}
        ]);
        let event = parse_rpc_result("listGroups", &result, None).unwrap();
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

    #[test]
    fn parse_receipt_event_extracts_type_and_timestamps() {
        // signal-cli uses boolean fields: isDelivery, isRead, isViewed
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
                    "receiptMessage": {
                        "when": 1700000000000_i64,
                        "isDelivery": true,
                        "isRead": false,
                        "isViewed": false,
                        "timestamps": [1700000000001_i64, 1700000000002_i64]
                    }
                }
            })),
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::ReceiptReceived { sender, receipt_type, timestamps } => {
                assert_eq!(sender, "+15551234567");
                assert_eq!(receipt_type, "DELIVERY");
                assert_eq!(timestamps, vec![1700000000001, 1700000000002]);
            }
            _ => panic!("Expected ReceiptReceived, got {:?}", event),
        }
    }

    #[test]
    fn parse_receipt_event_read() {
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
                    "receiptMessage": {
                        "when": 1700000000000_i64,
                        "isDelivery": false,
                        "isRead": true,
                        "isViewed": false,
                        "timestamps": [1700000000001_i64]
                    }
                }
            })),
        };
        let event = parse_signal_event(&resp, std::path::Path::new("/tmp")).unwrap();
        match event {
            SignalEvent::ReceiptReceived { receipt_type, .. } => {
                assert_eq!(receipt_type, "READ");
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
                conv_id, emoji, sender, sender_name, target_author, target_timestamp, is_remove,
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
                conv_id, emoji, sender, target_author, ..
            } => {
                assert_eq!(conv_id, "+15559876543");
                assert_eq!(emoji, "😂");
                assert_eq!(sender, "+15551234567");
                assert_eq!(target_author, "+15559876543");
            }
            _ => panic!("Expected ReactionReceived, got {:?}", event),
        }
    }
}
