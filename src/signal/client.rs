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

// Under the native feature the signal-cli adapter (this impl's main caller)
// is compiled out, but SignalClient itself stays: the one-shot CLI modes
// (--receive/--send/--watch) still drive it directly under either feature
// until U12/U13 migrate them through the boundary. Allow the orphaned
// methods rather than cfg-ing them one by one; drop with that migration.
#[cfg_attr(feature = "native-backend", allow(dead_code))]
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
                let Some(event) = handle_stdout_line(&line, &pending_clone, &download_dir) else {
                    continue;
                };

                if crate::debug_log::redact() {
                    crate::debug_log::logf(format_args!("event: {}", event.redacted_summary()));
                } else {
                    crate::debug_log::logf(format_args!("event: {event:?}"));
                }

                if event_tx.send(event).await.is_err() {
                    return;
                }
            }

            // stdout hit EOF: signal-cli exited. Fail any in-flight sends so their
            // local messages leave the Sending state, then emit an explicit
            // Disconnected before the channel closes (#497).
            for event in drain_pending_as_failures(&pending_clone) {
                if event_tx.send(event).await.is_err() {
                    return;
                }
            }
            let _ = event_tx.send(SignalEvent::Disconnected).await;
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

    /// Build the JSON-RPC envelope, send to signal-cli's stdin, and register the
    /// rpc id with `method` so the stdout reader can correlate the response.
    /// Returns the rpc id so callers that need to track the send (send_message,
    /// send_edit_message) can correlate the result.
    async fn send_rpc(&self, method: &str, params: serde_json::Value) -> Result<String> {
        send_rpc_impl(&self.stdin_tx, &self.pending_requests, method, params).await
    }

    /// Add a textStyle param with signal-cli's "start:length:STYLE" strings
    /// (UTF-16 offsets). No-op when there are no style ranges.
    fn set_text_styles(params: &mut serde_json::Value, text_styles: &[(usize, usize, StyleType)]) {
        if text_styles.is_empty() {
            return;
        }
        let arr: Vec<serde_json::Value> = text_styles
            .iter()
            .map(|(start, len, style)| {
                serde_json::Value::String(format!("{start}:{len}:{}", style.wire_name()))
            })
            .collect();
        params["textStyle"] = serde_json::Value::Array(arr);
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_message(
        &self,
        recipient: &str,
        body: &str,
        is_group: bool,
        mentions: &[(usize, String)],
        text_styles: &[(usize, usize, StyleType)],
        attachments: &[&Path],
        preview: Option<&LinkPreview>,
        quote: Option<(&str, i64, &str)>,
    ) -> Result<SendToken> {
        let mut params = serde_json::json!({
            "message": body,
            "account": self.account,
        });
        Self::set_target(&mut params, recipient, is_group);
        Self::set_text_styles(&mut params, text_styles);

        // Sender-generated link preview (#267). The URL must also appear in
        // the message body; the composer guarantees that.
        if let Some(p) = preview {
            params["previewUrl"] = serde_json::json!(p.url);
            if let Some(ref title) = p.title {
                params["previewTitle"] = serde_json::json!(title);
            }
            if let Some(ref description) = p.description {
                params["previewDescription"] = serde_json::json!(description);
            }
            if let Some(ref image_path) = p.image_path {
                params["previewImage"] = serde_json::json!(image_path);
            }
        }

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
        Ok(SendToken::new(id))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_edit_message(
        &self,
        recipient: &str,
        body: &str,
        is_group: bool,
        edit_timestamp: i64,
        mentions: &[(usize, String)],
        text_styles: &[(usize, usize, StyleType)],
        quote: Option<(&str, i64, &str)>,
    ) -> Result<SendToken> {
        let mut params = serde_json::json!({
            "message": body,
            "account": self.account,
            "editTimestamp": edit_timestamp,
        });
        Self::set_target(&mut params, recipient, is_group);
        Self::set_text_styles(&mut params, text_styles);

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
        Ok(SendToken::new(id))
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

    /// Resolve a Signal username (`name.123`, no `@`/`u:` prefix) to its
    /// account uuid and registration status (#612). The correlated response
    /// parses into [`SignalEvent::UserStatusList`].
    pub async fn get_user_status(&self, username: &str) -> Result<()> {
        self.send_rpc(
            "getUserStatus",
            serde_json::json!({
                "account": self.account,
                "username": [username],
            }),
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
        let params = build_send_reaction_params(
            &self.account,
            recipient,
            is_group,
            emoji,
            target_author,
            target_timestamp,
            remove,
        );
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
        let params = build_send_read_receipt_params(&self.account, recipient, timestamps);
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
        let params = build_update_contact_expiration_params(&self.account, recipient, seconds);
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
        let params = build_block_params(&self.account, recipient, is_group);
        self.send_rpc("block", params).await?;
        Ok(())
    }

    /// Unblock a contact or group.
    pub async fn unblock_contact(&self, recipient: &str, is_group: bool) -> Result<()> {
        let params = build_block_params(&self.account, recipient, is_group);
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
    ) -> Result<SendToken> {
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
        Ok(SendToken::new(id))
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

/// Build the params for `sendReaction`. Note: `recipient` is a bare string
/// (not wrapped in an array) for 1:1, unlike most other send_* RPCs.
/// signal-cli rejects the array form here.
fn build_send_reaction_params(
    account: &str,
    recipient: &str,
    is_group: bool,
    emoji: &str,
    target_author: &str,
    target_timestamp: i64,
    remove: bool,
) -> serde_json::Value {
    let mut params = if is_group {
        serde_json::json!({
            "groupId": recipient,
            "emoji": emoji,
            "targetAuthor": target_author,
            "targetTimestamp": target_timestamp,
            "account": account,
        })
    } else {
        serde_json::json!({
            "recipient": recipient,
            "emoji": emoji,
            "targetAuthor": target_author,
            "targetTimestamp": target_timestamp,
            "account": account,
        })
    };
    if remove {
        params["remove"] = serde_json::json!(true);
    }
    params
}

/// Build the params for `sendReceipt` (read receipts). `recipient` is wrapped
/// in a single-element array; `targetTimestamp` is the array of message
/// timestamps being acknowledged.
fn build_send_read_receipt_params(
    account: &str,
    recipient: &str,
    timestamps: &[i64],
) -> serde_json::Value {
    serde_json::json!({
        "recipient": [recipient],
        "type": "read",
        "targetTimestamp": timestamps,
        "account": account,
    })
}

/// Build the params for `updateContact` (1:1 disappearing-message timer).
/// `recipient` is a bare string (not array), unlike most other send_* RPCs.
fn build_update_contact_expiration_params(
    account: &str,
    recipient: &str,
    seconds: i64,
) -> serde_json::Value {
    serde_json::json!({
        "recipient": recipient,
        "expiration": seconds,
        "account": account,
    })
}

/// Build the params for `block` and `unblock`. Both wrap the identifier in
/// a single-element array (`groupId` for groups, `recipient` for contacts),
/// unlike `sendReaction` and `updateContact` which use bare strings.
fn build_block_params(account: &str, recipient: &str, is_group: bool) -> serde_json::Value {
    if is_group {
        serde_json::json!({
            "groupId": [recipient],
            "account": account,
        })
    } else {
        serde_json::json!({
            "recipient": [recipient],
            "account": account,
        })
    }
}

/// Drain `pending` and turn every tracked send method (`send` / `sendPollCreate`,
/// the ones dispatch_send registers) into a `SendFailed` event. Called when the
/// stdout reader exits so in-flight sends do not hang in the Sending state
/// forever after the child dies (#497). Recovers from a poisoned lock (REL-002).
fn drain_pending_as_failures(
    pending: &Mutex<HashMap<String, (String, Instant)>>,
) -> Vec<SignalEvent> {
    let mut map = pending.lock().unwrap_or_else(|e| e.into_inner());
    map.drain()
        .filter(|(_, (method, _))| method == "send" || method == "sendPollCreate")
        .map(|(id, _)| SignalEvent::SendFailed {
            token: SendToken::new(id),
        })
        .collect()
}

/// Parse one JSON-RPC line from signal-cli's stdout into an optional event.
///
/// Pure given the shared `pending` correlation map and `download_dir`, so it can
/// be unit-tested without spawning signal-cli. Returns:
/// - `None` for blank lines and for notifications/results that produce no event;
/// - `Some(SignalEvent::Error(..))` for malformed JSON (never panics);
/// - the correlated or parsed event otherwise.
///
/// A correlated RPC error on a tracked send method (`send` / `sendPollCreate`)
/// becomes `SendFailed` so the local message can leave the Sending state; other
/// RPC errors surface to the status bar (#486). Correlation recovers from a
/// poisoned `pending` lock instead of dropping the match, mirroring the
/// send-side REL-002 fix so a panic elsewhere can't strand in-flight sends.
fn handle_stdout_line(
    line: &str,
    pending: &Mutex<HashMap<String, (String, Instant)>>,
    download_dir: &Path,
) -> Option<SignalEvent> {
    if line.trim().is_empty() {
        return None;
    }

    let resp = match serde_json::from_str::<JsonRpcResponse>(line) {
        Ok(resp) => resp,
        Err(e) => {
            crate::debug_log::logf(format_args!("json parse error: {e}"));
            return Some(SignalEvent::Error(format!("JSON parse error: {e}")));
        }
    };

    // Correlate against a pending request by id. Notifications carry no id and
    // fall through to parse_signal_event.
    let rpc_id = resp.id.clone();
    let pending_method = rpc_id.as_ref().and_then(|id| {
        let mut map = pending.lock().unwrap_or_else(|e| e.into_inner());
        let method = map.remove(id).map(|(m, _)| m);
        // Sweep stale entries (signal-cli never responded).
        map.retain(|_, (_, ts)| ts.elapsed() < PENDING_REQUEST_TTL);
        method
    });

    if let Some(method) = pending_method {
        if let Some(ref err) = resp.error {
            crate::debug_log::logf(format_args!("rpc error: method={method} error={err:?}"));
            // Routing a tracked send method to the generic Error arm would leak
            // the pending entry and leave the local message in Sending forever.
            if method == "send" || method == "sendPollCreate" {
                rpc_id.map(|id| SignalEvent::SendFailed {
                    token: SendToken::new(id),
                })
            } else {
                Some(SignalEvent::Error(format!("{method}: {}", err.message)))
            }
        } else {
            resp.result
                .as_ref()
                .and_then(|result| parse_rpc_result(&method, result, rpc_id.as_deref()))
        }
    } else {
        parse_signal_event(&resp, download_dir)
    }
}

/// Send a JSON-RPC envelope to signal-cli's stdin and register the rpc id
/// with `method` for response correlation. Returns the rpc id.
///
/// Ordering matters: the entry only lands in `pending_requests` after the
/// stdin write succeeds. If we registered before the write, a serialize or
/// channel failure would leak an orphaned entry that sat in the map until the
/// 60s TTL sweep, and callers waiting on the correlated event (SendTimestamp
/// / SendFailed) would silently never hear back. See issue #434.
///
/// Extracted from `SignalClient::send_rpc` so it can be unit-tested without
/// spawning a real signal-cli process.
async fn send_rpc_impl(
    stdin_tx: &mpsc::Sender<String>,
    pending_requests: &Arc<Mutex<HashMap<String, (String, Instant)>>>,
    method: &str,
    params: serde_json::Value,
) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: method.to_string(),
        id: id.clone(),
        params: Some(params),
    };
    let json = serde_json::to_string(&request)?;
    stdin_tx
        .send(json)
        .await
        .with_context(|| format!("Failed to send {method} to signal-cli stdin"))?;
    match pending_requests.lock() {
        Ok(mut map) => {
            map.insert(id.clone(), (method.to_string(), Instant::now()));
        }
        Err(poisoned) => {
            // The mutex is poisoned (another task panicked while holding it).
            // We've already written the request to stdin, so signal-cli will
            // respond -- recover the inner data and insert anyway so the
            // response is still correlatable. Log loudly so the next debug
            // capture surfaces it.
            crate::debug_log::logf(format_args!(
                "send_rpc: pending_requests mutex poisoned, recovering and registering {method} (id={id})"
            ));
            let mut map = poisoned.into_inner();
            map.insert(id.clone(), (method.to_string(), Instant::now()));
        }
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Happy path: stdin write succeeds, pending_requests gains one entry
    /// whose method matches the call.
    #[tokio::test]
    async fn send_rpc_impl_registers_after_successful_send() {
        let (tx, mut rx) = mpsc::channel::<String>(8);
        let pending = Arc::new(Mutex::new(HashMap::new()));

        let id = send_rpc_impl(&tx, &pending, "listContacts", serde_json::json!({}))
            .await
            .expect("send_rpc_impl");

        let wire = rx.recv().await.expect("stdin payload");
        assert!(wire.contains("\"method\":\"listContacts\""));
        assert!(wire.contains(&id));

        let map = pending.lock().unwrap();
        let (method, _) = map.get(&id).expect("pending entry");
        assert_eq!(method, "listContacts");
        assert_eq!(map.len(), 1);
    }

    /// REL-001 regression: when the channel receiver is dropped, send() fails
    /// and pending_requests MUST NOT be mutated. Pre-fix, the insert ran first
    /// and orphaned an entry that lived until the 60s TTL sweep.
    #[tokio::test]
    async fn send_rpc_impl_does_not_leak_on_send_failure() {
        let (tx, rx) = mpsc::channel::<String>(8);
        drop(rx); // close the channel so send() returns Err
        let pending = Arc::new(Mutex::new(HashMap::new()));

        let result = send_rpc_impl(&tx, &pending, "listContacts", serde_json::json!({})).await;

        assert!(result.is_err(), "send must fail when receiver is dropped");
        let map = pending.lock().unwrap();
        assert!(
            map.is_empty(),
            "pending_requests must stay empty when stdin send fails (got {} entries)",
            map.len()
        );
    }

    /// REL-002 regression: a poisoned mutex used to silently drop the
    /// pending-requests insert (the `if let Ok(...)` arm just skipped on Err),
    /// orphaning the in-flight request. We now recover from the poison and
    /// insert anyway.
    #[tokio::test]
    async fn send_rpc_impl_recovers_from_poisoned_mutex() {
        let (tx, mut rx) = mpsc::channel::<String>(8);
        let pending: Arc<Mutex<HashMap<String, (String, Instant)>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Poison the mutex by panicking while holding the lock.
        let pending_clone = Arc::clone(&pending);
        let poison = std::thread::spawn(move || {
            let _guard = pending_clone.lock().unwrap();
            panic!("intentional poison");
        });
        let _ = poison.join();
        assert!(pending.is_poisoned(), "mutex should be poisoned");

        let id = send_rpc_impl(&tx, &pending, "listContacts", serde_json::json!({}))
            .await
            .expect("send_rpc_impl should succeed even with poisoned map");

        let _ = rx.recv().await;
        let map = pending.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            map.contains_key(&id),
            "pending_requests must contain the entry even after mutex was poisoned"
        );
    }

    // --- handle_stdout_line (#503) ---

    fn pending_map() -> Mutex<HashMap<String, (String, Instant)>> {
        Mutex::new(HashMap::new())
    }

    #[test]
    fn handle_stdout_line_blank_is_none() {
        let pending = pending_map();
        assert!(handle_stdout_line("   ", &pending, Path::new(".")).is_none());
        assert!(handle_stdout_line("", &pending, Path::new(".")).is_none());
    }

    #[test]
    fn handle_stdout_line_malformed_json_yields_error_without_panic() {
        let pending = pending_map();
        // Surfacing a parse error to the status bar is intentional; the key
        // guarantee is that a malformed frame never panics the reader task.
        let ev = handle_stdout_line("{ not json", &pending, Path::new("."));
        assert!(matches!(ev, Some(SignalEvent::Error(_))));
    }

    #[test]
    fn handle_stdout_line_correlated_response_removes_its_id() {
        let pending = pending_map();
        pending.lock().unwrap().insert(
            "abc".to_string(),
            ("listContacts".to_string(), Instant::now()),
        );
        let line = r#"{"jsonrpc":"2.0","id":"abc","result":[]}"#;
        let _ = handle_stdout_line(line, &pending, Path::new("."));
        assert!(
            !pending.lock().unwrap().contains_key("abc"),
            "the correlated id must be consumed from pending"
        );
    }

    #[test]
    fn handle_stdout_line_unknown_id_falls_through_to_notification() {
        let pending = pending_map();
        pending
            .lock()
            .unwrap()
            .insert("other".to_string(), ("send".to_string(), Instant::now()));
        // Id not in pending and no method field: correlation misses and
        // parse_signal_event yields None (nothing to parse).
        let line = r#"{"jsonrpc":"2.0","id":"unknown","result":[]}"#;
        assert!(handle_stdout_line(line, &pending, Path::new(".")).is_none());
        assert!(
            pending.lock().unwrap().contains_key("other"),
            "an unrelated pending entry must be left intact"
        );
    }

    #[test]
    fn handle_stdout_line_send_error_yields_send_failed() {
        let pending = pending_map();
        pending
            .lock()
            .unwrap()
            .insert("send-1".to_string(), ("send".to_string(), Instant::now()));
        let line = r#"{"jsonrpc":"2.0","id":"send-1","error":{"code":-1,"message":"boom"}}"#;
        let ev = handle_stdout_line(line, &pending, Path::new("."));
        assert!(
            matches!(ev, Some(SignalEvent::SendFailed { token }) if token == SendToken::new("send-1")),
            "a tracked send method error must route to SendFailed"
        );
        assert!(!pending.lock().unwrap().contains_key("send-1"));
    }

    #[test]
    fn handle_stdout_line_other_error_yields_status_error() {
        let pending = pending_map();
        pending.lock().unwrap().insert(
            "lc-1".to_string(),
            ("listContacts".to_string(), Instant::now()),
        );
        let line = r#"{"jsonrpc":"2.0","id":"lc-1","error":{"code":-1,"message":"nope"}}"#;
        match handle_stdout_line(line, &pending, Path::new(".")) {
            Some(SignalEvent::Error(msg)) => {
                assert!(msg.contains("listContacts"), "got: {msg}");
                assert!(msg.contains("nope"), "got: {msg}");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn handle_stdout_line_sweeps_stale_pending_entries() {
        let pending = pending_map();
        {
            let mut map = pending.lock().unwrap();
            map.insert(
                "stale".to_string(),
                (
                    "send".to_string(),
                    Instant::now() - PENDING_REQUEST_TTL - Duration::from_secs(1),
                ),
            );
            map.insert(
                "fresh".to_string(),
                ("listContacts".to_string(), Instant::now()),
            );
        }
        // Any correlated line triggers the TTL sweep (an id is needed to enter
        // the correlation closure).
        let line = r#"{"jsonrpc":"2.0","id":"fresh","result":[]}"#;
        let _ = handle_stdout_line(line, &pending, Path::new("."));
        let map = pending.lock().unwrap();
        assert!(!map.contains_key("stale"), "stale entry must be swept");
        assert!(!map.contains_key("fresh"), "correlated id consumed");
    }

    #[test]
    fn drain_pending_as_failures_fails_tracked_sends_only() {
        let pending = pending_map();
        {
            let mut map = pending.lock().unwrap();
            map.insert("s1".to_string(), ("send".to_string(), Instant::now()));
            map.insert(
                "p1".to_string(),
                ("sendPollCreate".to_string(), Instant::now()),
            );
            map.insert(
                "lc".to_string(),
                ("listContacts".to_string(), Instant::now()),
            );
        }

        let events = drain_pending_as_failures(&pending);

        // Only the two tracked send methods become SendFailed.
        let mut failed: Vec<String> = events
            .into_iter()
            .map(|e| match e {
                SignalEvent::SendFailed { token } => token.to_string(),
                other => panic!("expected SendFailed, got {other:?}"),
            })
            .collect();
        failed.sort();
        assert_eq!(failed, vec!["p1".to_string(), "s1".to_string()]);

        // The map is fully drained (the untracked entry is dropped too).
        assert!(pending.lock().unwrap().is_empty());
    }

    #[test]
    fn handle_stdout_line_recovers_from_poisoned_pending() {
        let pending: Arc<Mutex<HashMap<String, (String, Instant)>>> =
            Arc::new(Mutex::new(HashMap::new()));
        pending
            .lock()
            .unwrap()
            .insert("p-1".to_string(), ("send".to_string(), Instant::now()));

        let clone = Arc::clone(&pending);
        let _ = std::thread::spawn(move || {
            let _g = clone.lock().unwrap();
            panic!("intentional poison");
        })
        .join();
        assert!(pending.is_poisoned(), "mutex should be poisoned");

        let line = r#"{"jsonrpc":"2.0","id":"p-1","error":{"code":-1,"message":"x"}}"#;
        let ev = handle_stdout_line(line, &pending, Path::new("."));
        assert!(
            matches!(ev, Some(SignalEvent::SendFailed { token }) if token == SendToken::new("p-1")),
            "correlation must survive a poisoned pending lock (REL-002)"
        );
    }
}

#[cfg(test)]
mod wire_tests {
    //! Lock in the JSON wire format for RPCs sent to signal-cli. These tests
    //! catch silent regressions where a refactor "tidies up" a load-bearing
    //! shape quirk (bare-string vs. array recipient, etc). See issue #433.
    use super::*;
    use serde_json::json;

    /// set_target: 1:1 recipients are wrapped in a single-element array.
    #[test]
    fn set_target_wraps_recipient_in_array() {
        let mut params = json!({});
        SignalClient::set_target(&mut params, "+15551234567", false);
        assert_eq!(
            params,
            json!({
                "recipient": ["+15551234567"],
            })
        );
    }

    /// set_target: group recipients use a bare string under `groupId`.
    #[test]
    fn set_target_uses_bare_group_id() {
        let mut params = json!({});
        SignalClient::set_target(&mut params, "Z0VlVnFLbE...", true);
        assert_eq!(
            params,
            json!({
                "groupId": "Z0VlVnFLbE...",
            })
        );
    }

    /// sendReaction (1:1): bare-string recipient (NOT array). signal-cli
    /// rejects the array form here. Distinct from set_target's behaviour.
    #[test]
    fn send_reaction_one_to_one_uses_bare_recipient() {
        let params = build_send_reaction_params(
            "+15550000000",
            "+15551234567",
            false,
            "👍",
            "+15559876543",
            1_700_000_000_000,
            false,
        );
        assert_eq!(
            params,
            json!({
                "account": "+15550000000",
                "recipient": "+15551234567",
                "emoji": "👍",
                "targetAuthor": "+15559876543",
                "targetTimestamp": 1_700_000_000_000_i64,
            })
        );
    }

    /// sendReaction (group): bare-string groupId.
    #[test]
    fn send_reaction_group_uses_bare_group_id() {
        let params = build_send_reaction_params(
            "+15550000000",
            "Z0VlVnFLbE...",
            true,
            "❤️",
            "+15559876543",
            1_700_000_000_000,
            false,
        );
        assert_eq!(
            params,
            json!({
                "account": "+15550000000",
                "groupId": "Z0VlVnFLbE...",
                "emoji": "❤️",
                "targetAuthor": "+15559876543",
                "targetTimestamp": 1_700_000_000_000_i64,
            })
        );
    }

    /// sendReaction with remove=true: adds top-level `remove: true` field.
    #[test]
    fn send_reaction_remove_sets_flag() {
        let params = build_send_reaction_params(
            "+15550000000",
            "+15551234567",
            false,
            "👍",
            "+15559876543",
            1_700_000_000_000,
            true,
        );
        assert_eq!(params.get("remove"), Some(&json!(true)));
    }

    /// sendReceipt: recipient wrapped in single-element array, targetTimestamp
    /// is the message-timestamp array, type=read.
    #[test]
    fn send_read_receipt_wire_shape() {
        let params = build_send_read_receipt_params(
            "+15550000000",
            "+15551234567",
            &[1_700_000_000_000, 1_700_000_000_001],
        );
        assert_eq!(
            params,
            json!({
                "account": "+15550000000",
                "recipient": ["+15551234567"],
                "type": "read",
                "targetTimestamp": [1_700_000_000_000_i64, 1_700_000_000_001_i64],
            })
        );
    }

    /// updateContact (1:1 disappearing-message timer): bare-string recipient.
    #[test]
    fn update_contact_expiration_uses_bare_recipient() {
        let params = build_update_contact_expiration_params("+15550000000", "+15551234567", 3600);
        assert_eq!(
            params,
            json!({
                "account": "+15550000000",
                "recipient": "+15551234567",
                "expiration": 3600_i64,
            })
        );
    }

    /// block/unblock: recipient or groupId wrapped in a single-element array.
    /// Both methods share build_block_params; this covers both shapes.
    #[test]
    fn block_one_to_one_wraps_recipient_in_array() {
        let params = build_block_params("+15550000000", "+15551234567", false);
        assert_eq!(
            params,
            json!({
                "account": "+15550000000",
                "recipient": ["+15551234567"],
            })
        );
    }

    #[test]
    fn block_group_wraps_group_id_in_array() {
        let params = build_block_params("+15550000000", "Z0VlVnFLbE...", true);
        assert_eq!(
            params,
            json!({
                "account": "+15550000000",
                "groupId": ["Z0VlVnFLbE..."],
            })
        );
    }
}
