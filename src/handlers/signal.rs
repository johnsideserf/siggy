//! Signal event dispatch.
//!
//! [`handle_signal_event`] is the single entry point: it routes each
//! [`SignalEvent`] variant to a per-arm handler. Each handler updates
//! `App` state in place (in-memory conversations, read markers, etc.)
//! and persists side effects through the database.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use chrono::{DateTime, Utc};
use ratatui::text::Line;

use crate::app::{
    App, OverlayKind, PASTE_CLEANUP_DELAY_SECS, WireQuote, show_desktop_notification,
};
use crate::conversation_store::{Conversation, DisplayMessage, Quote, db_warn, short_name};
use crate::db::Database;
use crate::image_render;
use crate::signal::types::{
    Contact, Group, IdentityInfo, Mention, MessageStatus, PollData, PollVote, Reaction,
    SignalEvent, SignalMessage, StyleType,
};

/// Convert a local file path to a file:/// URI (forward slashes, for terminal Ctrl+Click).
fn path_to_file_uri(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    if normalized.starts_with('/') {
        format!("file://{normalized}")
    } else {
        format!("file:///{normalized}")
    }
}

/// Dispatch a `SignalEvent` from the signal-cli backend to the appropriate handler.
pub fn handle_signal_event(app: &mut App, event: SignalEvent) {
    match event {
        SignalEvent::MessageReceived(msg) => handle_message(app, msg),
        SignalEvent::ReceiptReceived {
            sender,
            receipt_type,
            timestamps,
        } => {
            handle_receipt(app, &sender, &receipt_type, &timestamps);
        }
        SignalEvent::SendTimestamp { rpc_id, server_ts } => {
            handle_send_timestamp(app, &rpc_id, server_ts);
        }
        SignalEvent::SendFailed { rpc_id } => {
            app.status_message = "send failed".to_string();
            handle_send_failed(app, &rpc_id);
        }
        SignalEvent::TypingIndicator {
            sender,
            sender_name,
            is_typing,
            group_id,
        } => {
            app.store
                .remember_contact_name(&sender, sender_name.as_deref());
            // Key by group ID for group messages, sender phone for 1:1
            let conv_key = group_id.as_ref().unwrap_or(&sender).clone();
            if is_typing {
                app.typing
                    .indicators
                    .entry(conv_key)
                    .or_default()
                    .insert(sender.clone(), Instant::now());
            } else if let Some(senders) = app.typing.indicators.get_mut(&conv_key) {
                senders.remove(&sender);
                if senders.is_empty() {
                    app.typing.indicators.remove(&conv_key);
                }
            }
        }
        SignalEvent::ReactionReceived {
            conv_id,
            emoji,
            sender,
            sender_name,
            target_author,
            target_timestamp,
            is_remove,
        } => {
            app.store
                .remember_contact_name(&sender, sender_name.as_deref());
            handle_reaction(
                app,
                &conv_id,
                &emoji,
                &sender,
                &target_author,
                target_timestamp,
                is_remove,
            );
        }
        SignalEvent::EditReceived {
            conv_id,
            sender,
            sender_name,
            target_timestamp,
            new_body,
            new_timestamp: _,
            is_outgoing: _,
        } => {
            app.store
                .remember_contact_name(&sender, sender_name.as_deref());
            handle_edit_received(app, &conv_id, target_timestamp, &new_body);
        }
        SignalEvent::RemoteDeleteReceived {
            conv_id,
            sender: _,
            target_timestamp,
        } => {
            handle_remote_delete(app, &conv_id, target_timestamp);
        }
        SignalEvent::PinReceived {
            conv_id,
            sender,
            sender_name,
            target_author: _,
            target_timestamp,
        } => {
            app.store
                .remember_contact_name(&sender, sender_name.as_deref());
            handle_pin_received(app, &conv_id, &sender, target_timestamp, true);
        }
        SignalEvent::UnpinReceived {
            conv_id,
            sender,
            sender_name,
            target_author: _,
            target_timestamp,
        } => {
            app.store
                .remember_contact_name(&sender, sender_name.as_deref());
            handle_pin_received(app, &conv_id, &sender, target_timestamp, false);
        }
        SignalEvent::PollCreated {
            conv_id,
            timestamp,
            poll_data,
        } => {
            handle_poll_created(app, &conv_id, timestamp, poll_data);
        }
        SignalEvent::PollVoteReceived {
            conv_id,
            target_timestamp,
            voter,
            voter_name,
            option_indexes,
            vote_count,
        } => {
            app.store
                .remember_contact_name(&voter, voter_name.as_deref());
            handle_poll_vote(
                app,
                &conv_id,
                target_timestamp,
                &voter,
                voter_name.as_deref(),
                &option_indexes,
                vote_count,
            );
        }
        SignalEvent::PollTerminated {
            conv_id,
            target_timestamp,
        } => {
            handle_poll_terminated(app, &conv_id, target_timestamp);
        }
        SignalEvent::SystemMessage {
            conv_id,
            body,
            timestamp,
            timestamp_ms,
        } => {
            handle_system_message(app, &conv_id, &body, timestamp, timestamp_ms);
        }
        SignalEvent::ExpirationTimerChanged {
            conv_id,
            seconds,
            body,
            timestamp,
            timestamp_ms,
        } => {
            // Update conversation timer
            let is_group = app
                .store
                .conversations
                .get(&conv_id)
                .map(|c| c.is_group)
                .unwrap_or(false);
            let conv_name = app
                .store
                .contact_names
                .get(&conv_id)
                .cloned()
                .unwrap_or_else(|| conv_id.to_string());
            app.store
                .get_or_create_conversation(&conv_id, &conv_name, is_group, &app.db);
            if let Some(conv) = app.store.conversations.get_mut(&conv_id) {
                conv.expiration_timer = seconds;
            }
            app.db_warn_visible(
                app.db.update_expiration_timer(&conv_id, seconds),
                "update_expiration_timer",
            );
            // Insert system message
            handle_system_message(app, &conv_id, &body, timestamp, timestamp_ms);
        }
        SignalEvent::ReadSyncReceived { read_messages } => {
            handle_read_sync(app, read_messages);
        }
        SignalEvent::ContactList(contacts) => handle_contact_list(app, contacts),
        SignalEvent::GroupList(groups) => handle_group_list(app, groups),
        SignalEvent::IdentityList(identities) => handle_identity_list(app, identities),
        SignalEvent::Error(ref err) => {
            crate::debug_log::logf(format_args!("signal event error: {err}"));
            app.status_message = format!("error: {err}");
        }
    }
}

fn handle_message(app: &mut App, msg: SignalMessage) {
    let conv_id = if let Some(ref gid) = msg.group_id {
        gid.clone()
    } else if msg.is_outgoing {
        // Outgoing 1:1 — conversation is keyed by recipient
        match msg.destination {
            Some(ref dest) => dest.clone(),
            None => return,
        }
    } else {
        msg.source.clone()
    };

    if app.store.move_conversation_to_top(&conv_id) && app.is_overlay(OverlayKind::SidebarFilter) {
        app.refresh_sidebar_filter();
    }

    // Track sync burst progress
    if app.sync.active {
        app.sync.message_count += 1;
        app.sync.last_message_time = Some(Instant::now());
        app.status_message = format!("Syncing... ({} messages received)", app.sync.message_count);
        // Pin the viewport against the message at the bottom of the
        // active conversation BEFORE we append the new sync message,
        // so subsequent renders can hold that message at its original
        // screen position. See #394.
        app.maybe_capture_sync_pin(&conv_id);
    }

    // Store source_name in contact lookup for future resolution (typing indicators, etc.)
    if !msg.is_outgoing {
        app.store
            .remember_contact_name(&msg.source, msg.source_name.as_deref());
        // Populate UUID->name for @mention resolution
        if let (Some(uuid), Some(name)) = (&msg.source_uuid, &msg.source_name)
            && !name.is_empty()
        {
            app.store
                .uuid_to_name
                .entry(uuid.clone())
                .or_insert_with(|| name.clone());
        }
    }

    // Resolve conversation name: prefer message metadata, then contact lookup, then raw ID
    // For groups, source_name is the sender (not the group), so skip it
    let is_group = msg.group_id.is_some();
    let conv_name = msg
        .group_name
        .as_deref()
        .or(if is_group {
            None
        } else {
            msg.source_name.as_deref()
        })
        .unwrap_or_else(|| {
            app.store
                .contact_names
                .get(&conv_id)
                .map(|s| s.as_str())
                .unwrap_or(&conv_id)
        })
        .to_string();

    let sender_display = if msg.is_outgoing {
        "you".to_string()
    } else {
        msg.source_name
            .clone()
            .or_else(|| app.store.contact_names.get(&msg.source).cloned())
            .unwrap_or_else(|| short_name(&msg.source))
    };

    let sender_id = if msg.is_outgoing {
        app.account.clone()
    } else {
        msg.source.clone()
    };

    // Ensure conversation exists; detect message requests for new 1:1 from unknown senders
    let is_new = !app.store.conversations.contains_key(&conv_id);
    app.store
        .get_or_create_conversation(&conv_id, &conv_name, is_group, &app.db);
    if is_new && !msg.is_outgoing && !is_group && !app.store.contact_names.contains_key(&conv_id) {
        if let Some(conv) = app.store.conversations.get_mut(&conv_id) {
            conv.accepted = false;
        }
        app.db_warn_visible(app.db.update_accepted(&conv_id, false), "update_accepted");
    }

    let msg_ts_ms = msg.timestamp.timestamp_millis();
    // Outgoing synced messages already have a server timestamp; incoming messages have no status
    let msg_status = if msg.is_outgoing {
        Some(MessageStatus::Sent)
    } else {
        None
    };

    // Disappearing messages: extract expiration metadata
    let msg_expires_in = msg.expires_in_seconds;
    let msg_expiration_start = if msg_expires_in > 0 {
        // For received messages, start countdown now; for sent sync, use message timestamp
        if msg.is_outgoing {
            msg_ts_ms
        } else {
            Utc::now().timestamp_millis()
        }
    } else {
        0
    };

    // Keep conversation's expiration_timer in sync with incoming messages
    if let Some(conv) = app.store.conversations.get_mut(&conv_id)
        && conv.expiration_timer != msg_expires_in
    {
        conv.expiration_timer = msg_expires_in;
        db_warn(
            app.db.update_expiration_timer(&conv_id, msg_expires_in),
            "update_expiration_timer",
        );
    }

    // Resolve @mentions before the push closure borrows app mutably
    let resolved_body = msg
        .body
        .as_ref()
        .map(|body| app.store.resolve_mentions(body, &msg.mentions));

    // Resolve text styles (UTF-16 → byte offsets, accounting for mention replacements)
    let resolved_styles = resolved_body
        .as_ref()
        .map(|(resolved, _)| {
            app.store
                .resolve_text_styles(resolved, &msg.text_styles, &msg.mentions)
        })
        .unwrap_or_default();

    // Resolve quote from wire format
    let msg_quote = msg.quote.as_ref().map(|(ts, author_phone, body)| {
        let author_display = app
            .store
            .contact_names
            .get(author_phone)
            .cloned()
            .unwrap_or_else(|| {
                if *author_phone == app.account {
                    "you".to_string()
                } else {
                    author_phone.clone()
                }
            });
        (
            Quote {
                author: author_display,
                body: body.clone(),
                timestamp_ms: *ts,
                author_id: author_phone.clone(),
            },
            author_phone.clone(),
            body.clone(),
            *ts,
        )
    });
    let display_quote = msg_quote.as_ref().map(|(q, _, _, _)| q.clone());
    let wire_quote_author = msg_quote.as_ref().map(|(_, a, _, _)| a.clone());
    let wire_quote_body = msg_quote.as_ref().map(|(_, _, b, _)| b.clone());
    let wire_quote_ts = msg_quote.as_ref().map(|(_, _, _, t)| *t);

    // Helper: build a DisplayMessage in timestamp order and persist via on_message_added.
    let push_msg = |app: &mut App,
                    body: String,
                    image_lines: Option<Vec<Line<'static>>>,
                    image_path: Option<String>,
                    mention_ranges: Vec<(usize, usize)>,
                    style_ranges: Vec<(usize, usize, StyleType)>,
                    quote: Option<Quote>,
                    body_raw: Option<String>,
                    mentions: Vec<Mention>| {
        // Check for buffered poll data from a race condition (poll event arrived first)
        let deferred_poll = app
            .poll_vote
            .pending_polls
            .remove(&(conv_id.clone(), msg_ts_ms));
        let display = DisplayMessage {
            sender: sender_display.clone(),
            timestamp: msg.timestamp,
            body,
            is_system: false,
            image_lines,
            image_path,
            status: msg_status,
            timestamp_ms: msg_ts_ms,
            reactions: Vec::new(),
            mention_ranges,
            style_ranges,
            body_raw,
            mentions,
            quote,
            is_edited: false,
            is_deleted: false,
            is_pinned: false,
            sender_id: sender_id.clone(),
            expires_in_seconds: msg_expires_in,
            expiration_start_ms: msg_expiration_start,
            poll_data: deferred_poll,
            poll_votes: Vec::new(),
            preview: None,
            preview_image_lines: None,
            preview_image_path: None,
        };
        app.on_message_added(
            &conv_id,
            display,
            WireQuote {
                author: wire_quote_author.clone(),
                body: wire_quote_body.clone(),
                timestamp: wire_quote_ts,
            },
            true,
        );
    };

    // Add text body (with resolved @mentions and text styles)
    let had_mentions = !msg.mentions.is_empty();
    if let Some((resolved, ranges)) = resolved_body {
        let raw_body_for_msg = if had_mentions { msg.body.clone() } else { None };
        let mentions_for_msg = if had_mentions {
            msg.mentions.clone()
        } else {
            Vec::new()
        };
        push_msg(
            app,
            resolved,
            None,
            None,
            ranges,
            resolved_styles,
            display_quote,
            raw_body_for_msg,
            mentions_for_msg,
        );
    }

    // Add attachment notices
    for att in &msg.attachments {
        let label = att.filename.as_deref().unwrap_or(&att.content_type);
        let is_image = matches!(
            att.content_type.as_str(),
            "image/jpeg" | "image/png" | "image/gif" | "image/webp"
        );

        let path_info = att
            .local_path
            .as_deref()
            .map(|p| format!("({})", path_to_file_uri(p)))
            .unwrap_or_default();

        if is_image {
            let rendered = att
                .local_path
                .as_deref()
                .and_then(|p| image_render::render_image(Path::new(p), 40));
            push_msg(
                app,
                format!("[image: {label}]{path_info}"),
                rendered,
                att.local_path.clone(),
                Vec::new(),
                Vec::new(),
                None,
                None,
                Vec::new(),
            );
        } else {
            push_msg(
                app,
                format!("[attachment: {label}]{path_info}"),
                None,
                None,
                Vec::new(),
                Vec::new(),
                None,
                None,
                Vec::new(),
            );
        }
    }

    // Persist raw body + mentions so the display body can be re-resolved
    // later when the contact list or group list fills in unknown UUIDs.
    if had_mentions && let Some(ref raw) = msg.body {
        db_warn(
            app.db
                .upsert_message_mentions(&conv_id, msg_ts_ms, raw, &msg.mentions),
            "upsert_message_mentions",
        );
    }

    // Attach first link preview to the body message (not attachment messages)
    if let Some(preview) = msg.previews.into_iter().next() {
        if let Some(conv) = app.store.conversations.get_mut(&conv_id)
            && let Some(dm) = conv
                .messages
                .iter_mut()
                .rev()
                .find(|m| m.timestamp_ms == msg_ts_ms && !m.body.starts_with('['))
        {
            let (img_lines, img_path) =
                if app.image.show_link_previews && app.image.image_mode != "none" {
                    if let Some(ref p) = preview.image_path {
                        (
                            image_render::render_image(Path::new(p), 30),
                            Some(p.clone()),
                        )
                    } else {
                        (None, None)
                    }
                } else {
                    (None, None)
                };
            dm.preview = Some(preview.clone());
            dm.preview_image_lines = img_lines;
            dm.preview_image_path = img_path;
        }
        db_warn(
            app.db.upsert_link_preview(&conv_id, msg_ts_ms, &preview),
            "upsert_link_preview",
        );
    }

    let is_active = app
        .active_conversation
        .as_ref()
        .map(|a| a == &conv_id)
        .unwrap_or(false);

    if !is_active && !msg.is_outgoing {
        if let Some(c) = app.store.conversations.get_mut(&conv_id) {
            c.unread += 1;
        }
        let conv_accepted = app
            .store
            .conversations
            .get(&conv_id)
            .map(|c| c.accepted)
            .unwrap_or(true);
        let is_muted = app.is_muted_at(&conv_id, Utc::now());
        let not_muted_or_blocked =
            conv_accepted && !is_muted && !app.blocked_conversations.contains(&conv_id);
        let type_enabled = if is_group {
            app.notifications.notify_group
        } else {
            app.notifications.notify_direct
        };
        if app.sync.active {
            if type_enabled && not_muted_or_blocked {
                *app.sync
                    .suppressed_notifications
                    .entry(conv_id.clone())
                    .or_insert(0) += 1;
            }
        } else {
            if type_enabled && not_muted_or_blocked {
                app.notifications.pending_bell = true;
            }
            if app.notifications.desktop_notifications && not_muted_or_blocked {
                let notif_body = msg.body.as_deref().unwrap_or("");
                let notif_group = if is_group {
                    app.store
                        .conversations
                        .get(&conv_id)
                        .map(|c| c.name.clone())
                } else {
                    None
                };
                show_desktop_notification(
                    &sender_display,
                    notif_body,
                    is_group,
                    notif_group.as_deref(),
                    &app.notifications.notification_preview,
                );
            }
        }
    }

    // Viewport stabilization happens render-side via SyncState::pin --
    // see App::maybe_capture_sync_pin and the chat_pane renderer.

    // Active conversation: send read receipt and advance read marker
    let conv_accepted = app
        .store
        .conversations
        .get(&conv_id)
        .map(|c| c.accepted)
        .unwrap_or(true);
    if is_active {
        if !app.sync.active {
            if !msg.is_outgoing && conv_accepted && !app.blocked_conversations.contains(&conv_id) {
                app.queue_single_read_receipt(&sender_id, msg_ts_ms);
            }
            if let Some(conv) = app.store.conversations.get(&conv_id) {
                app.store
                    .last_read_index
                    .insert(conv_id.clone(), conv.messages.len());
            }
        }
        if let Ok(Some(rowid)) = app.db.last_message_rowid(&conv_id) {
            db_warn(app.db.save_read_marker(&conv_id, rowid), "save_read_marker");
        }
    }
}

pub(crate) fn handle_system_message(
    app: &mut App,
    conv_id: &str,
    body: &str,
    timestamp: DateTime<Utc>,
    timestamp_ms: i64,
) {
    let is_group = app
        .store
        .conversations
        .get(conv_id)
        .map(|c| c.is_group)
        .unwrap_or(false);
    let conv_name = app
        .store
        .contact_names
        .get(conv_id)
        .cloned()
        .unwrap_or_else(|| conv_id.to_string());
    app.store
        .get_or_create_conversation(conv_id, &conv_name, is_group, &app.db);
    let msg = DisplayMessage {
        sender: String::new(),
        timestamp,
        body: body.to_string(),
        is_system: true,
        image_lines: None,
        image_path: None,
        status: None,
        timestamp_ms,
        reactions: Vec::new(),
        mention_ranges: Vec::new(),
        style_ranges: Vec::new(),
        body_raw: None,
        mentions: Vec::new(),
        quote: None,
        is_edited: false,
        is_deleted: false,
        is_pinned: false,
        sender_id: String::new(),
        expires_in_seconds: 0,
        expiration_start_ms: 0,
        poll_data: None,
        poll_votes: Vec::new(),
        preview: None,
        preview_image_lines: None,
        preview_image_path: None,
    };
    app.on_message_added(conv_id, msg, WireQuote::default(), true);
}

fn handle_reaction(
    app: &mut App,
    conv_id: &str,
    emoji: &str,
    sender: &str,
    target_author: &str,
    target_timestamp: i64,
    is_remove: bool,
) {
    // Find the message in memory and update reactions.
    // Pre-resolve names to avoid borrow conflict with app.store.conversations.
    let account = &app.account;
    let target_display = app.store.contact_names.get(target_author).cloned();
    // Resolve sender phone number to display name for rendering
    let is_self = sender == app.account;
    let sender_display = if is_self {
        "you".to_string()
    } else {
        app.store
            .contact_names
            .get(sender)
            .cloned()
            .unwrap_or_else(|| sender.to_string())
    };
    if let Some(conv) = app.store.conversations.get_mut(conv_id) {
        let found = conv.find_msg_idx(target_timestamp).and_then(|idx| {
            let m = &conv.messages[idx];
            let matches = if m.sender == "you" {
                target_author == account.as_str()
            } else {
                m.sender == target_author || target_display.as_deref() == Some(m.sender.as_str())
            };
            if matches { Some(idx) } else { None }
        });
        if let Some(msg) = found.map(|idx| &mut conv.messages[idx]) {
            if is_remove {
                // Match by display name or "you" (for own reactions from other devices)
                msg.reactions.retain(|r| r.sender != sender_display);
            } else {
                // One reaction per user — replace or push
                if let Some(existing) = msg
                    .reactions
                    .iter_mut()
                    .find(|r| r.sender == sender_display)
                {
                    existing.emoji = emoji.to_string();
                } else {
                    msg.reactions.push(Reaction {
                        emoji: emoji.to_string(),
                        sender: sender_display,
                    });
                }
            }
        }
    }

    // Persist to DB regardless of whether message is in memory
    if is_remove {
        app.db_warn_visible(
            app.db
                .remove_reaction(conv_id, target_timestamp, target_author, sender),
            "remove_reaction",
        );
    } else {
        app.db_warn_visible(
            app.db
                .upsert_reaction(conv_id, target_timestamp, target_author, sender, emoji),
            "upsert_reaction",
        );
    }
}

fn handle_edit_received(app: &mut App, conv_id: &str, target_timestamp: i64, new_body: &str) {
    if let Some(conv) = app.store.conversations.get_mut(conv_id)
        && let Some(idx) = conv.find_msg_idx(target_timestamp)
    {
        conv.messages[idx].body = new_body.to_string();
        conv.messages[idx].is_edited = true;
    }
    app.db_warn_visible(
        app.db
            .update_message_body(conv_id, target_timestamp, new_body),
        "update_message_body",
    );
}

fn handle_remote_delete(app: &mut App, conv_id: &str, target_timestamp: i64) {
    if let Some(conv) = app.store.conversations.get_mut(conv_id)
        && let Some(idx) = conv.find_msg_idx(target_timestamp)
    {
        conv.messages[idx].is_deleted = true;
        conv.messages[idx].body = "[deleted]".to_string();
        conv.messages[idx].reactions.clear();
    }
    app.db_warn_visible(
        app.db.mark_message_deleted(conv_id, target_timestamp),
        "mark_message_deleted",
    );
}

fn handle_pin_received(
    app: &mut App,
    conv_id: &str,
    sender: &str,
    target_timestamp: i64,
    pinned: bool,
) {
    if let Some(conv) = app.store.conversations.get_mut(conv_id)
        && let Some(idx) = conv.find_msg_idx(target_timestamp)
    {
        conv.messages[idx].is_pinned = pinned;
    }
    app.db_warn_visible(
        app.db.set_message_pinned(conv_id, target_timestamp, pinned),
        "set_message_pinned",
    );
    // Insert system message — resolve sender to display name
    let sender_display = if sender == app.account {
        "you".to_string()
    } else {
        app.store
            .contact_names
            .get(sender)
            .cloned()
            .unwrap_or_else(|| sender.to_string())
    };
    let action = if pinned { "pinned" } else { "unpinned" };
    let body = format!("{sender_display} {action} a message");
    let now = Utc::now();
    let now_ms = now.timestamp_millis();
    handle_system_message(app, conv_id, &body, now, now_ms);
}

fn handle_poll_created(app: &mut App, conv_id: &str, timestamp: i64, poll_data: PollData) {
    // The poll arrives as a regular message too — find it and attach poll_data.
    // If the message hasn't arrived yet (race), buffer the poll data so
    // handle_message can attach it when the message arrives.
    if let Some(conv) = app.store.conversations.get_mut(conv_id) {
        if let Some(idx) = conv.find_msg_idx(timestamp) {
            conv.messages[idx].poll_data = Some(poll_data.clone());
        } else {
            app.poll_vote
                .pending_polls
                .insert((conv_id.to_string(), timestamp), poll_data.clone());
        }
    }
    app.db_warn_visible(
        app.db.upsert_poll_data(conv_id, timestamp, &poll_data),
        "upsert_poll_data",
    );
}

pub(crate) fn handle_poll_vote(
    app: &mut App,
    conv_id: &str,
    target_timestamp: i64,
    voter: &str,
    voter_name: Option<&str>,
    option_indexes: &[i64],
    vote_count: i64,
) {
    if let Some(conv) = app.store.conversations.get_mut(conv_id)
        && let Some(idx) = conv.find_msg_idx(target_timestamp)
    {
        let msg = &mut conv.messages[idx];
        // Upsert vote in memory
        if let Some(existing) = msg.poll_votes.iter_mut().find(|v| v.voter == voter) {
            existing.option_indexes = option_indexes.to_vec();
            existing.vote_count = vote_count;
            existing.voter_name = voter_name.map(|s| s.to_string());
        } else {
            msg.poll_votes.push(PollVote {
                voter: voter.to_string(),
                voter_name: voter_name.map(|s| s.to_string()),
                option_indexes: option_indexes.to_vec(),
                vote_count,
            });
        }
    }
    app.db_warn_visible(
        app.db.upsert_poll_vote(
            conv_id,
            target_timestamp,
            voter,
            voter_name,
            option_indexes,
            vote_count,
        ),
        "upsert_poll_vote",
    );
}

fn handle_poll_terminated(app: &mut App, conv_id: &str, target_timestamp: i64) {
    if let Some(conv) = app.store.conversations.get_mut(conv_id)
        && let Some(idx) = conv.find_msg_idx(target_timestamp)
        && let Some(ref mut poll) = conv.messages[idx].poll_data
    {
        poll.closed = true;
    }
    app.db_warn_visible(app.db.close_poll(conv_id, target_timestamp), "close_poll");
}

fn handle_read_sync(app: &mut App, read_messages: Vec<(String, i64)>) {
    // Group entries by conversation: for 1:1, the sender phone IS the conv_id.
    // For groups, we need to scan existing conversations to find which group
    // contains a message with that timestamp from that sender.
    let mut max_ts_per_conv: HashMap<String, i64> = HashMap::new();

    for (sender, timestamp) in &read_messages {
        // First try direct match: sender is a 1:1 conversation
        if app.store.conversations.contains_key(sender.as_str()) {
            let entry = max_ts_per_conv.entry(sender.clone()).or_insert(0);
            *entry = (*entry).max(*timestamp);
            continue;
        }
        // Otherwise, scan group conversations for a message matching this timestamp
        let mut found = false;
        for (conv_id, conv) in &app.store.conversations {
            if !conv.is_group {
                continue;
            }
            if conv.messages.iter().any(|m| m.timestamp_ms == *timestamp) {
                let entry = max_ts_per_conv.entry(conv_id.clone()).or_insert(0);
                *entry = (*entry).max(*timestamp);
                found = true;
                break;
            }
        }
        if !found {
            crate::debug_log::logf(format_args!(
                "read_sync: no conversation found for sender={} ts={timestamp}",
                crate::debug_log::mask_phone(sender)
            ));
        }
    }

    // For each conversation, advance the read marker
    for (conv_id, max_ts) in &max_ts_per_conv {
        let new_read_idx = if let Some(conv) = app.store.conversations.get(conv_id) {
            // partition_point gives the index of the first message with ts > max_ts
            conv.messages.partition_point(|m| m.timestamp_ms <= *max_ts)
        } else {
            continue;
        };

        // Only advance, never retreat
        let current = app.store.last_read_index.get(conv_id).copied().unwrap_or(0);
        if new_read_idx > current {
            app.store
                .last_read_index
                .insert(conv_id.clone(), new_read_idx);

            // Recompute unread from remaining messages after the read marker
            if let Some(conv) = app.store.conversations.get_mut(conv_id) {
                let unread = conv.messages[new_read_idx..]
                    .iter()
                    .filter(|m| !m.is_system && m.status.is_none())
                    .count();
                conv.unread = unread;
            }

            // Persist to DB
            if let Ok(Some(rowid)) = app.db.max_rowid_up_to_timestamp(conv_id, *max_ts) {
                db_warn(
                    app.db.save_read_marker(conv_id, rowid),
                    "save_read_marker (read_sync)",
                );
            }
        }
    }
}

fn handle_contact_list(app: &mut App, contacts: Vec<Contact>) {
    app.loading = false;
    app.startup_status.clear();
    for contact in contacts {
        // Store name in lookup for future message resolution
        if let Some(ref name) = contact.name
            && !name.is_empty()
        {
            app.store
                .contact_names
                .insert(contact.number.clone(), name.clone());
        }
        // Build UUID maps for @mention resolution
        if let Some(ref uuid) = contact.uuid {
            if let Some(ref name) = contact.name
                && !name.is_empty()
            {
                app.store.uuid_to_name.insert(uuid.clone(), name.clone());
            }
            app.store
                .number_to_uuid
                .insert(contact.number.clone(), uuid.clone());
        }
        // Update name on existing conversations only — don't create new ones
        if let Some(conv) = app.store.conversations.get_mut(&contact.number)
            && let Some(ref contact_name) = contact.name
            && !contact_name.is_empty()
            && conv.name != *contact_name
        {
            conv.name = contact_name.clone();
            db_warn(
                app.db
                    .upsert_conversation(&contact.number, contact_name, false),
                "upsert_conversation",
            );
        }
    }
    // Auto-accept unaccepted 1:1 conversations whose sender is now a known contact
    let to_accept: Vec<String> = app
        .store
        .conversations
        .iter()
        .filter(|(_, c)| !c.accepted && !c.is_group && app.store.contact_names.contains_key(&c.id))
        .map(|(id, _)| id.clone())
        .collect();
    for id in to_accept {
        if let Some(conv) = app.store.conversations.get_mut(&id) {
            conv.accepted = true;
            db_warn(app.db.update_accepted(&id, true), "update_accepted");
        }
    }

    // Re-resolve reaction senders: DB stores phone numbers but display
    // needs contact names (or "you" for own reactions).
    app.store.resolve_stored_names(&app.account);

    // Re-resolve @mention display bodies: messages that arrived before the
    // contact list may have fallen back to truncated UUIDs. (#283)
    app.store.rebuild_mention_display(&app.db);
}

fn handle_group_list(app: &mut App, groups: Vec<Group>) {
    for group in groups {
        // Store name in lookup for future message resolution
        if !group.name.is_empty() {
            app.store
                .contact_names
                .insert(group.id.clone(), group.name.clone());
        }
        // Store UUID↔phone mappings from group members
        for (phone, uuid) in &group.member_uuids {
            app.store
                .number_to_uuid
                .entry(phone.clone())
                .or_insert_with(|| uuid.clone());
        }
        // Populate UUID->name from group members (phone->uuid + phone->name)
        for (phone, uuid) in &group.member_uuids {
            if let Some(name) = app.store.contact_names.get(phone)
                && !name.is_empty()
            {
                app.store
                    .uuid_to_name
                    .entry(uuid.clone())
                    .or_insert_with(|| name.clone());
            }
        }
        // Store group for @mention member lookup
        app.store.groups.insert(group.id.clone(), group.clone());
        // Groups are always "active" (you're a member), so create conversations
        let conv = app
            .store
            .get_or_create_conversation(&group.id, &group.name, true, &app.db);
        if !group.name.is_empty() && conv.name != group.name {
            conv.name = group.name.clone();
            db_warn(
                app.db.upsert_conversation(&group.id, &group.name, true),
                "upsert_conversation",
            );
        }
    }
    // Re-resolve reaction senders with any new names from group members.
    app.store.resolve_stored_names(&app.account);

    // Re-resolve @mention display bodies: group member names may now fill
    // in UUIDs that weren't known at message-receipt time. (#283)
    app.store.rebuild_mention_display(&app.db);
}

fn handle_identity_list(app: &mut App, identities: Vec<IdentityInfo>) {
    // Populate the trust level cache
    app.identity_trust.clear();
    for id in &identities {
        if let Some(ref number) = id.number {
            app.identity_trust.insert(number.clone(), id.trust_level);
        }
    }
    // If verify overlay is open, refresh the displayed identities
    if app.is_overlay(OverlayKind::Verify)
        && let Some(ref conv_id) = app.active_conversation
    {
        let conv_id = conv_id.clone();
        let is_group = app
            .store
            .conversations
            .get(&conv_id)
            .map(|c| c.is_group)
            .unwrap_or(false);
        if is_group {
            if let Some(group) = app.store.groups.get(&conv_id) {
                let members: HashSet<&str> = group.members.iter().map(|s| s.as_str()).collect();
                app.verify.identities = identities
                    .iter()
                    .filter(|id| {
                        id.number
                            .as_ref()
                            .is_some_and(|n| members.contains(n.as_str()))
                    })
                    .cloned()
                    .collect();
            }
        } else {
            app.verify.identities = identities
                .iter()
                .filter(|id| id.number.as_deref() == Some(conv_id.as_str()))
                .cloned()
                .collect();
        }
        // Clamp index
        if !app.verify.identities.is_empty() && app.verify.index >= app.verify.identities.len() {
            app.verify.index = app.verify.identities.len() - 1;
        }
    }
}

fn handle_send_timestamp(app: &mut App, rpc_id: &str, server_ts: i64) {
    // Schedule any paste temp file for deletion after the delay (signal-cli has confirmed send)
    if let Some((path, _)) = app.pending_paste_cleanups.remove(rpc_id) {
        app.pending_paste_cleanups.insert(
            rpc_id.to_string(),
            (
                path,
                Instant::now() + std::time::Duration::from_secs(PASTE_CLEANUP_DELAY_SECS),
            ),
        );
    }
    if let Some((conv_id, local_ts)) = app.pending.sends.remove(rpc_id) {
        crate::debug_log::logf(format_args!(
            "send confirmed: conv={} local_ts={local_ts} server_ts={server_ts}",
            crate::debug_log::mask_phone(&conv_id)
        ));
        let effective_ts = if server_ts != 0 { server_ts } else { local_ts };
        let mut found = false;
        if let Some(conv) = app.store.conversations.get_mut(&conv_id) {
            // Find the outgoing message with matching local timestamp
            if let Some(idx) = conv
                .find_msg_idx(local_ts)
                .filter(|&idx| conv.messages[idx].sender == "you")
            {
                conv.messages[idx].timestamp_ms = effective_ts;
                conv.messages[idx].status = Some(MessageStatus::Sent);
                found = true;
            }
        }
        if found {
            // Update the DB row's timestamp_ms from local → server
            app.db_warn_visible(
                app.db.update_message_timestamp_ms(
                    &conv_id,
                    local_ts,
                    effective_ts,
                    MessageStatus::Sent.to_i32(),
                ),
                "update_message_timestamp_ms",
            );
        }

        // Replay any buffered receipts that may have arrived before this SendTimestamp
        if !app.pending.receipts.is_empty() {
            let receipts = std::mem::take(&mut app.pending.receipts);
            for (sender, receipt_type, timestamps) in receipts {
                handle_receipt(app, &sender, &receipt_type, &timestamps);
            }
        }
    }
}

fn handle_send_failed(app: &mut App, rpc_id: &str) {
    // Schedule any paste temp file for deletion after the delay (signal-cli has finished with it)
    if let Some((path, _)) = app.pending_paste_cleanups.remove(rpc_id) {
        app.pending_paste_cleanups.insert(
            rpc_id.to_string(),
            (
                path,
                Instant::now() + std::time::Duration::from_secs(PASTE_CLEANUP_DELAY_SECS),
            ),
        );
    }
    if let Some((conv_id, local_ts)) = app.pending.sends.remove(rpc_id) {
        let mut found = false;
        if let Some(conv) = app.store.conversations.get_mut(&conv_id)
            && let Some(idx) = conv
                .find_msg_idx(local_ts)
                .filter(|&idx| conv.messages[idx].sender == "you")
        {
            conv.messages[idx].status = Some(MessageStatus::Failed);
            found = true;
        }
        if found {
            app.db_warn_visible(
                app.db
                    .update_message_status(&conv_id, local_ts, MessageStatus::Failed.to_i32()),
                "update_message_status",
            );
        }
    }
}

/// Try to upgrade an outgoing message's status in a single conversation.
/// Returns true if a match was found for `ts`.
fn try_upgrade_receipt(
    db: &Database,
    conv_id: &str,
    conv: &mut Conversation,
    ts: i64,
    new_status: MessageStatus,
) -> bool {
    if let Some(idx) = conv
        .find_msg_idx(ts)
        .filter(|&idx| conv.messages[idx].sender == "you")
    {
        if let Some(current) = conv.messages[idx].status
            && new_status > current
        {
            conv.messages[idx].status = Some(new_status);
            db_warn(
                db.update_message_status(conv_id, ts, new_status.to_i32()),
                "update_message_status",
            );
        }
        return true;
    }
    false
}

fn handle_receipt(app: &mut App, sender: &str, receipt_type: &str, timestamps: &[i64]) {
    let receipt_upper = receipt_type.to_uppercase();
    let new_status = match receipt_upper.as_str() {
        "DELIVERY" => MessageStatus::Delivered,
        "READ" => MessageStatus::Read,
        "VIEWED" => MessageStatus::Viewed,
        _ => return,
    };

    let mut matched_any = false;

    // Try matching in the 1:1 conversation keyed by the receipt sender
    let conv_id = sender.to_string();
    if let Some(conv) = app.store.conversations.get_mut(&conv_id) {
        for ts in timestamps {
            if try_upgrade_receipt(&app.db, &conv_id, conv, *ts, new_status) {
                matched_any = true;
            }
        }
    }

    // If no match in 1:1, scan all conversations (handles group receipts
    // where sender is a member but conv is keyed by group ID)
    if !matched_any {
        for ts in timestamps {
            for (cid, conv) in &mut app.store.conversations {
                if try_upgrade_receipt(&app.db, cid, conv, *ts, new_status) {
                    matched_any = true;
                    break;
                }
            }
        }
    }

    // If still no match, the receipt may have arrived before the SendTimestamp
    // that assigns the server timestamp. Buffer it for replay.
    if !matched_any && !timestamps.is_empty() {
        crate::debug_log::logf(format_args!(
            "receipt: buffering {receipt_type} from {} (no matching ts)",
            crate::debug_log::mask_phone(sender)
        ));
        app.pending.receipts.push((
            sender.to_string(),
            receipt_type.to_string(),
            timestamps.to_vec(),
        ));
    } else if matched_any {
        crate::debug_log::logf(format_args!(
            "receipt: {receipt_type} from {} -> {new_status:?}",
            crate::debug_log::mask_phone(sender)
        ));
    }
}
