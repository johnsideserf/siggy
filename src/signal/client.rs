//! signal-cli child process bridge over JSON-RPC.
//!
//! [`SignalClient`] spawns signal-cli and runs two tokio tasks: a stdout
//! reader that parses JSON-RPC frames into [`SignalEvent`]s, and a stdin
//! writer that sends [`JsonRpcRequest`]s. The `pending_requests` map
//! correlates response IDs with the originating method so the reader can
//! emit the right event variant. Notifications (incoming messages, typing,
//! receipts) and RPC results both flow through the same mpsc channel.

use anyhow::{Context, Result};
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

use super::parse::{parse_rpc_result, parse_signal_event};
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
                                crate::debug_log::logf(format_args!(
                                    "rpc error: method={method} error={err:?}"
                                ));
                                // RPC error — emit SendFailed for send requests,
                                // surface other errors to the status bar
                                if method == "send" {
                                    rpc_id.map(|id| SignalEvent::SendFailed { rpc_id: id })
                                } else {
                                    Some(SignalEvent::Error(format!("{method}: {}", err.message)))
                                }
                            } else {
                                resp.result.as_ref().and_then(|result| {
                                    parse_rpc_result(&method, result, rpc_id.as_deref())
                                })
                            }
                        } else {
                            parse_signal_event(&resp, &download_dir)
                        };

                        if let Some(ref event) = event {
                            if crate::debug_log::redact() {
                                crate::debug_log::logf(format_args!(
                                    "event: {}",
                                    event.redacted_summary()
                                ));
                            } else {
                                crate::debug_log::logf(format_args!("event: {event:?}"));
                            }
                        }

                        if let Some(event) = event
                            && event_tx.send(event).await.is_err()
                        {
                            break;
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

    /// Set the target field on params for recipient/groupId based on is_group.
    /// The non-group case wraps recipient in a single-element array because that's
    /// the shape signal-cli expects for almost every RPC that takes one. The few
    /// special cases (sendReaction wants a bare string; block/unblock want a single
    /// groupId in an array) build their target field by hand.
    fn set_target(params: &mut serde_json::Value, recipient: &str, is_group: bool) {
        if is_group {
            params["groupId"] = serde_json::Value::String(recipient.to_string());
        } else {
            params["recipient"] = serde_json::json!([recipient]);
        }
    }

    /// Build the JSON-RPC envelope, register the rpc id with `method` so the stdout
    /// reader can correlate the response, and send to signal-cli's stdin. Returns
    /// the rpc id so callers that need to track the send (send_message,
    /// send_edit_message) can correlate the result.
    async fn send_rpc(&self, method: &str, params: serde_json::Value) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        if let Ok(mut map) = self.pending_requests.lock() {
            map.insert(id.clone(), (method.to_string(), Instant::now()));
        }
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
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

    pub async fn send_message(
        &self,
        recipient: &str,
        body: &str,
        is_group: bool,
        mentions: &[(usize, String)],
        attachments: &[&Path],
        quote: Option<(&str, i64, &str)>,
    ) -> Result<String> {
        let mut params = serde_json::json!({
            "message": body,
            "account": self.account,
        });
        Self::set_target(&mut params, recipient, is_group);

        if !mentions.is_empty() {
            // signal-cli expects mentions as colon-separated strings: "start:length:uuid"
            let mention_arr: Vec<serde_json::Value> = mentions
                .iter()
                .map(|(start, uuid)| serde_json::Value::String(format!("{start}:1:{uuid}")))
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

        let id = self.send_rpc("send", params).await?;
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
        let mut params = serde_json::json!({
            "message": body,
            "account": self.account,
            "editTimestamp": edit_timestamp,
        });
        Self::set_target(&mut params, recipient, is_group);

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

        let id = self.send_rpc("send", params).await?;
        Ok(id)
    }

    pub async fn send_remote_delete(
        &self,
        recipient: &str,
        is_group: bool,
        target_timestamp: i64,
    ) -> Result<()> {
        let mut params = serde_json::json!({
            "targetTimestamp": target_timestamp,
            "account": self.account,
        });
        Self::set_target(&mut params, recipient, is_group);
        self.send_rpc("remoteDelete", params).await?;
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
        let mut params = serde_json::json!({
            "targetAuthor": target_author,
            "targetTimestamp": target_timestamp,
            "pinDuration": pin_duration,
            "account": self.account,
        });
        Self::set_target(&mut params, recipient, is_group);
        self.send_rpc("sendPinMessage", params).await?;
        Ok(())
    }

    pub async fn send_unpin_message(
        &self,
        recipient: &str,
        is_group: bool,
        target_author: &str,
        target_timestamp: i64,
    ) -> Result<()> {
        let mut params = serde_json::json!({
            "targetAuthor": target_author,
            "targetTimestamp": target_timestamp,
            "pinDuration": -1,
            "account": self.account,
        });
        Self::set_target(&mut params, recipient, is_group);
        self.send_rpc("sendUnpinMessage", params).await?;
        Ok(())
    }

    pub async fn list_groups(&self) -> Result<()> {
        self.send_rpc("listGroups", serde_json::json!({ "account": self.account }))
            .await?;
        Ok(())
    }

    pub async fn list_contacts(&self) -> Result<()> {
        self.send_rpc(
            "listContacts",
            serde_json::json!({ "account": self.account }),
        )
        .await?;
        Ok(())
    }

    pub async fn list_identities(&self) -> Result<()> {
        self.send_rpc(
            "listIdentities",
            serde_json::json!({ "account": self.account }),
        )
        .await?;
        Ok(())
    }

    pub async fn trust_identity(&self, recipient: &str, safety_number: &str) -> Result<()> {
        let params = serde_json::json!({
            "recipient": [recipient],
            "verifiedSafetyNumber": safety_number,
            "account": self.account,
        });
        self.send_rpc("trust", params).await?;
        Ok(())
    }

    /// Note: previously this method did not register in pending_requests. After this
    /// refactor it goes through send_rpc and will be registered. The entry will be
    /// swept by the existing TTL cleanup if signal-cli never sends a correlated
    /// response, and the parser falls through to default for unknown methods.
    pub async fn send_sync_request(&self) -> Result<()> {
        self.send_rpc(
            "sendSyncRequest",
            serde_json::json!({ "account": self.account }),
        )
        .await?;
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
        // sendReaction uses recipient as a bare string (not wrapped in array) for 1:1.
        // Do not use set_target here — build inline to preserve the bare-string shape.
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

        self.send_rpc("sendReaction", params).await?;
        Ok(())
    }

    pub async fn send_typing(&self, recipient: &str, is_group: bool, stop: bool) -> Result<()> {
        let mut params = serde_json::json!({ "account": self.account });
        Self::set_target(&mut params, recipient, is_group);
        if stop {
            params["stop"] = serde_json::json!(true);
        }
        self.send_rpc("sendTypingIndicator", params).await?;
        Ok(())
    }

    /// Send a read receipt to a single recipient for one or more message timestamps.
    /// Fire-and-forget — no useful result is expected from signal-cli.
    pub async fn send_read_receipt(&self, recipient: &str, timestamps: &[i64]) -> Result<()> {
        // target is a single recipient, no is_group branch — inline to preserve shape.
        let params = serde_json::json!({
            "recipient": [recipient],
            "type": "read",
            "targetTimestamp": timestamps,
            "account": self.account,
        });
        self.send_rpc("sendReceipt", params).await?;
        Ok(())
    }

    /// Accept or delete a message request.
    pub async fn send_message_request_response(
        &self,
        recipient: &str,
        is_group: bool,
        response_type: &str,
    ) -> Result<()> {
        let mut params = serde_json::json!({
            "type": response_type,
            "account": self.account,
        });
        Self::set_target(&mut params, recipient, is_group);
        self.send_rpc("sendMessageRequestResponse", params).await?;
        Ok(())
    }

    /// Set the disappearing message timer for a 1:1 contact.
    pub async fn send_update_contact_expiration(
        &self,
        recipient: &str,
        seconds: i64,
    ) -> Result<()> {
        // updateContact uses recipient as a bare string (not array) — inline to preserve shape.
        let params = serde_json::json!({
            "recipient": recipient,
            "expiration": seconds,
            "account": self.account,
        });
        self.send_rpc("updateContact", params).await?;
        Ok(())
    }

    /// Create a new group with the given name (optionally with initial members).
    pub async fn create_group(&self, name: &str, members: &[String]) -> Result<()> {
        let mut params = serde_json::json!({
            "name": name,
            "account": self.account,
        });
        if !members.is_empty() {
            params["members"] = serde_json::json!(members);
        }
        self.send_rpc("updateGroup", params).await?;
        Ok(())
    }

    /// Add members to an existing group.
    pub async fn add_group_members(&self, group_id: &str, members: &[String]) -> Result<()> {
        let params = serde_json::json!({
            "groupId": group_id,
            "members": members,
            "account": self.account,
        });
        self.send_rpc("updateGroup", params).await?;
        Ok(())
    }

    /// Remove members from an existing group.
    pub async fn remove_group_members(&self, group_id: &str, members: &[String]) -> Result<()> {
        let params = serde_json::json!({
            "groupId": group_id,
            "removeMembers": members,
            "account": self.account,
        });
        self.send_rpc("updateGroup", params).await?;
        Ok(())
    }

    /// Rename an existing group.
    pub async fn rename_group(&self, group_id: &str, name: &str) -> Result<()> {
        let params = serde_json::json!({
            "groupId": group_id,
            "name": name,
            "account": self.account,
        });
        self.send_rpc("updateGroup", params).await?;
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
        let params = serde_json::json!({
            "account": self.account,
            "givenName": given_name,
            "familyName": family_name,
            "about": about,
            "aboutEmoji": about_emoji,
        });
        self.send_rpc("updateProfile", params).await?;
        Ok(())
    }

    /// Block a contact or group.
    pub async fn block_contact(&self, recipient: &str, is_group: bool) -> Result<()> {
        // block/unblock wrap groupId in a single-element array (not a bare string).
        // Do not use set_target — build inline to preserve that shape.
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
        self.send_rpc("block", params).await?;
        Ok(())
    }

    /// Unblock a contact or group.
    pub async fn unblock_contact(&self, recipient: &str, is_group: bool) -> Result<()> {
        // block/unblock wrap groupId in a single-element array (not a bare string).
        // Do not use set_target — build inline to preserve that shape.
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
        self.send_rpc("unblock", params).await?;
        Ok(())
    }

    /// Leave (quit) a group.
    pub async fn quit_group(&self, group_id: &str) -> Result<()> {
        let params = serde_json::json!({
            "groupId": group_id,
            "account": self.account,
        });
        self.send_rpc("quitGroup", params).await?;
        Ok(())
    }

    /// Set the disappearing message timer for a group.
    pub async fn send_update_group_expiration(&self, group_id: &str, seconds: i64) -> Result<()> {
        let params = serde_json::json!({
            "groupId": group_id,
            "expiration": seconds,
            "account": self.account,
        });
        self.send_rpc("updateGroup", params).await?;
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
        let option_arr: Vec<serde_json::Value> = options
            .iter()
            .map(|o| serde_json::Value::String(o.clone()))
            .collect();

        let mut params = serde_json::json!({
            "question": question,
            "option": option_arr,
            "account": self.account,
        });
        Self::set_target(&mut params, recipient, is_group);

        if !allow_multiple {
            params["noMulti"] = serde_json::json!(true);
        }

        let id = self.send_rpc("sendPollCreate", params).await?;
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
        let option_arr: Vec<serde_json::Value> =
            options.iter().map(|&o| serde_json::json!(o)).collect();

        let mut params = serde_json::json!({
            "pollAuthor": poll_author,
            "pollTimestamp": poll_timestamp,
            "option": option_arr,
            "account": self.account,
        });
        Self::set_target(&mut params, recipient, is_group);

        if vote_count != 1 {
            params["voteCount"] = serde_json::json!(vote_count);
        }

        self.send_rpc("sendPollVote", params).await?;
        Ok(())
    }

    pub async fn send_poll_terminate(
        &self,
        recipient: &str,
        is_group: bool,
        poll_timestamp: i64,
    ) -> Result<()> {
        let mut params = serde_json::json!({
            "pollTimestamp": poll_timestamp,
            "account": self.account,
        });
        Self::set_target(&mut params, recipient, is_group);
        self.send_rpc("sendPollTerminate", params).await?;
        Ok(())
    }

    /// Returns accumulated stderr output from the signal-cli process.
    pub fn stderr_output(&self) -> String {
        self.stderr_buffer
            .lock()
            .map(|buf| buf.clone())
            .unwrap_or_default()
    }

    /// Non-blocking check: returns `Some(exit_code)` if the child has exited.
    pub fn try_child_exit(&mut self) -> Option<Option<i32>> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.code()),
            _ => None,
        }
    }

    /// Wait up to `timeout` for signal-cli to either stay alive (ready) or exit early
    /// (likely unregistered). Returns `true` if the process is still running, `false`
    /// if it exited during the window.
    pub async fn wait_for_ready(&mut self, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if self.try_child_exit().is_some() {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        true
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        let _ = self.child.kill().await;
        Ok(())
    }
}
