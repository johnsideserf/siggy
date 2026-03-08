use anyhow::{Context, Result};
use chrono::DateTime;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use uuid::Uuid;

/// Maximum size of the stderr capture buffer (~1 MB).
const MAX_STDERR_LEN: usize = 1_000_000;

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
    stderr_buffer: Arc<Mutex<String>>,
}

impl SignalClient {
    pub async fn spawn(config: &Config) -> Result<Self> {
        let mut cmd = Command::new(&config.signal_cli_path);
        if !config.account.is_empty() {
            cmd.arg("-a").arg(&config.account);
        }
        if !config.proxy.is_empty() {
            cmd.arg("--proxy").arg(&config.proxy);
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
        let stderr = child.stderr.take().context("Failed to capture stderr")?;

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
                            if let Some(ref err) = resp.error {
                                crate::debug_log::logf(format_args!("rpc error: method={method} error={err:?}"));
                                // RPC error — emit SendFailed for send requests,
                                // surface other errors to the status bar
                                if method == "send" {
                                    rpc_id.map(|id| SignalEvent::SendFailed { rpc_id: id })
                                } else {
                                    Some(SignalEvent::Error(format!("{method}: {}", err.message)))
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
                            if crate::debug_log::redact() {
                                crate::debug_log::logf(format_args!("event: {}", event.redacted_summary()));
                            } else {
                                crate::debug_log::logf(format_args!("event: {event:?}"));
                            }
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

        // Stderr reader task — capture signal-cli error output
        let stderr_buffer: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let stderr_clone = Arc::clone(&stderr_buffer);
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                crate::debug_log::logf(format_args!("signal-cli stderr: {line}"));
                if let Ok(mut buf) = stderr_clone.lock() {
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
                    buf.push_str(&line);
                    if buf.len() > MAX_STDERR_LEN {
                        let drain_to = buf.len() - MAX_STDERR_LEN / 2;
                        buf.drain(..drain_to);
                    }
                }
            }
        });

        Ok(Self {
            child,
            stdin_tx,
            event_rx,
            account: config.account.clone(),
            pending_requests,
            stderr_buffer,
        })
    }

    pub async fn send_message(
        &self,
        recipient: &str,
        body: &str,
        is_group: bool,
        mentions: &[(usize, String)],
        attachments: &[&Path],
        quote: Option<(&str, i64, &str)>,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();

        // Track the RPC so we can correlate the response with a SendTimestamp/SendFailed event
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("send".to_string(), Instant::now()));
        }

        let mut params = if is_group {
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

        if !mentions.is_empty() {
            // signal-cli expects mentions as colon-separated strings: "start:length:uuid"
            let mention_arr: Vec<serde_json::Value> = mentions
                .iter()
                .map(|(start, uuid)| {
                    serde_json::Value::String(format!("{start}:1:{uuid}"))
                })
                .collect();
            params["mention"] = serde_json::Value::Array(mention_arr);
        }

        if !attachments.is_empty() {
            let att_arr: Vec<serde_json::Value> = attachments
                .iter()
                .map(|p| serde_json::Value::String(p.to_string_lossy().to_string()))
                .collect();
            params["attachment"] = serde_json::Value::Array(att_arr);
        }

        if let Some((author, timestamp, body_text)) = quote {
            params["quoteTimestamp"] = serde_json::json!(timestamp);
            params["quoteAuthor"] = serde_json::json!(author);
            params["quoteMessage"] = serde_json::json!(body_text);
        }

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

    pub async fn send_edit_message(
        &self,
        recipient: &str,
        body: &str,
        is_group: bool,
        edit_timestamp: i64,
        mentions: &[(usize, String)],
        quote: Option<(&str, i64, &str)>,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();

        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("send".to_string(), Instant::now()));
        }

        let mut params = if is_group {
            serde_json::json!({
                "groupId": recipient,
                "message": body,
                "account": self.account,
                "editTimestamp": edit_timestamp,
            })
        } else {
            serde_json::json!({
                "recipient": [recipient],
                "message": body,
                "account": self.account,
                "editTimestamp": edit_timestamp,
            })
        };

        if !mentions.is_empty() {
            let mention_arr: Vec<serde_json::Value> = mentions
                .iter()
                .map(|(start, uuid)| serde_json::Value::String(format!("{start}:1:{uuid}")))
                .collect();
            params["mention"] = serde_json::Value::Array(mention_arr);
        }

        if let Some((author, timestamp, body_text)) = quote {
            params["quoteTimestamp"] = serde_json::json!(timestamp);
            params["quoteAuthor"] = serde_json::json!(author);
            params["quoteMessage"] = serde_json::json!(body_text);
        }

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
            .context("Failed to send edit to signal-cli stdin")?;
        Ok(id)
    }

    pub async fn send_remote_delete(
        &self,
        recipient: &str,
        is_group: bool,
        target_timestamp: i64,
    ) -> Result<()> {
        let id = Uuid::new_v4().to_string();

        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("remoteDelete".to_string(), Instant::now()));
        }

        let params = if is_group {
            serde_json::json!({
                "groupId": recipient,
                "targetTimestamp": target_timestamp,
                "account": self.account,
            })
        } else {
            serde_json::json!({
                "recipient": [recipient],
                "targetTimestamp": target_timestamp,
                "account": self.account,
            })
        };

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "remoteDelete".to_string(),
            id,
            params: Some(params),
        };

        let json = serde_json::to_string(&request)?;
        self.stdin_tx
            .send(json)
            .await
            .context("Failed to send remote delete to signal-cli stdin")?;
        Ok(())
    }

    pub async fn send_pin_message(
        &self,
        recipient: &str,
        is_group: bool,
        target_author: &str,
        target_timestamp: i64,
        pin_duration: i64,
    ) -> Result<()> {
        let id = Uuid::new_v4().to_string();

        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("sendPinMessage".to_string(), Instant::now()));
        }

        let params = if is_group {
            serde_json::json!({
                "groupId": recipient,
                "targetAuthor": target_author,
                "targetTimestamp": target_timestamp,
                "pinDuration": pin_duration,
                "account": self.account,
            })
        } else {
            serde_json::json!({
                "recipient": [recipient],
                "targetAuthor": target_author,
                "targetTimestamp": target_timestamp,
                "pinDuration": pin_duration,
                "account": self.account,
            })
        };

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "sendPinMessage".to_string(),
            id,
            params: Some(params),
        };

        let json = serde_json::to_string(&request)?;
        self.stdin_tx
            .send(json)
            .await
            .context("Failed to send pin message to signal-cli stdin")?;
        Ok(())
    }

    pub async fn send_unpin_message(
        &self,
        recipient: &str,
        is_group: bool,
        target_author: &str,
        target_timestamp: i64,
    ) -> Result<()> {
        let id = Uuid::new_v4().to_string();

        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("sendUnpinMessage".to_string(), Instant::now()));
        }

        let params = if is_group {
            serde_json::json!({
                "groupId": recipient,
                "targetAuthor": target_author,
                "targetTimestamp": target_timestamp,
                "pinDuration": -1,
                "account": self.account,
            })
        } else {
            serde_json::json!({
                "recipient": [recipient],
                "targetAuthor": target_author,
                "targetTimestamp": target_timestamp,
                "pinDuration": -1,
                "account": self.account,
            })
        };

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "sendUnpinMessage".to_string(),
            id,
            params: Some(params),
        };

        let json = serde_json::to_string(&request)?;
        self.stdin_tx
            .send(json)
            .await
            .context("Failed to send unpin message to signal-cli stdin")?;
        Ok(())
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

    pub async fn list_identities(&self) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("listIdentities".to_string(), Instant::now()));
        }
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "listIdentities".to_string(),
            id,
            params: Some(serde_json::json!({ "account": self.account })),
        };
        let json = serde_json::to_string(&request)?;
        self.stdin_tx.send(json).await.context("Failed to send")?;
        Ok(())
    }

    pub async fn trust_identity(&self, recipient: &str, safety_number: &str) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("trust".to_string(), Instant::now()));
        }
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "trust".to_string(),
            id,
            params: Some(serde_json::json!({
                "recipient": [recipient],
                "verifiedSafetyNumber": safety_number,
                "account": self.account,
            })),
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
            params["remove"] = serde_json::json!(true);
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

    pub async fn send_typing(
        &self,
        recipient: &str,
        is_group: bool,
        stop: bool,
    ) -> Result<()> {
        let id = Uuid::new_v4().to_string();

        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("sendTypingIndicator".to_string(), Instant::now()));
        }

        let mut params = if is_group {
            serde_json::json!({
                "groupId": recipient,
                "account": self.account,
            })
        } else {
            serde_json::json!({
                "recipient": [recipient],
                "account": self.account,
            })
        };

        if stop {
            params["stop"] = serde_json::json!(true);
        }

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "sendTypingIndicator".to_string(),
            id,
            params: Some(params),
        };

        let json = serde_json::to_string(&request)?;
        self.stdin_tx
            .send(json)
            .await
            .context("Failed to send typing indicator to signal-cli stdin")?;
        Ok(())
    }

    /// Send a read receipt to a single recipient for one or more message timestamps.
    /// Fire-and-forget — no useful result is expected from signal-cli.
    pub async fn send_read_receipt(
        &self,
        recipient: &str,
        timestamps: &[i64],
    ) -> Result<()> {
        let id = Uuid::new_v4().to_string();

        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("sendReceipt".to_string(), Instant::now()));
        }

        let params = serde_json::json!({
            "recipient": [recipient],
            "type": "read",
            "targetTimestamp": timestamps,
            "account": self.account,
        });

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "sendReceipt".to_string(),
            id,
            params: Some(params),
        };

        let json = serde_json::to_string(&request)?;
        self.stdin_tx
            .send(json)
            .await
            .context("Failed to send read receipt to signal-cli stdin")?;
        Ok(())
    }

    /// Accept or delete a message request.
    pub async fn send_message_request_response(
        &self,
        recipient: &str,
        is_group: bool,
        response_type: &str,
    ) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("sendMessageRequestResponse".to_string(), Instant::now()));
        }
        let mut params = serde_json::json!({
            "type": response_type,
            "account": self.account,
        });
        if is_group {
            params["groupId"] = serde_json::json!(recipient);
        } else {
            params["recipient"] = serde_json::json!([recipient]);
        }
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "sendMessageRequestResponse".to_string(),
            id,
            params: Some(params),
        };
        let json = serde_json::to_string(&request)?;
        self.stdin_tx
            .send(json)
            .await
            .context("Failed to send message request response to signal-cli stdin")?;
        Ok(())
    }

    /// Set the disappearing message timer for a 1:1 contact.
    pub async fn send_update_contact_expiration(
        &self,
        recipient: &str,
        seconds: i64,
    ) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("updateContact".to_string(), Instant::now()));
        }
        let params = serde_json::json!({
            "recipient": recipient,
            "expiration": seconds,
            "account": self.account,
        });
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "updateContact".to_string(),
            id,
            params: Some(params),
        };
        let json = serde_json::to_string(&request)?;
        self.stdin_tx
            .send(json)
            .await
            .context("Failed to send updateContact to signal-cli stdin")?;
        Ok(())
    }

    /// Create a new group with the given name (optionally with initial members).
    pub async fn create_group(&self, name: &str, members: &[String]) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("updateGroup".to_string(), Instant::now()));
        }
        let mut params = serde_json::json!({
            "name": name,
            "account": self.account,
        });
        if !members.is_empty() {
            params["members"] = serde_json::json!(members);
        }
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "updateGroup".to_string(),
            id,
            params: Some(params),
        };
        let json = serde_json::to_string(&request)?;
        self.stdin_tx.send(json).await.context("Failed to send createGroup to signal-cli stdin")?;
        Ok(())
    }

    /// Add members to an existing group.
    pub async fn add_group_members(&self, group_id: &str, members: &[String]) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("updateGroup".to_string(), Instant::now()));
        }
        let params = serde_json::json!({
            "groupId": group_id,
            "members": members,
            "account": self.account,
        });
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "updateGroup".to_string(),
            id,
            params: Some(params),
        };
        let json = serde_json::to_string(&request)?;
        self.stdin_tx.send(json).await.context("Failed to send addGroupMembers to signal-cli stdin")?;
        Ok(())
    }

    /// Remove members from an existing group.
    pub async fn remove_group_members(&self, group_id: &str, members: &[String]) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("updateGroup".to_string(), Instant::now()));
        }
        let params = serde_json::json!({
            "groupId": group_id,
            "removeMembers": members,
            "account": self.account,
        });
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "updateGroup".to_string(),
            id,
            params: Some(params),
        };
        let json = serde_json::to_string(&request)?;
        self.stdin_tx.send(json).await.context("Failed to send removeGroupMembers to signal-cli stdin")?;
        Ok(())
    }

    /// Rename an existing group.
    pub async fn rename_group(&self, group_id: &str, name: &str) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("updateGroup".to_string(), Instant::now()));
        }
        let params = serde_json::json!({
            "groupId": group_id,
            "name": name,
            "account": self.account,
        });
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "updateGroup".to_string(),
            id,
            params: Some(params),
        };
        let json = serde_json::to_string(&request)?;
        self.stdin_tx.send(json).await.context("Failed to send renameGroup to signal-cli stdin")?;
        Ok(())
    }

    /// Update the user's Signal profile.
    pub async fn update_profile(
        &self,
        given_name: &str,
        family_name: &str,
        about: &str,
        about_emoji: &str,
    ) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("updateProfile".to_string(), Instant::now()));
        }
        let params = serde_json::json!({
            "account": self.account,
            "givenName": given_name,
            "familyName": family_name,
            "about": about,
            "aboutEmoji": about_emoji,
        });
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "updateProfile".to_string(),
            id,
            params: Some(params),
        };
        let json = serde_json::to_string(&request)?;
        self.stdin_tx.send(json).await.context("Failed to send updateProfile to signal-cli stdin")?;
        Ok(())
    }

    /// Block a contact or group.
    pub async fn block_contact(&self, recipient: &str, is_group: bool) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("block".to_string(), Instant::now()));
        }
        let params = if is_group {
            serde_json::json!({
                "groupId": [recipient],
                "account": self.account,
            })
        } else {
            serde_json::json!({
                "recipient": [recipient],
                "account": self.account,
            })
        };
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "block".to_string(),
            id,
            params: Some(params),
        };
        let json = serde_json::to_string(&request)?;
        self.stdin_tx.send(json).await.context("Failed to send block to signal-cli stdin")?;
        Ok(())
    }

    /// Unblock a contact or group.
    pub async fn unblock_contact(&self, recipient: &str, is_group: bool) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("unblock".to_string(), Instant::now()));
        }
        let params = if is_group {
            serde_json::json!({
                "groupId": [recipient],
                "account": self.account,
            })
        } else {
            serde_json::json!({
                "recipient": [recipient],
                "account": self.account,
            })
        };
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "unblock".to_string(),
            id,
            params: Some(params),
        };
        let json = serde_json::to_string(&request)?;
        self.stdin_tx.send(json).await.context("Failed to send unblock to signal-cli stdin")?;
        Ok(())
    }

    /// Leave (quit) a group.
    pub async fn quit_group(&self, group_id: &str) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("quitGroup".to_string(), Instant::now()));
        }
        let params = serde_json::json!({
            "groupId": group_id,
            "account": self.account,
        });
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "quitGroup".to_string(),
            id,
            params: Some(params),
        };
        let json = serde_json::to_string(&request)?;
        self.stdin_tx.send(json).await.context("Failed to send quitGroup to signal-cli stdin")?;
        Ok(())
    }

    /// Set the disappearing message timer for a group.
    pub async fn send_update_group_expiration(
        &self,
        group_id: &str,
        seconds: i64,
    ) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("updateGroup".to_string(), Instant::now()));
        }
        let params = serde_json::json!({
            "groupId": group_id,
            "expiration": seconds,
            "account": self.account,
        });
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "updateGroup".to_string(),
            id,
            params: Some(params),
        };
        let json = serde_json::to_string(&request)?;
        self.stdin_tx
            .send(json)
            .await
            .context("Failed to send updateGroup to signal-cli stdin")?;
        Ok(())
    }

    pub async fn send_poll_create(
        &self,
        recipient: &str,
        is_group: bool,
        question: &str,
        options: &[String],
        allow_multiple: bool,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();

        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("sendPollCreate".to_string(), Instant::now()));
        }

        let option_arr: Vec<serde_json::Value> = options
            .iter()
            .map(|o| serde_json::Value::String(o.clone()))
            .collect();

        let mut params = if is_group {
            serde_json::json!({
                "groupId": recipient,
                "question": question,
                "option": option_arr,
                "account": self.account,
            })
        } else {
            serde_json::json!({
                "recipient": [recipient],
                "question": question,
                "option": option_arr,
                "account": self.account,
            })
        };

        if !allow_multiple {
            params["noMulti"] = serde_json::json!(true);
        }

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "sendPollCreate".to_string(),
            id: id.clone(),
            params: Some(params),
        };

        let json = serde_json::to_string(&request)?;
        self.stdin_tx
            .send(json)
            .await
            .context("Failed to send poll create to signal-cli stdin")?;
        Ok(id)
    }

    pub async fn send_poll_vote(
        &self,
        recipient: &str,
        is_group: bool,
        poll_author: &str,
        poll_timestamp: i64,
        options: &[i64],
        vote_count: i64,
    ) -> Result<()> {
        let id = Uuid::new_v4().to_string();

        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("sendPollVote".to_string(), Instant::now()));
        }

        let option_arr: Vec<serde_json::Value> = options
            .iter()
            .map(|&o| serde_json::json!(o))
            .collect();

        let mut params = if is_group {
            serde_json::json!({
                "groupId": recipient,
                "pollAuthor": poll_author,
                "pollTimestamp": poll_timestamp,
                "option": option_arr,
                "account": self.account,
            })
        } else {
            serde_json::json!({
                "recipient": [recipient],
                "pollAuthor": poll_author,
                "pollTimestamp": poll_timestamp,
                "option": option_arr,
                "account": self.account,
            })
        };

        if vote_count != 1 {
            params["voteCount"] = serde_json::json!(vote_count);
        }

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "sendPollVote".to_string(),
            id,
            params: Some(params),
        };

        let json = serde_json::to_string(&request)?;
        self.stdin_tx
            .send(json)
            .await
            .context("Failed to send poll vote to signal-cli stdin")?;
        Ok(())
    }

    pub async fn send_poll_terminate(
        &self,
        recipient: &str,
        is_group: bool,
        poll_timestamp: i64,
    ) -> Result<()> {
        let id = Uuid::new_v4().to_string();

        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), ("sendPollTerminate".to_string(), Instant::now()));
        }

        let params = if is_group {
            serde_json::json!({
                "groupId": recipient,
                "pollTimestamp": poll_timestamp,
                "account": self.account,
            })
        } else {
            serde_json::json!({
                "recipient": [recipient],
                "pollTimestamp": poll_timestamp,
                "account": self.account,
            })
        };

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "sendPollTerminate".to_string(),
            id,
            params: Some(params),
        };

        let json = serde_json::to_string(&request)?;
        self.stdin_tx
            .send(json)
            .await
            .context("Failed to send poll terminate to signal-cli stdin")?;
        Ok(())
    }

    /// Returns accumulated stderr output from the signal-cli process.
    pub fn stderr_output(&self) -> String {
        self.stderr_buffer.lock().map(|buf| buf.clone()).unwrap_or_default()
    }

    /// Non-blocking check: returns `Some(exit_code)` if the child has exited.
    pub fn try_child_exit(&mut self) -> Option<Option<i32>> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.code()),
            _ => None,
        }
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        let _ = self.child.kill().await;
        Ok(())
    }
}

pub fn parse_rpc_result(method: &str, result: &serde_json::Value, rpc_id: Option<&str>) -> Option<SignalEvent> {
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
                    let uuid = obj.get("uuid").and_then(|v| v.as_str()).map(|s| s.to_string());
                    Some(Contact {
                        number: number.to_string(),
                        name,
                        uuid,
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
                            let phone = m.get("number")
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
                    let number = obj.get("number").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let uuid = obj.get("uuid").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let fingerprint = obj.get("fingerprint").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let safety_number = obj.get("safetyNumber").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let trust_level = obj.get("trustLevel").and_then(|v| v.as_str())
                        .map(TrustLevel::from_str)
                        .unwrap_or(TrustLevel::TrustedUnverified);
                    let added_timestamp = obj.get("addedTimestamp").and_then(|v| v.as_i64()).unwrap_or(0);
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
        "sendPollCreate" => {
            let id = rpc_id?.to_string();
            let server_ts = result.get("timestamp").and_then(|v| v.as_i64())
                .or_else(|| result.as_i64())
                .unwrap_or(0);
            Some(SignalEvent::SendTimestamp { rpc_id: id, server_ts })
        }
        "sendReaction" | "remoteDelete" | "sendTypingIndicator" | "sendReceipt" | "updateContact" | "updateGroup" | "quitGroup" | "sendMessageRequestResponse" | "block" | "unblock" | "sendPinMessage" | "sendUnpinMessage" | "sendPollVote" | "sendPollTerminate" | "trust" => None, // fire-and-forget, no action needed
        _ => None,
    }
}

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
        // Safety number change → system message instead of generic error
        let exc_type = exc.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if exc_type == "UntrustedIdentityException" {
            let envelope = params.get("envelope");
            let conv_id = envelope
                .and_then(|e| e.get("sourceNumber"))
                .and_then(|v| v.as_str())
                .or_else(|| exc.get("sender").and_then(|v| v.as_str()))
                .unwrap_or("unknown")
                .to_string();
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
            let kind = if call_type == "VIDEO_CALL" { "video" } else { "voice" };
            let conv_id = envelope
                .get("sourceNumber")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let timestamp_ms = envelope.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
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
                let dest = sent.get("destinationNumber")
                    .or_else(|| sent.get("destination"))
                    .and_then(|v| v.as_str());
                return parse_edit_message(envelope, edit_msg, true, dest);
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
    Some(SignalEvent::ReadSyncReceived { read_messages: entries })
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

    // Check for pin message
    if let Some(pin) = data.get("pinMessage") {
        let target_author = pin.get("targetAuthor").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
        let target_timestamp = pin.get("targetSentTimestamp").and_then(|v| v.as_i64()).unwrap_or(0);
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
        let group_id = data
            .get("groupInfo")
            .and_then(|g| g.get("groupId"))
            .and_then(|v| v.as_str());
        let conv_id = group_id
            .map(|g| g.to_string())
            .unwrap_or_else(|| sender.clone());
        return Some(SignalEvent::PinReceived {
            conv_id,
            sender,
            sender_name,
            target_author,
            target_timestamp,
        });
    }

    // Check for unpin message
    if let Some(unpin) = data.get("unpinMessage") {
        let target_author = unpin.get("targetAuthor").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
        let target_timestamp = unpin.get("targetSentTimestamp").and_then(|v| v.as_i64()).unwrap_or(0);
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
        let group_id = data
            .get("groupInfo")
            .and_then(|g| g.get("groupId"))
            .and_then(|v| v.as_str());
        let conv_id = group_id
            .map(|g| g.to_string())
            .unwrap_or_else(|| sender.clone());
        return Some(SignalEvent::UnpinReceived {
            conv_id,
            sender,
            sender_name,
            target_author,
            target_timestamp,
        });
    }

    // Check for poll messages
    if let Some(poll_create) = data.get("pollCreate") {
        return parse_poll_create(envelope, data, poll_create);
    }
    if let Some(poll_vote) = data.get("pollVote") {
        return parse_poll_vote(envelope, data, poll_vote);
    }
    if let Some(poll_terminate) = data.get("pollTerminate") {
        return parse_poll_terminate(envelope, data, poll_terminate);
    }

    // Check for remote delete
    if let Some(remote_delete) = data.get("remoteDelete") {
        let target_timestamp = remote_delete.get("timestamp").and_then(|v| v.as_i64())?;
        let sender = envelope
            .get("sourceNumber")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let group_id = data
            .get("groupInfo")
            .and_then(|g| g.get("groupId"))
            .and_then(|v| v.as_str());
        let conv_id = group_id
            .map(|g| g.to_string())
            .unwrap_or_else(|| sender.clone());
        return Some(SignalEvent::RemoteDeleteReceived {
            conv_id,
            sender,
            target_timestamp,
        });
    }

    // Expiration timer update → ExpirationTimerChanged event
    if data.get("isExpirationUpdate").and_then(|v| v.as_bool()).unwrap_or(false) {
        let group_id = data.get("groupInfo").and_then(|g| g.get("groupId")).and_then(|v| v.as_str());
        let conv_id = group_id
            .map(|g| g.to_string())
            .unwrap_or_else(|| {
                envelope.get("sourceNumber").and_then(|v| v.as_str()).unwrap_or("unknown").to_string()
            });
        let seconds = data.get("expiresInSeconds").and_then(|v| v.as_i64()).unwrap_or(0);
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

    // Group update with no body/reaction/remoteDelete → system message
    if let Some(group_info) = data.get("groupInfo") {
        let group_type = group_info.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if group_type == "UPDATE"
            && data.get("message").and_then(|v| v.as_str()).is_none()
            && data.get("reaction").is_none()
            && data.get("remoteDelete").is_none()
        {
            let conv_id = group_info.get("groupId").and_then(|v| v.as_str())
                .unwrap_or("unknown").to_string();
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

    // Synthesize body for sticker messages
    let sticker_body = data.get("sticker").map(|sticker| {
        match sticker.get("emoji").and_then(|v| v.as_str()) {
            Some(emoji) => format!("[Sticker: {}]", emoji),
            None => "[Sticker]".to_string(),
        }
    });

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

    let mut attachments = data
        .get("attachments")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| parse_attachment(a, download_dir))
                .collect()
        })
        .unwrap_or_default();

    let mut previews = parse_link_previews(data, download_dir);

    // View-once messages: replace content with placeholder
    if data.get("viewOnce").and_then(|v| v.as_bool()).unwrap_or(false) {
        body = Some("[View-once message]".to_string());
        attachments = Vec::new();
        previews = Vec::new();
    }

    let mentions = parse_mentions(data);

    let text_styles = parse_text_styles(data);

    // Parse quoted reply (strip U+FFFC mention placeholders from quote text)
    let quote = data.get("quote").and_then(|q| {
        let q_ts = q.get("id").and_then(|v| v.as_i64())?;
        let q_author = q.get("authorNumber").and_then(|v| v.as_str())?.to_string();
        let q_body = q.get("text").and_then(|v| v.as_str()).unwrap_or("")
            .replace('\u{FFFC}', "").to_string();
        Some((q_ts, q_author, q_body))
    });

    let expires_in_seconds = data.get("expiresInSeconds").and_then(|v| v.as_i64()).unwrap_or(0);

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
        mentions,
        text_styles,
        quote,
        expires_in_seconds,
        previews,
    }))
}

fn parse_poll_create(
    envelope: &serde_json::Value,
    data: &serde_json::Value,
    poll_create: &serde_json::Value,
) -> Option<SignalEvent> {
    let question = poll_create.get("question").and_then(|v| v.as_str())?.to_string();
    let allow_multiple = poll_create.get("allowMultiple").and_then(|v| v.as_bool()).unwrap_or(false);
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

    let group_id = data.get("groupInfo").and_then(|g| g.get("groupId")).and_then(|v| v.as_str());
    let sender = envelope.get("sourceNumber").and_then(|v| v.as_str()).unwrap_or("unknown");
    let conv_id = group_id.unwrap_or(sender).to_string();
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

fn parse_poll_vote(
    envelope: &serde_json::Value,
    data: &serde_json::Value,
    poll_vote: &serde_json::Value,
) -> Option<SignalEvent> {
    let target_timestamp = poll_vote.get("targetSentTimestamp").and_then(|v| v.as_i64())?;
    let voter = poll_vote
        .get("authorNumber")
        .and_then(|v| v.as_str())
        .or_else(|| envelope.get("sourceNumber").and_then(|v| v.as_str()))
        .unwrap_or("unknown")
        .to_string();
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
    let vote_count = poll_vote.get("voteCount").and_then(|v| v.as_i64()).unwrap_or(1);

    let group_id = data.get("groupInfo").and_then(|g| g.get("groupId")).and_then(|v| v.as_str());
    let sender = envelope.get("sourceNumber").and_then(|v| v.as_str()).unwrap_or("unknown");
    let conv_id = group_id.unwrap_or(sender).to_string();

    Some(SignalEvent::PollVoteReceived {
        conv_id,
        target_timestamp,
        voter,
        voter_name,
        option_indexes,
        vote_count,
    })
}

fn parse_poll_terminate(
    envelope: &serde_json::Value,
    data: &serde_json::Value,
    poll_terminate: &serde_json::Value,
) -> Option<SignalEvent> {
    let target_timestamp = poll_terminate.get("targetSentTimestamp").and_then(|v| v.as_i64())?;
    let group_id = data.get("groupInfo").and_then(|g| g.get("groupId")).and_then(|v| v.as_str());
    let sender = envelope.get("sourceNumber").and_then(|v| v.as_str()).unwrap_or("unknown");
    let conv_id = group_id.unwrap_or(sender).to_string();

    Some(SignalEvent::PollTerminated {
        conv_id,
        target_timestamp,
    })
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

    // Check for synced poll messages
    if let Some(poll_create) = sent.get("pollCreate") {
        return parse_poll_create(envelope, sent, poll_create);
    }
    if let Some(poll_vote) = sent.get("pollVote") {
        return parse_poll_vote(envelope, sent, poll_vote);
    }
    if let Some(poll_terminate) = sent.get("pollTerminate") {
        return parse_poll_terminate(envelope, sent, poll_terminate);
    }

    // Check for synced pin message
    if let Some(pin) = sent.get("pinMessage") {
        let target_author = pin.get("targetAuthor").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
        let target_timestamp = pin.get("targetSentTimestamp").and_then(|v| v.as_i64()).unwrap_or(0);
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
        return Some(SignalEvent::PinReceived {
            conv_id,
            sender,
            sender_name,
            target_author,
            target_timestamp,
        });
    }

    // Check for synced unpin message
    if let Some(unpin) = sent.get("unpinMessage") {
        let target_author = unpin.get("targetAuthor").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
        let target_timestamp = unpin.get("targetSentTimestamp").and_then(|v| v.as_i64()).unwrap_or(0);
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
        return Some(SignalEvent::UnpinReceived {
            conv_id,
            sender,
            sender_name,
            target_author,
            target_timestamp,
        });
    }

    // Check for synced remote delete
    if let Some(remote_delete) = sent.get("remoteDelete") {
        let target_timestamp = remote_delete.get("timestamp").and_then(|v| v.as_i64())?;
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
        return Some(SignalEvent::RemoteDeleteReceived {
            conv_id,
            sender,
            target_timestamp,
        });
    }

    // Expiration timer update (synced) → ExpirationTimerChanged event
    if sent.get("isExpirationUpdate").and_then(|v| v.as_bool()).unwrap_or(false) {
        let group_id = sent.get("groupInfo").and_then(|g| g.get("groupId")).and_then(|v| v.as_str());
        let conv_id = group_id
            .map(|g| g.to_string())
            .or_else(|| {
                sent.get("destinationNumber")
                    .or_else(|| sent.get("destination"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| {
                envelope.get("sourceNumber").and_then(|v| v.as_str()).unwrap_or("unknown").to_string()
            });
        let seconds = sent.get("expiresInSeconds").and_then(|v| v.as_i64()).unwrap_or(0);
        let timestamp_ms = sent.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
        let timestamp = DateTime::from_timestamp_millis(timestamp_ms).unwrap_or_default();
        return Some(SignalEvent::ExpirationTimerChanged {
            conv_id,
            seconds,
            body: format_expiration(seconds),
            timestamp,
            timestamp_ms,
        });
    }

    // Group update (synced) with no body/reaction/remoteDelete → system message
    if let Some(group_info) = sent.get("groupInfo") {
        let group_type = group_info.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if group_type == "UPDATE"
            && sent.get("message").and_then(|v| v.as_str()).is_none()
            && sent.get("reaction").is_none()
            && sent.get("remoteDelete").is_none()
        {
            let conv_id = group_info.get("groupId").and_then(|v| v.as_str())
                .unwrap_or("unknown").to_string();
            let timestamp_ms = sent.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
            let timestamp = DateTime::from_timestamp_millis(timestamp_ms).unwrap_or_default();
            return Some(SignalEvent::SystemMessage {
                conv_id,
                body: "Group updated".to_string(),
                timestamp,
                timestamp_ms,
            });
        }
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

    // Synthesize body for sticker messages
    let sticker_body = sent.get("sticker").map(|sticker| {
        match sticker.get("emoji").and_then(|v| v.as_str()) {
            Some(emoji) => format!("[Sticker: {}]", emoji),
            None => "[Sticker]".to_string(),
        }
    });

    let mut body = sent
        .get("message")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or(sticker_body);

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

    let mut attachments = sent
        .get("attachments")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| parse_attachment(a, download_dir))
                .collect()
        })
        .unwrap_or_default();

    let mut previews = parse_link_previews(sent, download_dir);

    // View-once messages: replace content with placeholder
    if sent.get("viewOnce").and_then(|v| v.as_bool()).unwrap_or(false) {
        body = Some("[View-once message]".to_string());
        attachments = Vec::new();
        previews = Vec::new();
    }

    let mentions = parse_mentions(sent);

    let text_styles = parse_text_styles(sent);

    // Parse quoted reply (strip U+FFFC mention placeholders from quote text)
    let quote = sent.get("quote").and_then(|q| {
        let q_ts = q.get("id").and_then(|v| v.as_i64())?;
        let q_author = q.get("authorNumber").and_then(|v| v.as_str())?.to_string();
        let q_body = q.get("text").and_then(|v| v.as_str()).unwrap_or("")
            .replace('\u{FFFC}', "").to_string();
        Some((q_ts, q_author, q_body))
    });

    let expires_in_seconds = sent.get("expiresInSeconds").and_then(|v| v.as_i64()).unwrap_or(0);

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
        mentions,
        text_styles,
        quote,
        expires_in_seconds,
        previews,
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

fn parse_edit_message(
    envelope: &serde_json::Value,
    edit_msg: &serde_json::Value,
    is_outgoing: bool,
    destination: Option<&str>,
) -> Option<SignalEvent> {
    let target_timestamp = edit_msg.get("targetSentTimestamp").and_then(|v| v.as_i64())?;
    let data = edit_msg.get("dataMessage")?;
    let new_body = data.get("message").and_then(|v| v.as_str())?.to_string();
    let new_timestamp = data.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);

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

    let group_id = data
        .get("groupInfo")
        .and_then(|g| g.get("groupId"))
        .and_then(|v| v.as_str());

    let conv_id = group_id
        .map(|g| g.to_string())
        .or_else(|| {
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

    // Sanitize filename: strip path separators and traversal sequences
    // to prevent writes outside the download directory.
    effective_name = effective_name
        .replace(['/', '\\'], "_")
        .replace("..", "_");
    if effective_name.is_empty() {
        let short_id = if id.len() > 8 { &id[id.len() - 8..] } else { &id };
        effective_name = format!("{short_id}.bin");
    }

    let dest = download_dir.join(&effective_name);

    // Defense-in-depth: verify resolved path stays within download directory.
    let canon_dir = download_dir.canonicalize().unwrap_or_else(|_| download_dir.to_path_buf());
    let canon_dest = dest.canonicalize().unwrap_or_else(|_| canon_dir.join(&effective_name));
    if !canon_dest.starts_with(&canon_dir) {
        return None;
    }

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
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(download_dir, std::fs::Permissions::from_mode(0o700));
            }
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

/// Parse link previews from a dataMessage / sentMessage object.
fn parse_link_previews(
    data: &serde_json::Value,
    download_dir: &std::path::Path,
) -> Vec<LinkPreview> {
    // signal-cli uses "previews" (plural) in some versions, "preview" in others
    let arr = data
        .get("previews")
        .or_else(|| data.get("preview"))
        .and_then(|v| v.as_array());
    let Some(arr) = arr else { return Vec::new() };
    arr.iter()
        .filter_map(|p| {
            let url = p.get("url").and_then(|v| v.as_str())?.to_string();
            let title = p.get("title").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
            let description = p.get("description").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
            let image_path = p.get("image")
                .and_then(|img| parse_attachment(img, download_dir))
                .and_then(|att| att.local_path);
            Some(LinkPreview { url, title, description, image_path })
        })
        .collect()
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

/// Format an expiration timer value as a human-readable string.
fn format_expiration(seconds: i64) -> String {
    if seconds == 0 {
        return "Disappearing messages disabled".to_string();
    }
    let (n, unit) = if seconds < 60 {
        (seconds, "second")
    } else if seconds < 3600 {
        (seconds / 60, "minute")
    } else if seconds < 86400 {
        (seconds / 3600, "hour")
    } else if seconds < 604800 {
        (seconds / 86400, "day")
    } else {
        (seconds / 604800, "week")
    };
    let plural = if n == 1 { "" } else { "s" };
    format!("Disappearing messages set to {n} {unit}{plural}")
}

/// Parse mentions from a data/sync message.
/// signal-cli uses "mentions" array with "uuid" field; fall back to legacy "bodyRanges" with "mentionUuid".
fn parse_mentions(data: &serde_json::Value) -> Vec<Mention> {
    let arr = data
        .get("mentions")
        .and_then(|v| v.as_array())
        .or_else(|| data.get("bodyRanges").and_then(|v| v.as_array()));

    arr.map(|items| {
        items
            .iter()
            .filter_map(|r| {
                let start = r.get("start").and_then(|v| v.as_u64())? as usize;
                let length = r.get("length").and_then(|v| v.as_u64())? as usize;
                let uuid = r
                    .get("uuid")
                    .or_else(|| r.get("mentionUuid"))
                    .and_then(|v| v.as_str())?
                    .to_string();
                Some(Mention { start, length, uuid })
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Parse text styles from a data message's textStyles array (or bodyRanges style entries).
fn parse_text_styles(data: &serde_json::Value) -> Vec<TextStyle> {
    // Try textStyles array first, then fall back to bodyRanges entries with "style" field
    let arr = data
        .get("textStyles")
        .and_then(|v| v.as_array())
        .or_else(|| data.get("bodyRanges").and_then(|v| v.as_array()));

    arr.map(|items| {
        items
            .iter()
            .filter_map(|r| {
                let start = r.get("start").and_then(|v| v.as_u64())? as usize;
                let length = r.get("length").and_then(|v| v.as_u64())? as usize;
                let style_str = r.get("style").and_then(|v| v.as_str())?;
                let style = match style_str {
                    "BOLD" => StyleType::Bold,
                    "ITALIC" => StyleType::Italic,
                    "STRIKETHROUGH" => StyleType::Strikethrough,
                    "MONOSPACE" => StyleType::Monospace,
                    "SPOILER" => StyleType::Spoiler,
                    _ => return None,
                };
                Some(TextStyle { start, length, style })
            })
            .collect()
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
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
                assert_eq!(groups[0].member_uuids, vec![
                    ("+1".to_string(), "uuid-1".to_string()),
                    ("+2".to_string(), "uuid-2".to_string()),
                ]);
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
            SignalEvent::ReceiptReceived { sender, receipt_type, timestamps } => {
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
            SignalEvent::ExpirationTimerChanged { conv_id, seconds, body, .. } => {
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
                assert_eq!(read_messages[0], ("+15551234567".to_string(), 1700000000001));
                assert_eq!(read_messages[1], ("+15559876543".to_string(), 1700000000002));
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
            SignalEvent::PollCreated { conv_id, timestamp, poll_data } => {
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
            SignalEvent::PollVoteReceived { conv_id, target_timestamp, voter, option_indexes, vote_count, .. } => {
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
            SignalEvent::PollTerminated { conv_id, target_timestamp } => {
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
        assert_eq!(previews[0].description.as_deref(), Some("An interesting article"));
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
}
