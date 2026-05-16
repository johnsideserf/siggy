//! Composer input dispatch.
//!
//! [`handle_input`] is the single entry point: it parses the current
//! buffer into an [`InputAction`] and routes to a per-arm handler. Each
//! arm either updates `App` state in place or returns a [`SendRequest`]
//! for the main event loop to forward to `signal-cli`.

use std::collections::HashSet;

use chrono::Utc;

use crate::app::{App, GroupMenuState, OverlayKind, SendRequest, WireQuote};
use crate::conversation_store::{DisplayMessage, Quote, db_warn};
use crate::domain::EmojiPickerSource;
use crate::image_render;
use crate::input::{self, InputAction};
use crate::mute::MuteState;
use crate::signal::types::{IdentityInfo, Mention, MessageStatus, PollData, PollOption};

/// Handle a line of user input; returns Some(SendRequest) if a message
/// must be sent to signal-cli.
pub fn handle_input(app: &mut App) -> Option<SendRequest> {
    let input = app.input.buffer.clone();
    let trimmed = input.trim();
    if !trimmed.is_empty() {
        app.input.history.push(trimmed.to_string());
    }
    app.input.history_index = None;
    app.input.buffer.clear();
    app.input.cursor = 0;

    let action = input::parse_input(&input);
    match action {
        InputAction::SendText(raw_text) => send_text(app, raw_text),
        InputAction::Join(target) => {
            app.join_conversation(&target);
            None
        }
        InputAction::Part => {
            app.save_scroll_position();
            app.active_conversation = None;
            app.scroll.offset = 0;
            app.scroll.focused_index = None;
            app.pending_attachment = None;
            app.reset_typing_with_stop();
            app.update_status();
            None
        }
        InputAction::DeleteConversation => {
            if app.active_conversation.is_some() {
                app.open_overlay(OverlayKind::DeleteConversationConfirm);
            } else {
                app.status_message = "No active conversation to delete".to_string();
            }
            None
        }
        InputAction::Quit => {
            if app.input.buffer.is_empty() || app.quit_confirm {
                app.should_quit = true;
            } else {
                app.quit_confirm = true;
            }
            None
        }
        InputAction::Lock => {
            app.lock_now();
            None
        }
        InputAction::LockReset => {
            // If no passphrase exists yet, treat as SetPassphrase. Otherwise
            // enter ChangePassphraseOld and require verification first.
            let has_hash = crate::domain::load_hash(&app.lock.hash_path)
                .ok()
                .flatten()
                .filter(|s| !s.is_empty())
                .is_some();
            app.lock.phase = if has_hash {
                crate::domain::LockPhase::ChangePassphraseOld
            } else {
                crate::domain::LockPhase::SetPassphrase
            };
            app.lock.input_buffer.clear();
            app.lock.error = None;
            app.lock.old_passphrase_verified = false;
            None
        }
        InputAction::ToggleSidebar => {
            app.sidebar_visible = !app.sidebar_visible;
            None
        }
        InputAction::ToggleBell(target) => {
            toggle_bell(app, target.as_deref());
            None
        }
        InputAction::Mute(opt_dur) => {
            mute(app, opt_dur);
            None
        }
        InputAction::Block => block(app),
        InputAction::Unblock => unblock(app),
        InputAction::Settings => {
            app.open_overlay(OverlayKind::Settings);
            app.settings_index = 0;
            app.settings_mouse_snapshot = app.mouse.enabled;
            None
        }
        InputAction::Attach => {
            app.open_file_browser();
            None
        }
        InputAction::Search(query) => {
            app.search
                .open(query, app.active_conversation.as_deref(), &app.db);
            app.open_overlay(OverlayKind::Search);
            None
        }
        InputAction::Contacts => {
            app.open_overlay(OverlayKind::Contacts);
            app.contacts_overlay.index = 0;
            app.contacts_overlay.filter.clear();
            app.refresh_contacts_filter();
            None
        }
        InputAction::Emoji(query) => {
            let filter = if query.is_empty() { None } else { Some(query) };
            app.emoji_picker.open(EmojiPickerSource::Input, filter);
            app.open_overlay(OverlayKind::EmojiPicker);
            None
        }
        InputAction::Theme => {
            app.open_overlay(OverlayKind::ThemePicker);
            app.theme_picker.index = app
                .theme_picker
                .available_themes
                .iter()
                .position(|t| t.name == app.theme.name)
                .unwrap_or(0);
            None
        }
        InputAction::Group => {
            app.open_overlay(OverlayKind::GroupMenu);
            app.group_menu.state = Some(GroupMenuState::Menu);
            app.group_menu.index = 0;
            app.group_menu.filter.clear();
            app.group_menu.input.clear();
            None
        }
        InputAction::Verify => verify(app),
        InputAction::Profile => {
            app.open_overlay(OverlayKind::Profile);
            app.profile.index = 0;
            app.profile.editing = false;
            None
        }
        InputAction::About => {
            app.open_overlay(OverlayKind::About);
            None
        }
        InputAction::Keybindings => {
            app.open_overlay(OverlayKind::Keybindings);
            app.keybindings_overlay.index = 0;
            None
        }
        InputAction::Help => {
            app.open_overlay(OverlayKind::Help);
            None
        }
        InputAction::SetDisappearing(duration_str) => set_disappearing(app, duration_str),
        InputAction::Poll {
            question,
            options,
            allow_multiple,
        } => create_poll(app, question, options, allow_multiple),
        InputAction::Paste => app.handle_paste_command(),
        InputAction::Export(limit) => {
            app.export_chat_history(limit);
            None
        }
        InputAction::Unknown(msg) => {
            app.status_message = msg;
            None
        }
    }
}

fn send_text(app: &mut App, raw_text: String) -> Option<SendRequest> {
    let text = input::replace_shortcodes(&raw_text);
    if text.is_empty() && app.pending_attachment.is_none() && app.editing_message.is_none() {
        return None;
    }

    if let Some((edit_ts, edit_conv_id)) = app.editing_message.take() {
        return try_send_edit(app, edit_ts, edit_conv_id, &text);
    }

    let Some(conv_id) = app.active_conversation.clone() else {
        app.status_message = "No active conversation. Use /join <name> first.".to_string();
        return None;
    };

    let attachment = app.pending_attachment.take();
    let is_group = app
        .store
        .conversations
        .get(&conv_id)
        .map(|c| c.is_group)
        .unwrap_or(false);

    let (display_body, outgoing_image_lines, outgoing_image_path) =
        build_outgoing_attachment_body(app, &text, attachment.as_deref());

    let mut mention_ranges = Vec::new();
    for (name, _uuid) in &app.autocomplete.pending_mentions {
        let needle = format!("@{name}");
        if let Some(pos) = display_body.find(&needle) {
            mention_ranges.push((pos, pos + needle.len()));
        }
    }

    let (wire_body, wire_mentions) = app.prepare_outgoing_mentions(&text);
    app.autocomplete.pending_mentions.clear();

    let now = Utc::now();
    let local_ts_ms = now.timestamp_millis();
    let (quote, quote_timestamp, quote_author, quote_body) = build_outgoing_quote(app);

    let out_expires = app
        .store
        .conversations
        .get(&conv_id)
        .map(|c| c.expiration_timer)
        .unwrap_or(0);
    let out_expiry_start = if out_expires > 0 { local_ts_ms } else { 0 };

    let outgoing_msg = DisplayMessage {
        sender: "you".to_string(),
        timestamp: now,
        body: display_body,
        is_system: false,
        image_lines: outgoing_image_lines,
        image_path: outgoing_image_path,
        status: Some(MessageStatus::Sending),
        timestamp_ms: local_ts_ms,
        reactions: Vec::new(),
        mention_ranges,
        style_ranges: Vec::new(),
        body_raw: if wire_mentions.is_empty() {
            None
        } else {
            Some(wire_body.clone())
        },
        mentions: wire_mentions
            .iter()
            .map(|(start, uuid)| Mention {
                start: *start,
                length: 1,
                uuid: uuid.clone(),
            })
            .collect(),
        quote,
        is_edited: false,
        is_deleted: false,
        is_pinned: false,
        sender_id: app.account.clone(),
        expires_in_seconds: out_expires,
        expiration_start_ms: out_expiry_start,
        poll_data: None,
        poll_votes: Vec::new(),
        preview: None,
        preview_image_lines: None,
        preview_image_path: None,
    };
    app.on_message_added(
        &conv_id,
        outgoing_msg,
        WireQuote {
            author: quote_author.clone(),
            body: quote_body.clone(),
            timestamp: quote_timestamp,
        },
        false,
    );
    app.scroll.offset = 0;
    app.scroll.focused_index = None;
    app.reply_target = None;
    Some(SendRequest::Message {
        recipient: conv_id,
        body: wire_body,
        is_group,
        local_ts_ms,
        mentions: wire_mentions,
        attachment,
        quote_timestamp,
        quote_author,
        quote_body,
    })
}

fn try_send_edit(
    app: &mut App,
    edit_ts: i64,
    edit_conv_id: String,
    text: &str,
) -> Option<SendRequest> {
    if text.is_empty() {
        return None;
    }
    let original_quote = app
        .store
        .conversations
        .get(&edit_conv_id)
        .and_then(|conv| conv.find_msg_idx(edit_ts).map(|idx| &conv.messages[idx]))
        .filter(|msg| msg.sender == "you")
        .and_then(|msg| msg.quote.as_ref())
        .map(|q| (q.timestamp_ms, q.author_id.clone(), q.body.clone()));

    let conv = app.store.conversations.get_mut(&edit_conv_id)?;
    if let Some(idx) = conv
        .find_msg_idx(edit_ts)
        .filter(|&idx| conv.messages[idx].sender == "you")
    {
        conv.messages[idx].body = text.to_string();
        conv.messages[idx].is_edited = true;
    }
    let is_group = conv.is_group;
    let (wire_body, wire_mentions) = app.prepare_outgoing_mentions(text);
    app.autocomplete.pending_mentions.clear();
    app.db_warn_visible(
        app.db.update_message_body(&edit_conv_id, edit_ts, text),
        "update_message_body",
    );
    let now = Utc::now();
    Some(SendRequest::Edit {
        recipient: edit_conv_id,
        body: wire_body,
        is_group,
        edit_timestamp: edit_ts,
        local_ts_ms: now.timestamp_millis(),
        mentions: wire_mentions,
        quote_timestamp: original_quote.as_ref().map(|(ts, _, _)| *ts),
        quote_author: original_quote.as_ref().map(|(_, a, _)| a.clone()),
        quote_body: original_quote.map(|(_, _, b)| b),
    })
}

fn build_outgoing_attachment_body(
    app: &App,
    text: &str,
    attachment: Option<&std::path::Path>,
) -> (
    String,
    Option<Vec<ratatui::text::Line<'static>>>,
    Option<String>,
) {
    let Some(path) = attachment else {
        return (text.to_string(), None, None);
    };
    let fname = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let is_image = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp");
    let prefix = if is_image { "image" } else { "attachment" };
    let body = if text.is_empty() {
        format!("[{prefix}: {fname}]")
    } else {
        format!("[{prefix}: {fname}] {text}")
    };
    let (img_lines, img_path) = if is_image && app.image.image_mode != "none" {
        (
            image_render::render_image(path, 40),
            Some(path.to_string_lossy().into_owned()),
        )
    } else {
        (None, None)
    };
    (body, img_lines, img_path)
}

fn build_outgoing_quote(app: &App) -> (Option<Quote>, Option<i64>, Option<String>, Option<String>) {
    let Some((author_phone, body, ts)) = app.reply_target.as_ref() else {
        return (None, None, None, None);
    };
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
    let quote = Quote {
        author: author_display,
        body: body.clone(),
        timestamp_ms: *ts,
        author_id: author_phone.clone(),
    };
    (
        Some(quote),
        Some(*ts),
        Some(author_phone.clone()),
        Some(body.clone()),
    )
}

fn toggle_bell(app: &mut App, target: Option<&str>) {
    match target {
        None => {
            let new_state = !(app.notifications.notify_direct && app.notifications.notify_group);
            app.notifications.notify_direct = new_state;
            app.notifications.notify_group = new_state;
            let state = if new_state { "on" } else { "off" };
            app.status_message = format!("notifications {state}");
        }
        Some("direct" | "dm" | "1:1") => {
            app.notifications.notify_direct = !app.notifications.notify_direct;
            let state = if app.notifications.notify_direct {
                "on"
            } else {
                "off"
            };
            app.status_message = format!("direct notifications {state}");
        }
        Some("group" | "groups") => {
            app.notifications.notify_group = !app.notifications.notify_group;
            let state = if app.notifications.notify_group {
                "on"
            } else {
                "off"
            };
            app.status_message = format!("group notifications {state}");
        }
        Some(other) => {
            app.status_message = format!("unknown bell type: {other} (use direct or group)");
        }
    }
}

fn mute(app: &mut App, opt_dur: Option<String>) {
    app.status_message = match app.active_conversation.clone() {
        None => "no active conversation to mute".to_string(),
        Some(conv_id) => match opt_dur {
            None => {
                let new_state = (!app.muted_conversations.contains_key(&conv_id))
                    .then_some(MuteState::Permanent);
                app.apply_mute(&conv_id, new_state);
                let name = app.conversation_name(&conv_id);
                match new_state {
                    None => format!("unmuted {name}"),
                    Some(_) => format!("muted {name}"),
                }
            }
            Some(dur_str) => match input::parse_duration_to_seconds(&dur_str) {
                Ok(secs) if secs > 0 => {
                    let expiry = Utc::now() + chrono::Duration::seconds(secs);
                    app.apply_mute(&conv_id, Some(MuteState::Until(expiry)));
                    format!("muted {} for {dur_str}", app.conversation_name(&conv_id))
                }
                Ok(_) => "use /mute to unmute, or specify a duration: 30m, 2h, 1d, 1w".to_string(),
                Err(_) => format!("invalid duration '{dur_str}'. Try 30m, 2h, 1d, 1w"),
            },
        },
    };
}

fn block(app: &mut App) -> Option<SendRequest> {
    let Some(conv_id) = app.active_conversation.clone() else {
        app.status_message = "no active conversation to block".to_string();
        return None;
    };
    let is_group = app
        .store
        .conversations
        .get(&conv_id)
        .map(|c| c.is_group)
        .unwrap_or(false);
    if app.blocked_conversations.contains(&conv_id) {
        let name = app
            .store
            .conversations
            .get(&conv_id)
            .map(|c| c.name.as_str())
            .unwrap_or(&conv_id);
        app.status_message = format!("{name} is already blocked");
        None
    } else {
        let name = app
            .store
            .conversations
            .get(&conv_id)
            .map(|c| c.name.as_str())
            .unwrap_or(&conv_id);
        app.status_message = format!("blocked {name}");
        app.blocked_conversations.insert(conv_id.clone());
        db_warn(app.db.set_blocked(&conv_id, true), "set_blocked");
        Some(SendRequest::Block {
            recipient: conv_id,
            is_group,
        })
    }
}

fn unblock(app: &mut App) -> Option<SendRequest> {
    let Some(conv_id) = app.active_conversation.clone() else {
        app.status_message = "no active conversation to unblock".to_string();
        return None;
    };
    let is_group = app
        .store
        .conversations
        .get(&conv_id)
        .map(|c| c.is_group)
        .unwrap_or(false);
    if app.blocked_conversations.remove(&conv_id) {
        let name = app
            .store
            .conversations
            .get(&conv_id)
            .map(|c| c.name.as_str())
            .unwrap_or(&conv_id);
        app.status_message = format!("unblocked {name}");
        db_warn(app.db.set_blocked(&conv_id, false), "set_blocked");
        Some(SendRequest::Unblock {
            recipient: conv_id,
            is_group,
        })
    } else {
        let name = app
            .store
            .conversations
            .get(&conv_id)
            .map(|c| c.name.as_str())
            .unwrap_or(&conv_id);
        app.status_message = format!("{name} is not blocked");
        None
    }
}

fn verify(app: &mut App) -> Option<SendRequest> {
    let Some(conv_id) = app.active_conversation.clone() else {
        app.status_message = "no active conversation".to_string();
        return None;
    };
    let conv = &app.store.conversations[&conv_id];
    if conv.is_group {
        if let Some(group) = app.store.groups.get(&conv_id) {
            let members: HashSet<&str> = group.members.iter().map(|s| s.as_str()).collect();
            app.verify.identities = app
                .identity_trust
                .keys()
                .filter(|num| members.contains(num.as_str()))
                .filter_map(|num| {
                    Some(IdentityInfo {
                        number: Some(num.clone()),
                        uuid: None,
                        fingerprint: String::new(),
                        safety_number: String::new(),
                        trust_level: *app.identity_trust.get(num)?,
                        added_timestamp: 0,
                    })
                })
                .collect();
        } else {
            app.verify.identities.clear();
        }
    } else {
        app.verify.identities = app
            .identity_trust
            .get(&conv_id)
            .map(|tl| {
                vec![IdentityInfo {
                    number: Some(conv_id.clone()),
                    uuid: None,
                    fingerprint: String::new(),
                    safety_number: String::new(),
                    trust_level: *tl,
                    added_timestamp: 0,
                }]
            })
            .unwrap_or_default();
    }
    app.open_overlay(OverlayKind::Verify);
    app.verify.index = 0;
    Some(SendRequest::ListIdentities)
}

fn set_disappearing(app: &mut App, duration_str: String) -> Option<SendRequest> {
    let seconds = match input::parse_duration_to_seconds(&duration_str) {
        Ok(s) => s,
        Err(msg) => {
            app.status_message = msg;
            return None;
        }
    };
    let Some(conv_id) = app.active_conversation.clone() else {
        app.status_message = "No active conversation".to_string();
        return None;
    };
    let is_group = app
        .store
        .conversations
        .get(&conv_id)
        .map(|c| c.is_group)
        .unwrap_or(false);
    if let Some(conv) = app.store.conversations.get_mut(&conv_id) {
        conv.expiration_timer = seconds;
    }
    app.db_warn_visible(
        app.db.update_expiration_timer(&conv_id, seconds),
        "update_expiration_timer",
    );
    Some(SendRequest::UpdateExpiration {
        conv_id,
        is_group,
        seconds,
    })
}

fn create_poll(
    app: &mut App,
    question: String,
    options: Vec<String>,
    allow_multiple: bool,
) -> Option<SendRequest> {
    let Some(conv_id) = app.active_conversation.clone() else {
        app.status_message = "No active conversation".to_string();
        return None;
    };
    let is_group = app
        .store
        .conversations
        .get(&conv_id)
        .map(|c| c.is_group)
        .unwrap_or(false);
    let now = Utc::now();
    let local_ts_ms = now.timestamp_millis();

    let poll_options: Vec<PollOption> = options
        .iter()
        .enumerate()
        .map(|(i, text)| PollOption {
            id: i as i64,
            text: text.clone(),
        })
        .collect();
    let poll_data = PollData {
        question: question.clone(),
        options: poll_options,
        allow_multiple,
        closed: false,
    };

    let poll_data_for_db = poll_data.clone();
    let body = format!("\u{1F4CA} {question}");
    let poll_msg = DisplayMessage {
        sender: "you".to_string(),
        timestamp: now,
        body,
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
        expires_in_seconds: 0,
        expiration_start_ms: 0,
        poll_data: Some(poll_data),
        poll_votes: Vec::new(),
        preview: None,
        preview_image_lines: None,
        preview_image_path: None,
    };
    app.on_message_added(&conv_id, poll_msg, WireQuote::default(), false);
    app.db_warn_visible(
        app.db
            .upsert_poll_data(&conv_id, local_ts_ms, &poll_data_for_db),
        "upsert_poll_data",
    );

    app.scroll.offset = 0;
    Some(SendRequest::PollCreate {
        recipient: conv_id,
        is_group,
        question,
        options,
        allow_multiple,
        local_ts_ms,
    })
}
