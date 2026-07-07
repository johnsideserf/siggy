//! signal-cli adapter (plan KTD-1, #640): the [`Backend`] impl that drives
//! the existing [`SignalClient`] JSON-RPC bridge.
//!
//! Ownership: the adapter borrows the client rather than owning it. The
//! caller (`run_main_flow`) owns the `SignalClient` because it must outlive
//! the main loop (shutdown happens after `run_app` returns), and the adapter
//! is just the seam the loop talks through. signal-cli wire quirks
//! (bare-string vs array recipients, rpc-id correlation) stay inside
//! [`SignalClient`] and `signal/parse/`; this module only routes.

use std::time::{Duration, Instant};

use crate::app::{self, App, SendRequest};
use crate::config::Config;
use crate::debug_log;
use crate::handlers;
use crate::input;
use crate::link;
use crate::signal::client::SignalClient;
use crate::signal::types::LinkState;

use super::Backend;

/// User-facing engine name (flow gap G7). Also exposed instance-free as
/// [`super::ACTIVE_ENGINE_NAME`] for surfaces that run before any backend
/// exists (`--check`).
pub const ENGINE_NAME: &str = "signal-cli";

pub struct SignalCliBackend<'a> {
    client: &'a mut SignalClient,
}

impl<'a> SignalCliBackend<'a> {
    pub fn new(client: &'a mut SignalClient) -> Self {
        Self { client }
    }

    /// Probe whether the configured account is registered as a linked device
    /// (plan U5, flow gap G1, #640). Spawns a one-shot `signal-cli
    /// listContacts` and reads its exit code - the same probe the wizard's
    /// linking step has always used. An associated fn rather than a trait
    /// method because every caller (`--check`, the setup wizard) runs before
    /// any engine instance or connection exists; see the [`Backend`] docs for
    /// the convention. U10 adds the native twin (open store, inspect
    /// registration state).
    pub async fn link_state(config: &Config) -> LinkState {
        link_state_from_probe(link::check_account_registered(config).await)
    }

    /// Registration sniff on a freshly spawned jsonRpc child (plan U5, flow
    /// gap G1, #640): signal-cli exits within milliseconds when the account
    /// is not registered, so a child still alive after 500ms is treated as
    /// linked. Byte-identical relocation of `run_main_flow`'s old inline
    /// `wait_for_ready` + stderr debug log; only the *inference* moved here -
    /// the raw process plumbing (`wait_for_ready`, stderr capture) stays on
    /// [`SignalClient`]. This sniff can only distinguish "exited early" from
    /// "still running", so it never reports [`LinkState::Corrupt`].
    pub async fn startup_link_state(client: &mut SignalClient) -> LinkState {
        if client.wait_for_ready(Duration::from_millis(500)).await {
            LinkState::Linked
        } else {
            let stderr = client.stderr_output();
            debug_log::logf(format_args!(
                "signal-cli exited early during startup, stderr: {stderr}"
            ));
            LinkState::Unlinked
        }
    }
}

/// Map the registration probe's outcome to a [`LinkState`]. Free function so
/// tests can inject probe results without spawning signal-cli (plan U5 test
/// scenarios). A probe that could not run cannot prove a link, so a spawn
/// failure maps to `Unlinked` - matching the wizard's old
/// `check_account_registered(..).unwrap_or(false)` exactly; `--check` reports
/// a missing binary separately before this is consulted.
pub(crate) fn link_state_from_probe(registered: anyhow::Result<bool>) -> LinkState {
    match registered {
        Ok(true) => LinkState::Linked,
        Ok(false) | Err(_) => LinkState::Unlinked,
    }
}

impl Backend for SignalCliBackend<'_> {
    fn display_name(&self) -> &'static str {
        ENGINE_NAME
    }

    /// Dispatch a SendRequest to signal-cli.
    async fn dispatch(&mut self, app: &mut App, req: SendRequest) {
        match req {
            SendRequest::Message {
                recipient,
                body,
                is_group,
                local_ts_ms,
                mentions,
                text_styles,
                attachment,
                preview,
                quote_timestamp,
                quote_author,
                quote_body,
            } => {
                let attachments: Vec<std::path::PathBuf> = attachment.into_iter().collect();
                let quote = match (quote_author, quote_timestamp, quote_body) {
                    (Some(author), Some(ts), Some(body_text)) => Some((author, ts, body_text)),
                    _ => None,
                };
                let att_refs: Vec<&std::path::Path> =
                    attachments.iter().map(|p| p.as_path()).collect();
                match self
                    .client
                    .send_message(
                        &recipient,
                        &body,
                        is_group,
                        &mentions,
                        &text_styles,
                        &att_refs,
                        preview.as_ref(),
                        quote.as_ref().map(|(a, t, b)| (a.as_str(), *t, b.as_str())),
                    )
                    .await
                {
                    Ok(token) => {
                        debug_log::logf(format_args!(
                            "send: to={} ts={local_ts_ms}",
                            debug_log::mask_phone(&recipient)
                        ));
                        app.pending
                            .sends
                            .insert(token.clone(), (recipient.to_string(), local_ts_ms));
                        // Register any paste temp file for deferred deletion. The actual delete is
                        // triggered after send confirmation; this sentinel keeps it alive until then.
                        // Only one paste attachment per send is expected; break after the first match.
                        for path in &attachments {
                            if path.starts_with(&app.paste_temp_path) {
                                let sentinel = Instant::now()
                                    + Duration::from_secs(app::PASTE_CLEANUP_SENTINEL_SECS);
                                app.pending_paste_cleanups
                                    .insert(token.clone(), (path.clone(), sentinel));
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        app.status_message = format!("send error: {e}");
                        // The request never reached signal-cli, so no SendFailed event
                        // will ever arrive - mark the optimistic message Failed here
                        // or it stays in Sending forever (#486).
                        handlers::signal::mark_send_failed(app, &recipient, local_ts_ms);
                        // RPC failed to send - delete temp file immediately (signal-cli never saw it)
                        for path in &attachments {
                            if path.starts_with(&app.paste_temp_path) {
                                let _ = std::fs::remove_file(path);
                            }
                        }
                    }
                }
            }
            SendRequest::Reaction {
                conv_id,
                emoji,
                is_group,
                target_author,
                target_timestamp,
                remove,
            } => {
                if let Err(e) = self
                    .client
                    .send_reaction(
                        &conv_id,
                        is_group,
                        &emoji,
                        &target_author,
                        target_timestamp,
                        remove,
                    )
                    .await
                {
                    app.status_message = format!("reaction error: {e}");
                }
            }
            SendRequest::Edit {
                recipient,
                body,
                is_group,
                edit_timestamp,
                local_ts_ms,
                mentions,
                text_styles,
                quote_timestamp,
                quote_author,
                quote_body,
            } => {
                let quote = match (quote_author, quote_timestamp, quote_body) {
                    (Some(author), Some(ts), Some(body_text)) => Some((author, ts, body_text)),
                    _ => None,
                };
                match self
                    .client
                    .send_edit_message(
                        &recipient,
                        &body,
                        is_group,
                        edit_timestamp,
                        &mentions,
                        &text_styles,
                        quote.as_ref().map(|(a, t, b)| (a.as_str(), *t, b.as_str())),
                    )
                    .await
                {
                    Ok(token) => {
                        debug_log::logf(format_args!(
                            "edit: to={} ts={edit_timestamp}",
                            debug_log::mask_phone(&recipient)
                        ));
                        app.pending
                            .sends
                            .insert(token, (recipient.to_string(), local_ts_ms));
                    }
                    Err(e) => {
                        app.status_message = format!("edit error: {e}");
                    }
                }
            }
            SendRequest::RemoteDelete {
                recipient,
                is_group,
                target_timestamp,
            } => {
                if let Err(e) = self
                    .client
                    .send_remote_delete(&recipient, is_group, target_timestamp)
                    .await
                {
                    app.status_message = format!("delete error: {e}");
                }
            }
            SendRequest::Typing {
                recipient,
                is_group,
                stop,
            } => {
                let _ = self.client.send_typing(&recipient, is_group, stop).await;
            }
            SendRequest::ReadReceipt {
                recipient,
                timestamps,
            } => {
                if let Err(e) = self.client.send_read_receipt(&recipient, &timestamps).await {
                    debug_log::logf(format_args!("read receipt error: {e}"));
                }
            }
            SendRequest::UpdateExpiration {
                conv_id,
                is_group,
                seconds,
            } => {
                let result = if is_group {
                    self.client
                        .send_update_group_expiration(&conv_id, seconds)
                        .await
                } else {
                    self.client
                        .send_update_contact_expiration(&conv_id, seconds)
                        .await
                };
                if let Err(e) = result {
                    app.status_message = format!("expiration error: {e}");
                } else if seconds == 0 {
                    app.status_message = "Disappearing messages disabled".to_string();
                } else {
                    app.status_message = format!(
                        "Disappearing messages set to {}",
                        input::format_compact_duration(seconds),
                    );
                }
            }
            SendRequest::CreateGroup { name } => match self.client.create_group(&name, &[]).await {
                Err(e) => {
                    app.status_message = format!("create group error: {e}");
                }
                _ => {
                    app.status_message = format!("Created group \"{}\"", name);
                    let _ = self.client.list_groups().await;
                }
            },
            SendRequest::AddGroupMembers { group_id, members } => {
                match self.client.add_group_members(&group_id, &members).await {
                    Err(e) => {
                        app.status_message = format!("add member error: {e}");
                    }
                    _ => {
                        let names: Vec<String> = members
                            .iter()
                            .map(|m| {
                                app.store
                                    .contact_names
                                    .get(m)
                                    .cloned()
                                    .unwrap_or_else(|| m.clone())
                            })
                            .collect();
                        app.status_message = format!("Added {}", names.join(", "));
                        let _ = self.client.list_groups().await;
                    }
                }
            }
            SendRequest::RemoveGroupMembers { group_id, members } => {
                match self.client.remove_group_members(&group_id, &members).await {
                    Err(e) => {
                        app.status_message = format!("remove member error: {e}");
                    }
                    _ => {
                        let names: Vec<String> = members
                            .iter()
                            .map(|m| {
                                app.store
                                    .contact_names
                                    .get(m)
                                    .cloned()
                                    .unwrap_or_else(|| m.clone())
                            })
                            .collect();
                        app.status_message = format!("Removed {}", names.join(", "));
                        let _ = self.client.list_groups().await;
                    }
                }
            }
            SendRequest::RenameGroup { group_id, name } => {
                match self.client.rename_group(&group_id, &name).await {
                    Err(e) => {
                        app.status_message = format!("rename group error: {e}");
                    }
                    _ => {
                        // Update locally for instant visual feedback
                        if let Some(conv) = app.store.conversations.get_mut(&group_id) {
                            conv.name = name.clone();
                        }
                        app.store
                            .contact_names
                            .insert(group_id.clone(), name.clone());
                        app.status_message = format!("Renamed group to \"{}\"", name);
                        let _ = self.client.list_groups().await;
                    }
                }
            }
            SendRequest::LeaveGroup { group_id } => match self.client.quit_group(&group_id).await {
                Err(e) => {
                    app.status_message = format!("leave group error: {e}");
                }
                _ => {
                    let name = app
                        .store
                        .conversations
                        .get(&group_id)
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| group_id.clone());
                    app.store.conversations.remove(&group_id);
                    app.store.conversation_order.retain(|id| id != &group_id);
                    app.store.groups.remove(&group_id);
                    if app.active_conversation.as_ref() == Some(&group_id) {
                        app.active_conversation = None;
                    }
                    app.status_message = format!("Left group \"{}\"", name);
                }
            },
            SendRequest::Block {
                recipient,
                is_group,
            } => {
                if let Err(e) = self.client.block_contact(&recipient, is_group).await {
                    app.status_message = format!("block error: {e}");
                }
            }
            SendRequest::Unblock {
                recipient,
                is_group,
            } => {
                if let Err(e) = self.client.unblock_contact(&recipient, is_group).await {
                    app.status_message = format!("unblock error: {e}");
                }
            }
            SendRequest::Pin {
                recipient,
                is_group,
                target_author,
                target_timestamp,
                pin_duration,
            } => {
                if let Err(e) = self
                    .client
                    .send_pin_message(
                        &recipient,
                        is_group,
                        &target_author,
                        target_timestamp,
                        pin_duration,
                    )
                    .await
                {
                    app.status_message = format!("pin error: {e}");
                }
            }
            SendRequest::Unpin {
                recipient,
                is_group,
                target_author,
                target_timestamp,
            } => {
                if let Err(e) = self
                    .client
                    .send_unpin_message(&recipient, is_group, &target_author, target_timestamp)
                    .await
                {
                    app.status_message = format!("unpin error: {e}");
                }
            }
            SendRequest::PollCreate {
                recipient,
                is_group,
                question,
                options,
                allow_multiple,
                local_ts_ms,
            } => {
                match self
                    .client
                    .send_poll_create(&recipient, is_group, &question, &options, allow_multiple)
                    .await
                {
                    Ok(token) => {
                        app.pending.sends.insert(token, (recipient, local_ts_ms));
                    }
                    Err(e) => {
                        app.status_message = format!("poll error: {e}");
                        handlers::signal::mark_send_failed(app, &recipient, local_ts_ms);
                    }
                }
            }
            SendRequest::PollVote {
                recipient,
                is_group,
                poll_author,
                poll_timestamp,
                option_indexes,
                vote_count,
            } => {
                if let Err(e) = self
                    .client
                    .send_poll_vote(
                        &recipient,
                        is_group,
                        &poll_author,
                        poll_timestamp,
                        &option_indexes,
                        vote_count,
                    )
                    .await
                {
                    app.status_message = format!("vote error: {e}");
                }
            }
            SendRequest::PollTerminate {
                recipient,
                is_group,
                poll_timestamp,
            } => {
                if let Err(e) = self
                    .client
                    .send_poll_terminate(&recipient, is_group, poll_timestamp)
                    .await
                {
                    app.status_message = format!("end poll error: {e}");
                }
            }
            SendRequest::MessageRequestResponse {
                recipient,
                is_group,
                response_type,
            } => {
                match self
                    .client
                    .send_message_request_response(&recipient, is_group, &response_type)
                    .await
                {
                    Err(e) => {
                        app.status_message = format!("message request error: {e}");
                    }
                    _ => {
                        app.status_message = match response_type.as_str() {
                            "accept" => "Message request accepted".to_string(),
                            "delete" => "Message request deleted".to_string(),
                            _ => String::new(),
                        };
                    }
                }
            }
            SendRequest::ListIdentities => {
                let _ = self.client.list_identities().await;
            }
            SendRequest::ResolveUsername { username } => {
                if let Err(e) = self.client.get_user_status(&username).await {
                    app.pending.username_resolve = None;
                    app.status_message = format!("Username lookup failed: {e}");
                }
            }
            SendRequest::TrustIdentity {
                recipient,
                safety_number,
            } => {
                match self.client.trust_identity(&recipient, &safety_number).await {
                    Err(e) => {
                        app.status_message = format!("trust error: {e}");
                    }
                    _ => {
                        app.status_message = format!(
                            "Verified {}",
                            app.store
                                .contact_names
                                .get(&recipient)
                                .unwrap_or(&recipient)
                        );
                        // Re-fetch identities to update trust levels
                        let _ = self.client.list_identities().await;
                    }
                }
            }
            SendRequest::UpdateProfile {
                given_name,
                family_name,
                about,
                about_emoji,
            } => {
                match self
                    .client
                    .update_profile(&given_name, &family_name, &about, &about_emoji)
                    .await
                {
                    Err(e) => {
                        app.status_message = format!("profile error: {e}");
                    }
                    _ => {
                        app.status_message = "Profile updated".to_string();
                    }
                }
            }
        }
    }

    async fn startup(&mut self, app: &mut App) {
        app.set_connected();

        // Purge messages that expired while the app was closed
        app.sweep_expired_messages();

        // Ask primary device to sync contacts/groups, then fetch them (best-effort)
        app.startup_status = "Syncing with primary device...".to_string();
        let _ = self.client.send_sync_request().await;
        app.startup_status = "Loading contacts...".to_string();
        let _ = self.client.list_contacts().await;
        app.startup_status = "Loading groups...".to_string();
        let _ = self.client.list_groups().await;
        app.startup_status = "Loading identities...".to_string();
        let _ = self.client.list_identities().await;
    }

    fn drain_events(&mut self, app: &mut App) -> bool {
        let mut changed = false;
        // Batch every DB write from this drain into one transaction. During
        // the initial sync burst this turns thousands of per-message
        // autocommits into a single commit per loop iteration (#489).
        app.db.begin_batch();
        loop {
            match self.client.event_rx.try_recv() {
                Ok(ev) => {
                    app.handle_signal_event(ev);
                    changed = true;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    if app.connection_error.is_none() {
                        let stderr = self.client.stderr_output();
                        let exit_info = self.client.try_child_exit();
                        let msg = if let Some(last_line) =
                            stderr.lines().last().filter(|l| !l.is_empty())
                        {
                            format!("signal-cli: {last_line}")
                        } else if let Some(code) = exit_info {
                            match code {
                                Some(c) => format!("signal-cli exited with code {c}"),
                                None => "signal-cli killed by signal".to_string(),
                            }
                        } else {
                            "signal-cli disconnected".to_string()
                        };
                        debug_log::logf(format_args!("disconnect: {msg}"));
                        app.connection_error = Some(msg);
                        app.connected = false;
                    }
                    break;
                }
                Err(_) => break,
            }
        }
        app.db.commit_batch();
        changed
    }

    fn supports_reconnect(&self) -> bool {
        true
    }

    /// Attempt to respawn signal-cli and swap it in behind the existing `&mut`
    /// borrow. Returns true on success. The previous client's child is already
    /// dead (that is why we are reconnecting), so dropping it here is safe (#497).
    async fn try_reconnect(&mut self, config: &Config) -> bool {
        match SignalClient::spawn(config).await {
            Ok(client) => {
                *self.client = client;
                true
            }
            Err(e) => {
                debug_log::logf(format_args!("reconnect spawn failed: {e}"));
                false
            }
        }
    }

    async fn resync_after_reconnect(&mut self) {
        let _ = self.client.send_sync_request().await;
        let _ = self.client.list_contacts().await;
        let _ = self.client.list_groups().await;
        let _ = self.client.list_identities().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mocked-probe mapping (plan U5 test scenarios): `link_state()`
    /// delegates to this, so these pin `--check`'s registration input
    /// without spawning signal-cli.
    #[test]
    fn link_state_from_probe_maps_registration_outcomes() {
        assert_eq!(link_state_from_probe(Ok(true)), LinkState::Linked);
        assert_eq!(link_state_from_probe(Ok(false)), LinkState::Unlinked);
        // A probe that could not run cannot prove a link (fails toward
        // relinking, like the wizard's old unwrap_or(false)).
        assert_eq!(
            link_state_from_probe(Err(anyhow::anyhow!("spawn failed"))),
            LinkState::Unlinked
        );
    }
}
