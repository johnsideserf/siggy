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
    Contact, Group, IdentityInfo, LinkPreview, Mention, MessageStatus, PollData, PollVote,
    Reaction, ReceiptKind, SignalEvent, SignalMessage, StyleType,
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

/// Actionable hint for a known class of signal-cli error (#530).
///
/// signal-cli surfaces low-level failures as `SignalEvent::Error` with cryptic
/// text (e.g. `getServerGuid(...) must not be null`) and emits them one per bad
/// envelope, so a burst of four flashes the raw exception four times and tells
/// the user nothing. This maps the recognized classes to one clear message;
/// the raw error always still goes to the debug log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignalCliHint {
    /// signal-cli lacks a JSON-RPC method siggy called: the build is outdated.
    Outdated,
    /// signal-cli threw while processing an incoming envelope, so the message
    /// never reached siggy. Typically an outdated build or a drifted session.
    UnprocessableMessage,
}

impl SignalCliHint {
    /// Classify an error string, or `None` to fall back to showing it raw.
    fn classify(err: &str) -> Option<Self> {
        if err.contains("Method not implemented") {
            return Some(Self::Outdated);
        }
        const MARKERS: &[&str] = &[
            "getServerGuid",
            "must not be null",
            "InvalidMessage",
            "ProtocolInvalidMessage",
            "No valid sessions",
            "InvalidKey",
        ];
        if MARKERS.iter().any(|m| err.contains(m)) {
            return Some(Self::UnprocessableMessage);
        }
        None
    }

    fn message(self) -> &'static str {
        match self {
            Self::Outdated => {
                "signal-cli looks outdated (missing a feature siggy uses); upgrade it to the latest version"
            }
            Self::UnprocessableMessage => {
                "signal-cli could not process an incoming message, so some messages may not appear; upgrade signal-cli, and re-link with --setup if it persists"
            }
        }
    }

    /// Per-session, per-category "already warned" flag so a burst shows the
    /// hint once rather than flickering it repeatedly.
    fn warned_flag(self) -> &'static std::sync::atomic::AtomicBool {
        use std::sync::atomic::AtomicBool;
        static OUTDATED: AtomicBool = AtomicBool::new(false);
        static UNPROCESSABLE: AtomicBool = AtomicBool::new(false);
        match self {
            Self::Outdated => &OUTDATED,
            Self::UnprocessableMessage => &UNPROCESSABLE,
        }
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
            handle_receipt(app, &sender, receipt_type, &timestamps);
        }
        SignalEvent::SendTimestamp { token, server_ts } => {
            handle_send_timestamp(app, &token, server_ts);
        }
        SignalEvent::SendFailed { token } => {
            app.status_message = "send failed".to_string();
            handle_send_failed(app, &token);
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
            handle_edit_received(app, &conv_id, &sender, target_timestamp, &new_body);
        }
        SignalEvent::RemoteDeleteReceived {
            conv_id,
            sender,
            target_timestamp,
        } => {
            handle_remote_delete(app, &conv_id, &sender, target_timestamp);
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
        SignalEvent::UserStatusList(statuses) => handle_user_status_list(app, statuses),
        SignalEvent::Error(ref err) => {
            crate::debug_log::logf(format_args!("signal event error: {err}"));
            // A getUserStatus RPC error means the pending /join @handle
            // resolution will never get a UserStatusList — clear the marker
            // so the join doesn't wedge on "Resolving..." (#612). The error
            // text itself lands in the status bar below.
            if err.starts_with("getUserStatus") {
                app.pending.username_resolve = None;
            }
            match SignalCliHint::classify(err) {
                Some(hint) => {
                    // Show the actionable hint once per session per category;
                    // signal-cli emits these in bursts, all kept in the log.
                    if !hint
                        .warned_flag()
                        .swap(true, std::sync::atomic::Ordering::Relaxed)
                    {
                        app.status_message = hint.message().to_string();
                    }
                }
                None => {
                    app.status_message = format!("error: {err}");
                }
            }
        }
        SignalEvent::SyncComplete => {
            // End-of-initial-sync (KTD-5, #640): flush the notification
            // digest, deferred read receipts, and viewport pin. The signal-cli
            // path emits this from the main loop's wall-clock heuristic; the
            // native backend will map presage's QueueEmpty to it.
            if app.sync.active {
                app.end_sync();
            }
        }
        SignalEvent::Disconnected => {
            // The signal-cli child exited. Any in-flight sends were already
            // failed via SendFailed events emitted just before this one. Mark
            // disconnected immediately; the main loop's channel-closed path
            // fills in the detailed exit reason (#497).
            app.connected = false;
        }
    }
}

/// Incoming sender identity to fold into the contact lookup before applying
/// the message-request check.
struct ContactIdentity {
    source: String,
    source_uuid: Option<String>,
    source_name: Option<String>,
}

/// One pushable DisplayMessage worth of resolved data. A single incoming
/// `SignalMessage` produces one entry for the text body (if any) plus one entry
/// per attachment.
struct ResolvedEntry {
    body: String,
    image_lines: Option<Vec<Line<'static>>>,
    image_path: Option<String>,
    mention_ranges: Vec<(usize, usize)>,
    style_ranges: Vec<(usize, usize, StyleType)>,
    quote: Option<Quote>,
    body_raw: Option<String>,
    mentions: Vec<Mention>,
}

/// An incoming `SignalMessage` after all pure-read resolution: identity, body
/// resolution, mention/style decoding, quote lookup, per-attachment body
/// strings. Built by [`resolve_incoming`] (no mutation) and consumed by
/// [`push_resolved`] and [`apply_notification_policy`].
struct ResolvedMessage {
    conv_id: String,
    conv_name: String,
    is_group: bool,
    is_outgoing: bool,
    sender_display: String,
    sender_id: String,
    timestamp: DateTime<Utc>,
    msg_ts_ms: i64,
    msg_status: Option<MessageStatus>,
    msg_expires_in: i64,
    msg_expiration_start: i64,
    /// One entry per DisplayMessage to append: text body first (if any),
    /// then one entry per attachment, in push order.
    entries: Vec<ResolvedEntry>,
    /// Raw body + mentions for `upsert_message_mentions` so the display body
    /// can be re-resolved when the contact/group list later fills in UUIDs.
    /// `None` when the message had no mentions.
    raw_body_for_mentions_db: Option<(String, Vec<Mention>)>,
    /// First link preview attached to this message, if any.
    preview: Option<LinkPreview>,
    /// Wire-format quote fields, for DB persistence via `on_message_added`.
    wire_quote: WireQuote,
    /// Original message body used as the desktop-notification preview string.
    /// Read only by [`apply_notification_policy`] when the OS notification fires.
    notification_preview_body: Option<String>,
    /// Source identity to fold into contact_names / uuid_to_name after the
    /// conversation is created. Only set for incoming messages.
    source_to_remember: Option<ContactIdentity>,
}

/// Pure read-only resolution of an incoming `SignalMessage`. Returns `None`
/// for outgoing 1:1 messages with no destination (can't be routed).
fn resolve_incoming(app: &App, msg: &SignalMessage) -> Option<ResolvedMessage> {
    let conv_id = if let Some(ref gid) = msg.group_id {
        gid.clone()
    } else if msg.is_outgoing {
        // Outgoing 1:1 — conversation is keyed by recipient
        msg.destination.as_ref()?.clone()
    } else {
        msg.source.clone()
    };

    let is_group = msg.group_id.is_some();

    // Conversation name: prefer message metadata, then contact lookup, then raw ID.
    // For groups, source_name is the sender (not the group), so skip it.
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

    let msg_ts_ms = msg.timestamp.timestamp_millis();
    // Outgoing synced messages already have a server timestamp; incoming messages have no status.
    let msg_status = if msg.is_outgoing {
        Some(MessageStatus::Sent)
    } else {
        None
    };

    // Disappearing-messages: extract expiration metadata.
    let msg_expires_in = msg.expires_in_seconds;
    let msg_expiration_start = if msg_expires_in > 0 {
        // For received messages, start countdown now; for sent sync, use message timestamp.
        if msg.is_outgoing {
            msg_ts_ms
        } else {
            Utc::now().timestamp_millis()
        }
    } else {
        0
    };

    let resolved_body = msg
        .body
        .as_ref()
        .map(|body| app.store.resolve_mentions(body, &msg.mentions));
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
    let wire_quote = WireQuote {
        author: msg_quote.as_ref().map(|(_, a, _, _)| a.clone()),
        body: msg_quote.as_ref().map(|(_, _, b, _)| b.clone()),
        timestamp: msg_quote.as_ref().map(|(_, _, _, t)| *t),
    };

    let had_mentions = !msg.mentions.is_empty();
    let mut entries: Vec<ResolvedEntry> = Vec::new();

    // Text body entry (if present).
    if let Some((resolved, ranges)) = resolved_body {
        let raw_body_for_msg = if had_mentions { msg.body.clone() } else { None };
        let mentions_for_msg = if had_mentions {
            msg.mentions.clone()
        } else {
            Vec::new()
        };
        entries.push(ResolvedEntry {
            body: resolved,
            image_lines: None,
            image_path: None,
            mention_ranges: ranges,
            style_ranges: resolved_styles,
            quote: display_quote,
            body_raw: raw_body_for_msg,
            mentions: mentions_for_msg,
        });
    }

    // Attachment entries.
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
            let rendered = att.local_path.as_deref().and_then(|p| {
                image_render::render_image_with_limits(
                    Path::new(p),
                    app.image.image_max_width,
                    app.image.image_max_height,
                )
            });
            entries.push(ResolvedEntry {
                body: format!("[image: {label}]{path_info}"),
                image_lines: rendered,
                image_path: att.local_path.clone(),
                mention_ranges: Vec::new(),
                style_ranges: Vec::new(),
                quote: None,
                body_raw: None,
                mentions: Vec::new(),
            });
        } else if crate::audio::is_audio(&att.content_type) {
            // Voice notes / audio: a play affordance instead of a generic
            // attachment label. `o` (open) plays it inline (#199), and the
            // label carries the duration when the file is Ogg Opus (#618).
            let duration = att
                .local_path
                .as_deref()
                .and_then(|p| crate::audio::ogg_opus_duration(Path::new(p)))
                .map(|d| format!(" {}", crate::audio::format_mmss(d)))
                .unwrap_or_default();
            entries.push(ResolvedEntry {
                body: format!("[voice \u{25b6} {label}{duration}]{path_info}"),
                image_lines: None,
                image_path: None,
                mention_ranges: Vec::new(),
                style_ranges: Vec::new(),
                quote: None,
                body_raw: None,
                mentions: Vec::new(),
            });
        } else {
            entries.push(ResolvedEntry {
                body: format!("[attachment: {label}]{path_info}"),
                image_lines: None,
                image_path: None,
                mention_ranges: Vec::new(),
                style_ranges: Vec::new(),
                quote: None,
                body_raw: None,
                mentions: Vec::new(),
            });
        }
    }

    let raw_body_for_mentions_db = if had_mentions {
        msg.body
            .as_ref()
            .map(|raw| (raw.clone(), msg.mentions.clone()))
    } else {
        None
    };

    let source_to_remember = if !msg.is_outgoing {
        Some(ContactIdentity {
            source: msg.source.clone(),
            source_uuid: msg.source_uuid.clone(),
            source_name: msg.source_name.clone(),
        })
    } else {
        None
    };

    Some(ResolvedMessage {
        conv_id,
        conv_name,
        is_group,
        is_outgoing: msg.is_outgoing,
        sender_display,
        sender_id,
        timestamp: msg.timestamp,
        msg_ts_ms,
        msg_status,
        msg_expires_in,
        msg_expiration_start,
        entries,
        raw_body_for_mentions_db,
        preview: msg.previews.first().cloned(),
        wire_quote,
        notification_preview_body: msg.body.clone(),
        source_to_remember,
    })
}

/// Apply mutations for an incoming message: sync-burst tracking, conversation
/// creation, append each entry, persist mention rows, attach link preview, and
/// update the read marker for the active conversation. The corresponding bell /
/// unread / desktop-notification side effects are handled by
/// [`apply_notification_policy`].
///
/// Returns the final accepted-state of the conversation so the caller can pass
/// the same snapshot to `apply_notification_policy` without re-reading the
/// store (nothing else mutates `accepted` after this point).
fn push_resolved(app: &mut App, r: &ResolvedMessage, is_active: bool) -> bool {
    refresh_sidebar_after_move(app, &r.conv_id);
    track_sync_progress(app, r);
    remember_sender_identity(app, r);

    let conv_accepted = accept_or_create_conversation(app, r);
    append_entries(app, r);
    persist_message_extras(app, r);

    if is_active {
        update_active_read_state(app, r, conv_accepted);
    }
    conv_accepted
}

/// Move this conversation to the top of the sidebar order. If the sidebar
/// filter is open, re-run the filter so the moved entry appears at its new
/// position instead of vanishing until the user types another character.
fn refresh_sidebar_after_move(app: &mut App, conv_id: &str) {
    if app.store.move_conversation_to_top(conv_id) && app.is_overlay(OverlayKind::SidebarFilter) {
        app.refresh_sidebar_filter();
    }
}

/// While a sync burst is active, bump the visible counter and capture a
/// viewport pin against the message at the bottom of the active conversation
/// BEFORE the new message appends. The pin holds that anchor at its original
/// screen position so the user does not get scroll-jumped during the burst
/// (see #394).
fn track_sync_progress(app: &mut App, r: &ResolvedMessage) {
    if !app.sync.active {
        return;
    }
    app.sync.message_count += 1;
    app.sync.last_message_time = Some(Instant::now());
    app.status_message = format!("Syncing... ({} messages received)", app.sync.message_count);
    app.maybe_capture_sync_pin(&r.conv_id);
}

/// Cache the sender's display name (and UUID -> name mapping when present) so
/// later events from the same sender resolve correctly even before the contact
/// list fills in. Must run BEFORE `accept_or_create_conversation` so a
/// previously-unknown sender does not get misclassified as a message-request.
fn remember_sender_identity(app: &mut App, r: &ResolvedMessage) {
    let Some(identity) = &r.source_to_remember else {
        return;
    };
    app.store
        .remember_contact_name(&identity.source, identity.source_name.as_deref());
    if let (Some(uuid), Some(name)) = (&identity.source_uuid, &identity.source_name)
        && !name.is_empty()
    {
        app.store
            .uuid_to_name
            .entry(uuid.clone())
            .or_insert_with(|| name.clone());
    }
}

/// Ensure a conversation row exists for this message; mark it unaccepted if
/// it is a new 1:1 from a sender we have no contact record for (which the UI
/// renders as a message-request prompt). Also keep the conversation's
/// disappearing-message timer in sync with the incoming message's timer.
///
/// Returns the post-mutation `accepted` value -- this is the canonical
/// snapshot consumers (read-receipt gate, notification policy) should use.
fn accept_or_create_conversation(app: &mut App, r: &ResolvedMessage) -> bool {
    // Detect "first message in this conversation" BEFORE creation.
    let is_new = !app.store.conversations.contains_key(&r.conv_id);

    app.store
        .get_or_create_conversation(&r.conv_id, &r.conv_name, r.is_group, &app.db);

    let is_unaccepted_request = is_new
        && !r.is_outgoing
        && !r.is_group
        && !app.store.contact_names.contains_key(&r.conv_id);
    if is_unaccepted_request {
        if let Some(conv) = app.store.conversations.get_mut(&r.conv_id) {
            conv.accepted = false;
        }
        app.db_warn_visible(app.db.update_accepted(&r.conv_id, false), "update_accepted");
    }

    if let Some(conv) = app.store.conversations.get_mut(&r.conv_id)
        && conv.expiration_timer != r.msg_expires_in
    {
        conv.expiration_timer = r.msg_expires_in;
        db_warn(
            app.db.update_expiration_timer(&r.conv_id, r.msg_expires_in),
            "update_expiration_timer",
        );
    }

    app.store
        .conversations
        .get(&r.conv_id)
        .map(|c| c.accepted)
        .unwrap_or(true)
}

/// Append each `ResolvedEntry` as a `DisplayMessage` in push order. Two
/// "first-entry only" fields are carried in `take()`-shaped locals so they
/// land on the body row (or, when there is no body, the first attachment
/// row) and not on subsequent attachment entries:
/// - `deferred_poll`: a poll event that arrived before this message and was
///   buffered; attaches once to avoid duplicate poll-data rendering.
/// - `entry_wire_quote`: the message's quote payload, which historically
///   got copy-persisted to every attachment row and produced duplicate
///   quote rendering on reload.
fn append_entries(app: &mut App, r: &ResolvedMessage) {
    let mut deferred_poll = app
        .poll_vote
        .pending_polls
        .remove(&(r.conv_id.clone(), r.msg_ts_ms));
    let mut entry_wire_quote = Some(r.wire_quote.clone());

    for entry in &r.entries {
        let display = DisplayMessage {
            sender: r.sender_display.clone(),
            timestamp: r.timestamp,
            body: entry.body.clone(),
            is_system: false,
            image_lines: entry.image_lines.clone(),
            image_path: entry.image_path.clone(),
            status: r.msg_status,
            timestamp_ms: r.msg_ts_ms,
            reactions: Vec::new(),
            mention_ranges: entry.mention_ranges.clone(),
            style_ranges: entry.style_ranges.clone(),
            body_raw: entry.body_raw.clone(),
            mentions: entry.mentions.clone(),
            quote: entry.quote.clone(),
            is_edited: false,
            is_deleted: false,
            is_pinned: false,
            sender_id: r.sender_id.clone(),
            expires_in_seconds: r.msg_expires_in,
            expiration_start_ms: r.msg_expiration_start,
            poll_data: deferred_poll.take(),
            poll_votes: Vec::new(),
            preview: None,
            preview_image_lines: None,
            preview_image_path: None,
        };
        let wq = entry_wire_quote.take().unwrap_or_default();
        app.on_message_added(&r.conv_id, display, wq, true);
    }
}

/// Persist the artifacts that hang off a message but live outside the
/// per-entry rows: raw body + mention ranges (so the display body can be
/// re-resolved when the contact list later fills in unknown UUIDs), and the
/// first link preview (decoded and attached to the body row, with the
/// preview row itself written to the DB).
fn persist_message_extras(app: &mut App, r: &ResolvedMessage) {
    if let Some((raw, mentions)) = &r.raw_body_for_mentions_db {
        db_warn(
            app.db
                .upsert_message_mentions(&r.conv_id, r.msg_ts_ms, raw, mentions),
            "upsert_message_mentions",
        );
    }

    let Some(preview) = &r.preview else {
        return;
    };

    if let Some(conv) = app.store.conversations.get_mut(&r.conv_id)
        && let Some(dm) = conv
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.timestamp_ms == r.msg_ts_ms && !m.body.starts_with('['))
    {
        let (img_lines, img_path) = if app.image.show_link_previews
            && app.image.image_mode != crate::domain::ImageMode::None
            && let Some(ref p) = preview.image_path
        {
            (
                image_render::render_image_with_limits(
                    Path::new(p),
                    app.image.preview_image_max_width,
                    app.image.image_max_height,
                ),
                Some(p.clone()),
            )
        } else {
            (None, None)
        };
        dm.preview = Some(preview.clone());
        dm.preview_image_lines = img_lines;
        dm.preview_image_path = img_path;
    }
    db_warn(
        app.db.upsert_link_preview(&r.conv_id, r.msg_ts_ms, preview),
        "upsert_link_preview",
    );
}

/// For a message in the currently-active conversation: queue a read receipt
/// (skipped during sync, for outgoing messages, when the conv is unaccepted,
/// or when the contact is blocked), advance the in-memory read marker, and
/// persist the on-disk read marker so reloads see the new position.
fn update_active_read_state(app: &mut App, r: &ResolvedMessage, conv_accepted: bool) {
    // Everything here is gated on !sync.active, including the on-disk
    // marker: persisting it mid-burst marks messages the user never saw as
    // read if the app dies before end_sync reconciles (#484). end_sync's
    // mark_read persists the marker once the burst completes.
    if !app.sync.active {
        if !r.is_outgoing && conv_accepted && !app.blocked_conversations.contains(&r.conv_id) {
            app.queue_single_read_receipt(&r.sender_id, r.msg_ts_ms);
        }
        if let Some(conv) = app.store.conversations.get(&r.conv_id) {
            app.store
                .last_read_index
                .insert(r.conv_id.clone(), conv.messages.len());
        }
        if let Ok(Some(rowid)) = app.db.last_message_rowid(&r.conv_id) {
            db_warn(
                app.db.save_read_marker(&r.conv_id, rowid),
                "save_read_marker",
            );
        }
    }
}

/// Apply notification side effects for an incoming message that is NOT in
/// the active conversation: bump unread, ring the bell, fire desktop
/// notification, or buffer for the post-sync digest if a sync burst is
/// running. Outgoing messages and messages in the active conversation are
/// silently ignored.
fn apply_notification_policy(
    app: &mut App,
    r: &ResolvedMessage,
    is_active: bool,
    conv_accepted: bool,
) {
    if is_active || r.is_outgoing {
        return;
    }

    if let Some(c) = app.store.conversations.get_mut(&r.conv_id) {
        c.unread += 1;
    }
    let is_muted = app.is_muted_at(&r.conv_id, Utc::now());
    let not_muted_or_blocked =
        conv_accepted && !is_muted && !app.blocked_conversations.contains(&r.conv_id);
    let type_enabled = if r.is_group {
        app.notifications.notify_group
    } else {
        app.notifications.notify_direct
    };

    if app.sync.active {
        if type_enabled && not_muted_or_blocked {
            *app.sync
                .suppressed_notifications
                .entry(r.conv_id.clone())
                .or_insert(0) += 1;
        }
        return;
    }

    if type_enabled && not_muted_or_blocked && !app.lock.is_locked() {
        app.notifications.pending_bell = true;
    }
    if app.notifications.desktop_notifications && not_muted_or_blocked && !app.lock.is_locked() {
        let notif_body = r.notification_preview_body.as_deref().unwrap_or("");
        let notif_group = if r.is_group {
            app.store
                .conversations
                .get(&r.conv_id)
                .map(|c| c.name.clone())
        } else {
            None
        };
        show_desktop_notification(
            &r.sender_display,
            notif_body,
            r.is_group,
            notif_group.as_deref(),
            app.notifications.notification_preview,
        );
    }
}

fn handle_message(app: &mut App, msg: SignalMessage) {
    let Some(resolved) = resolve_incoming(app, &msg) else {
        return;
    };
    let is_active = app
        .active_conversation
        .as_ref()
        .map(|a| a == &resolved.conv_id)
        .unwrap_or(false);
    let conv_accepted = push_resolved(app, &resolved, is_active);
    apply_notification_policy(app, &resolved, is_active, conv_accepted);
    evaluate_triggers(app, &msg, &resolved);
}

/// Run the message through the trigger engine (#615). Replies queue on
/// `pending.trigger_sends` (drained into the normal send path by the main
/// loop); `run` actions spawn immediately, fire-and-forget.
fn evaluate_triggers(app: &mut App, msg: &SignalMessage, resolved: &ResolvedMessage) {
    if app.triggers.rule_count() == 0 {
        return;
    }
    let Some(body) = msg.body.as_deref().filter(|b| !b.is_empty()) else {
        return;
    };
    let ctx = crate::trigger::MessageContext {
        body,
        sender_id: &msg.source,
        sender_name: msg.source_name.as_deref(),
        conv_id: &resolved.conv_id,
        conv_name: &resolved.conv_name,
        is_group: resolved.is_group,
        is_outgoing: msg.is_outgoing,
        timestamp_ms: msg.timestamp.timestamp_millis(),
    };
    let actions = app.triggers.evaluate(&ctx, std::time::Instant::now());
    for action in actions {
        match action {
            crate::trigger::TriggerAction::Reply {
                conv_id,
                is_group,
                text,
            } => queue_trigger_reply(app, conv_id, is_group, text),
            crate::trigger::TriggerAction::Run { argv, stdin_json } => {
                if let Err(e) = crate::trigger::execute_run(&argv, stdin_json) {
                    app.status_message = format!("trigger run failed: {e}");
                    crate::debug_log::logf(format_args!("trigger run {argv:?} failed: {e}"));
                }
            }
        }
    }
}

/// Locally echo a trigger auto-reply and queue its send. Mirrors the plain
/// path of `send_text` (no markup, mentions, or attachments).
fn queue_trigger_reply(app: &mut App, conv_id: String, is_group: bool, text: String) {
    let now = Utc::now();
    let local_ts_ms = now.timestamp_millis();
    let out_expires = app
        .store
        .conversations
        .get(&conv_id)
        .map(|c| c.expiration_timer)
        .unwrap_or(0);
    let echo = DisplayMessage {
        sender: "you".to_string(),
        timestamp: now,
        body: text.clone(),
        is_system: false,
        image_lines: None,
        image_path: None,
        status: Some(MessageStatus::Sending),
        timestamp_ms: local_ts_ms,
        reactions: Vec::new(),
        mention_ranges: Vec::new(),
        style_ranges: Vec::new(),
        body_raw: None,
        mentions: Vec::new(),
        quote: None,
        is_edited: false,
        is_deleted: false,
        is_pinned: false,
        sender_id: app.account.clone(),
        expires_in_seconds: out_expires,
        expiration_start_ms: if out_expires > 0 { local_ts_ms } else { 0 },
        poll_data: None,
        poll_votes: Vec::new(),
        preview: None,
        preview_image_lines: None,
        preview_image_path: None,
    };
    app.on_message_added(&conv_id, echo, WireQuote::default(), false);
    app.pending
        .trigger_sends
        .push(crate::app::SendRequest::Message {
            recipient: conv_id,
            body: text,
            is_group,
            local_ts_ms,
            mentions: Vec::new(),
            text_styles: Vec::new(),
            attachment: None,
            preview: None,
            quote_timestamp: None,
            quote_author: None,
            quote_body: None,
        });
}

pub(super) fn handle_system_message(
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
            let matches = if m.is_outgoing() {
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

/// Whether `sender` is the author of the message at `target_timestamp` and
/// may therefore edit or remote-delete it. The Signal server does not
/// enforce author-match on edit/delete envelopes; official clients verify
/// client-side, and skipping the check lets any conversation member rewrite
/// or delete someone else's message locally (#482). Checks the in-memory
/// message first, falling back to the DB row for messages outside the
/// loaded page; a message found nowhere yields false (nothing to mutate).
fn sender_may_mutate(app: &App, conv_id: &str, target_timestamp: i64, sender: &str) -> bool {
    let author = app
        .store
        .conversations
        .get(conv_id)
        .and_then(|conv| {
            conv.find_msg_idx(target_timestamp).map(|idx| {
                let m = &conv.messages[idx];
                (m.sender.clone(), m.sender_id.clone(), m.is_outgoing())
            })
        })
        .or_else(|| {
            app.db
                .message_author(conv_id, target_timestamp)
                .ok()
                .flatten()
        });
    let Some((msg_sender, msg_sender_id, outgoing)) = author else {
        return false;
    };
    if outgoing {
        // Only this account (sync from another of our devices) may touch
        // our own messages.
        return sender == app.account;
    }
    // Incoming message: the mutating sender must be its author. sender_id
    // carries the wire id; the display-name fallbacks mirror the author
    // check reactions already perform.
    msg_sender_id == sender
        || msg_sender == sender
        || app.store.contact_names.get(sender).map(String::as_str) == Some(msg_sender.as_str())
}

fn handle_edit_received(
    app: &mut App,
    conv_id: &str,
    sender: &str,
    target_timestamp: i64,
    new_body: &str,
) {
    if !sender_may_mutate(app, conv_id, target_timestamp, sender) {
        crate::debug_log::logf(format_args!(
            "rejected edit from non-author: conv={} ts={target_timestamp}",
            crate::debug_log::mask_phone(conv_id)
        ));
        return;
    }
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

fn handle_remote_delete(app: &mut App, conv_id: &str, sender: &str, target_timestamp: i64) {
    if !sender_may_mutate(app, conv_id, target_timestamp, sender) {
        crate::debug_log::logf(format_args!(
            "rejected remote-delete from non-author: conv={} ts={target_timestamp}",
            crate::debug_log::mask_phone(conv_id)
        ));
        return;
    }
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
    // handle_message can attach it when the message arrives. Buffer even
    // when the CONVERSATION doesn't exist yet (first-ever contact whose
    // poll event is ordered before its companion message), or the poll
    // renders as bare text until restart (#485).
    let target_idx = app
        .store
        .conversations
        .get_mut(conv_id)
        .and_then(|conv| conv.find_msg_idx(timestamp));
    match target_idx {
        Some(idx) => {
            if let Some(conv) = app.store.conversations.get_mut(conv_id) {
                conv.messages[idx].poll_data = Some(poll_data.clone());
            }
        }
        None => {
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

pub(super) fn handle_poll_vote(
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

/// Handle a getUserStatus response: complete a pending `/join @handle`
/// username resolution (#612). Responses with no pending join are ignored —
/// nothing else issues getUserStatus. A response arriving while a join is
/// pending always terminates the pending state, so the UI can never wedge on
/// "Resolving..." (only one getUserStatus is ever in flight).
fn handle_user_status_list(app: &mut App, statuses: Vec<crate::signal::types::UserStatus>) {
    let Some(pending) = app.pending.username_resolve.clone() else {
        return;
    };
    app.pending.username_resolve = None;

    // Tolerate signal-cli echoing the queried recipient with the u: prefix.
    let status = statuses.iter().find(|s| {
        s.recipient
            .trim_start_matches("u:")
            .eq_ignore_ascii_case(&pending)
            || s.username
                .as_deref()
                .is_some_and(|u| u.eq_ignore_ascii_case(&pending))
    });
    let Some(status) = status else {
        app.status_message = format!("Username lookup failed for @{pending}");
        return;
    };

    let (true, Some(uuid)) = (status.registered, status.uuid.as_deref()) else {
        app.status_message = format!("No Signal account found for @{pending}");
        return;
    };
    let handle = status.username.clone().unwrap_or_else(|| pending.clone());
    // The resolved account may already be a phone-keyed contact whose
    // username simply wasn't in the local contact list — reuse that key, or
    // incoming messages (keyed by sourceNumber) would land in a parallel,
    // uuid-keyed conversation.
    let key = app
        .store
        .number_to_uuid
        .iter()
        .find(|(_, u)| u.as_str() == uuid)
        .map(|(number, _)| number.clone())
        .unwrap_or_else(|| uuid.to_string());
    // Remember the resolution so future /join hits skip the round-trip
    app.store.usernames.insert(key.clone(), handle.clone());
    app.store.username_to_id.insert(handle.to_lowercase(), key);
    // Re-join through the username path, which now resolves locally and
    // creates the conversation under the right key with the right name.
    app.join_conversation(&format!("@{handle}"));
}

fn handle_contact_list(app: &mut App, contacts: Vec<Contact>) {
    app.loading = false;
    app.startup_status.clear();
    for contact in contacts {
        // Conversation/map key: phone number, else uuid for username-only
        // contacts (#612). Entries with neither are dropped at parse time.
        let Some(key) = contact.key().map(|k| k.to_string()) else {
            continue;
        };
        // Store name in lookup for future message resolution. A username-only
        // contact with no profile name still gets a displayable identity: the
        // @handle (#612).
        if let Some(ref name) = contact.name
            && !name.is_empty()
        {
            app.store.contact_names.insert(key.clone(), name.clone());
        } else if contact.number.is_none()
            && let Some(ref username) = contact.username
        {
            // Fallback identity only — never downgrade a name learned from an
            // earlier sync (phone contacts keep theirs the same way).
            app.store
                .contact_names
                .entry(key.clone())
                .or_insert_with(|| format!("@{username}"));
        }
        // Username maps: key → handle for display, handle → key for /join (#612)
        if let Some(ref username) = contact.username {
            app.store.usernames.insert(key.clone(), username.clone());
            app.store
                .username_to_id
                .insert(username.to_lowercase(), key.clone());
        }
        // Build UUID maps for @mention resolution
        if let Some(ref uuid) = contact.uuid {
            if let Some(ref name) = contact.name
                && !name.is_empty()
            {
                app.store.uuid_to_name.insert(uuid.clone(), name.clone());
            }
            if let Some(ref number) = contact.number {
                app.store
                    .number_to_uuid
                    .insert(number.clone(), uuid.clone());
            }
        }
        // Update name on existing conversations only — don't create new ones
        if let Some(conv) = app.store.conversations.get_mut(&key)
            && let Some(ref contact_name) = contact.name
            && !contact_name.is_empty()
            && conv.name != *contact_name
        {
            conv.name = contact_name.clone();
            db_warn(
                app.db.upsert_conversation(&key, contact_name, false),
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

fn handle_send_timestamp(app: &mut App, token: &crate::signal::types::SendToken, server_ts: i64) {
    // Schedule any paste temp file for deletion after the delay (signal-cli has confirmed send)
    if let Some((path, _)) = app.pending_paste_cleanups.remove(token) {
        app.pending_paste_cleanups.insert(
            token.clone(),
            (
                path,
                Instant::now() + std::time::Duration::from_secs(PASTE_CLEANUP_DELAY_SECS),
            ),
        );
    }
    if let Some((conv_id, local_ts)) = app.pending.sends.remove(token) {
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
                .filter(|&idx| conv.messages[idx].is_outgoing())
            {
                // Rewrite AND re-position: the server timestamp is typically
                // later than the local one, so mutating in place can leave
                // the vec unsorted, and every later find_msg_idx binary
                // search (receipts, reactions, edits, the next
                // SendTimestamp) then misses (#480).
                let mut msg = conv.messages.remove(idx);
                msg.timestamp_ms = effective_ts;
                msg.status = Some(MessageStatus::Sent);
                let new_idx = conv
                    .messages
                    .partition_point(|m| m.timestamp_ms <= effective_ts);
                conv.messages.insert(new_idx, msg);
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

        // Replay any buffered receipts that may have arrived before this
        // SendTimestamp. Entries that still don't match re-buffer with a
        // bumped attempt count; at the cap they are resolved against the DB
        // and dropped, so a receipt whose target is outside the loaded page
        // cannot re-buffer forever with its status update lost (#484).
        if !app.pending.receipts.is_empty() {
            let buffered = std::mem::take(&mut app.pending.receipts);
            for b in buffered {
                let status = b.receipt_type.status();
                let unmatched = apply_receipt(app, &b.sender, status, &b.timestamps);
                if unmatched.is_empty() {
                    continue;
                }
                if b.attempts + 1 >= MAX_RECEIPT_REPLAYS {
                    resolve_receipt_against_db(app, status, &unmatched);
                } else {
                    buffer_receipt(app, &b.sender, b.receipt_type, unmatched, b.attempts + 1);
                }
            }
        }
    }
}

fn handle_send_failed(app: &mut App, token: &crate::signal::types::SendToken) {
    // Schedule any paste temp file for deletion after the delay (signal-cli has finished with it)
    if let Some((path, _)) = app.pending_paste_cleanups.remove(token) {
        app.pending_paste_cleanups.insert(
            token.clone(),
            (
                path,
                Instant::now() + std::time::Duration::from_secs(PASTE_CLEANUP_DELAY_SECS),
            ),
        );
    }
    if let Some((conv_id, local_ts)) = app.pending.sends.remove(token) {
        mark_send_failed(app, &conv_id, local_ts);
    }
}

/// Mark an outgoing message Failed in memory and in the DB. Shared by the
/// RPC SendFailed path above and by dispatch_send's local-failure path
/// (stdin channel closed before the request ever reached signal-cli, #486).
pub(crate) fn mark_send_failed(app: &mut App, conv_id: &str, local_ts: i64) {
    let mut found = false;
    if let Some(conv) = app.store.conversations.get_mut(conv_id)
        && let Some(idx) = conv
            .find_msg_idx(local_ts)
            .filter(|&idx| conv.messages[idx].is_outgoing())
    {
        conv.messages[idx].status = Some(MessageStatus::Failed);
        found = true;
    }
    if found {
        // Not update_message_status: its monotonic guard rejects the
        // Sending -> Failed transition (Failed sorts below Sending).
        app.db_warn_visible(
            app.db.mark_message_failed(conv_id, local_ts),
            "mark_message_failed",
        );
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
        .filter(|&idx| conv.messages[idx].is_outgoing())
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

/// Failed replays before a buffered receipt is resolved against the DB and
/// dropped. Each in-flight send triggers one replay, so this bounds how many
/// other sends can confirm while a receipt's own target is still pending.
const MAX_RECEIPT_REPLAYS: u8 = 8;
/// Hard cap on the receipt buffer; the oldest entry is DB-resolved and
/// evicted when full.
const MAX_BUFFERED_RECEIPTS: usize = 64;

/// Apply a receipt to in-memory messages, per timestamp: try the 1:1
/// conversation keyed by the receipt sender first, then scan all
/// conversations (group receipts come from a member but the conv is keyed
/// by group ID). Returns the timestamps that matched nothing.
fn apply_receipt(
    app: &mut App,
    sender: &str,
    new_status: MessageStatus,
    timestamps: &[i64],
) -> Vec<i64> {
    let mut unmatched = Vec::new();
    let sender_conv = sender.to_string();
    for &ts in timestamps {
        let mut matched = false;
        if let Some(conv) = app.store.conversations.get_mut(&sender_conv) {
            matched = try_upgrade_receipt(&app.db, &sender_conv, conv, ts, new_status);
        }
        if !matched {
            for (cid, conv) in &mut app.store.conversations {
                if try_upgrade_receipt(&app.db, cid, conv, ts, new_status) {
                    matched = true;
                    break;
                }
            }
        }
        if !matched {
            unmatched.push(ts);
        }
    }
    unmatched
}

/// Resolve a receipt that will never match in memory directly against the
/// DB: the target may simply be outside the loaded message page (#484).
/// The monotonic guard in the UPDATE keeps this upgrade-only.
fn resolve_receipt_against_db(app: &App, new_status: MessageStatus, timestamps: &[i64]) {
    for &ts in timestamps {
        db_warn(
            app.db
                .update_outgoing_status_any_conv(ts, new_status.to_i32()),
            "update_outgoing_status_any_conv",
        );
    }
}

/// Buffer the unmatched timestamps of a receipt for replay after the next
/// SendTimestamp, evicting (with DB resolution) the oldest entry when full.
fn buffer_receipt(
    app: &mut App,
    sender: &str,
    receipt_type: ReceiptKind,
    timestamps: Vec<i64>,
    attempts: u8,
) {
    if app.pending.receipts.len() >= MAX_BUFFERED_RECEIPTS {
        let evicted = app.pending.receipts.remove(0);
        resolve_receipt_against_db(app, evicted.receipt_type.status(), &evicted.timestamps);
    }
    app.pending.receipts.push(crate::domain::BufferedReceipt {
        sender: sender.to_string(),
        receipt_type,
        timestamps,
        attempts,
    });
}

fn handle_receipt(app: &mut App, sender: &str, receipt_type: ReceiptKind, timestamps: &[i64]) {
    let new_status = receipt_type.status();

    let unmatched = apply_receipt(app, sender, new_status, timestamps);

    if unmatched.is_empty() {
        crate::debug_log::logf(format_args!(
            "receipt: {} from {} -> {new_status:?}",
            receipt_type.as_str(),
            crate::debug_log::mask_phone(sender)
        ));
    } else {
        // The receipt may predate the SendTimestamp that assigns the server
        // timestamp; buffer ONLY the unmatched timestamps for replay. (The
        // old per-event flag dropped unmatched timestamps whenever any
        // sibling timestamp in the same event matched, #484.)
        crate::debug_log::logf(format_args!(
            "receipt: buffering {} unmatched {} ts from {}",
            unmatched.len(),
            receipt_type.as_str(),
            crate::debug_log::mask_phone(sender)
        ));
        buffer_receipt(app, sender, receipt_type, unmatched, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::SignalCliHint;

    // #530: the exact errors from the bug report classify into actionable hints.
    #[test]
    fn classifies_outdated_signal_cli() {
        assert_eq!(
            SignalCliHint::classify("sendTypingIndicator: Method not implemented"),
            Some(SignalCliHint::Outdated)
        );
    }

    #[test]
    fn classifies_unprocessable_message() {
        assert_eq!(
            SignalCliHint::classify("signal-cli: getServerGuid(...) must not be null"),
            Some(SignalCliHint::UnprocessableMessage)
        );
    }

    #[test]
    fn classifies_decrypt_failures_as_unprocessable() {
        for err in [
            "signal-cli: No valid sessions",
            "signal-cli: ProtocolInvalidMessageException",
            "signal-cli: InvalidKeyException",
        ] {
            assert_eq!(
                SignalCliHint::classify(err),
                Some(SignalCliHint::UnprocessableMessage),
                "expected UnprocessableMessage for {err:?}"
            );
        }
    }

    #[test]
    fn generic_errors_fall_through_to_raw() {
        assert_eq!(SignalCliHint::classify("connection lost"), None);
        assert_eq!(SignalCliHint::classify("some other thing"), None);
    }
}
