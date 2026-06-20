//! Overlay key and action handlers extracted from `App`.
//!
//! These are user-initiated actions on existing messages -- pinning, unpinning,
//! and voting in polls. They sit alongside `handlers/input.rs` (composer text
//! dispatch) and `handlers/signal.rs` (signal-cli event dispatch). Splitting
//! them out lets `handlers::signal::handle_system_message` and
//! `handlers::signal::handle_poll_vote` return to private visibility -- those
//! calls are now internal to `handlers::keys` rather than crossing the
//! `app.rs` boundary.

use chrono::Utc;
use crossterm::event::KeyCode;

use crate::app::{App, InputMode, OverlayKind, PIN_DURATIONS, PinPending, SendRequest};
use crate::list_overlay::{ListKeyAction, classify_list_key};

// ---------------------------------------------------------------------------
// Shared message actions (#493)
//
// Each action below is reachable three ways: a Normal-mode key, an
// Insert-mode key (in alternative keybinding profiles), and the per-message
// action menu. They were previously copy-pasted at all three call sites with
// subtle drift; these are the single implementations. All return
// Option<SendRequest> so call-site match arms stay single expressions, even
// though most never dispatch anything.
//
// Mode note: actions that move the user into the composer set
// `mode = InputMode::Insert` unconditionally. From Normal mode and the
// action menu that is the intended transition; from Insert mode it is an
// idempotent no-op, which is why one implementation serves all three sites.
// ---------------------------------------------------------------------------

/// Start a quoted reply to the focused message and enter the composer.
pub(crate) fn execute_reply(app: &mut App) -> Option<SendRequest> {
    if let Some(msg) = app.selected_message()
        && !msg.is_system
        && !msg.is_deleted
    {
        let phone = msg.route_author(&app.account).to_string();
        let snippet: String = if msg.body.chars().count() > 50 {
            format!("{}…", msg.body.chars().take(50).collect::<String>())
        } else {
            msg.body.clone()
        };
        let ts = msg.timestamp_ms;
        app.reply_target = Some((phone, snippet, ts));
        app.mode = InputMode::Insert;
    }
    None
}

/// Start editing the focused message (own, non-deleted messages only) with
/// its body pre-loaded into the composer.
pub(crate) fn execute_edit(app: &mut App) -> Option<SendRequest> {
    if let Some(msg) = app.selected_message()
        && msg.is_outgoing()
        && !msg.is_deleted
        && !msg.is_system
    {
        let ts = msg.timestamp_ms;
        let body = msg.body.clone();
        if let Some(ref conv_id) = app.active_conversation {
            let conv_id = conv_id.clone();
            app.editing_message = Some((ts, conv_id));
            app.input.buffer = body;
            app.input.cursor = app.input.buffer.len();
            app.mode = InputMode::Insert;
        }
    }
    None
}

/// Open the reaction picker for the focused message.
pub(crate) fn execute_react(app: &mut App) -> Option<SendRequest> {
    if app.selected_message().is_some_and(|m| !m.is_system) {
        app.open_overlay(OverlayKind::ReactionPicker);
        app.reactions.picker_index = 0;
    }
    None
}

/// Open the forward-to-conversation picker for the focused message.
pub(crate) fn execute_forward(app: &mut App) -> Option<SendRequest> {
    if let Some(msg) = app.selected_message()
        && !msg.is_system
        && !msg.is_deleted
    {
        app.forward.body = msg.body.clone();
        app.open_forward_picker();
    }
    None
}

/// Open the delete-confirmation overlay for the focused message.
pub(crate) fn execute_delete_confirm(app: &mut App) -> Option<SendRequest> {
    if let Some(msg) = app.selected_message()
        && !msg.is_system
        && !msg.is_deleted
    {
        app.open_overlay(OverlayKind::DeleteConfirm);
    }
    None
}

/// Jump to the next (`forward`) or previous search result, if any.
pub(crate) fn execute_search_jump(app: &mut App, forward: bool) -> Option<SendRequest> {
    if !app.search.results.is_empty() {
        app.jump_to_search_result(forward);
    }
    None
}

/// Open the per-message action menu for the focused message.
pub(crate) fn execute_open_action_menu(app: &mut App) -> Option<SendRequest> {
    if app.selected_message().is_some_and(|m| !m.is_system) {
        app.open_overlay(OverlayKind::ActionMenu);
        app.action_menu.index = 0;
    }
    None
}

/// Toggle the pinned state of the currently focused message. For unpinning,
/// this runs the local update immediately. For pinning, it opens the duration
/// picker overlay and defers the actual pin until the user selects a duration.
pub(crate) fn execute_pin_toggle(app: &mut App) -> Option<SendRequest> {
    let msg = app.selected_message()?;
    if msg.is_system || msg.is_deleted {
        return None;
    }
    let was_pinned = msg.is_pinned;
    let target_timestamp = msg.timestamp_ms;
    let target_author = msg.route_author(&app.account).to_string();
    let conv_id = app.active_conversation.clone()?;
    let is_group = app
        .store
        .conversations
        .get(&conv_id)
        .map(|c| c.is_group)
        .unwrap_or(false);

    if was_pinned {
        // Unpin immediately -- no duration needed.
        if let Some(conv) = app.store.conversations.get_mut(&conv_id)
            && let Some(idx) = conv.find_msg_idx(target_timestamp)
        {
            conv.messages[idx].is_pinned = false;
        }
        app.db_warn_visible(
            app.db.set_message_pinned(&conv_id, target_timestamp, false),
            "set_message_pinned",
        );
        app.scroll.offset = 0;
        app.scroll.focused_index = None;
        let body = "you unpinned a message";
        let now = Utc::now();
        let now_ms = now.timestamp_millis();
        super::signal::handle_system_message(app, &conv_id, body, now, now_ms);
        Some(SendRequest::Unpin {
            recipient: conv_id,
            is_group,
            target_author,
            target_timestamp,
        })
    } else {
        // Open pin duration picker.
        app.pin_duration.pending = Some(PinPending {
            conv_id,
            is_group,
            target_author,
            target_timestamp,
        });
        app.open_overlay(OverlayKind::PinDuration);
        app.pin_duration.index = 0;
        None
    }
}

/// Handle a key press while the pin duration picker overlay is open.
pub fn handle_pin_duration_key(app: &mut App, code: KeyCode) -> Option<SendRequest> {
    let action = classify_list_key(code, false);
    if crate::list_overlay::apply_nav(&action, &mut app.pin_duration.index, PIN_DURATIONS.len()) {
        return None;
    }
    match action {
        ListKeyAction::Select => {
            let duration = PIN_DURATIONS[app.pin_duration.index].0;
            app.close_overlay();
            let pending = app.pin_duration.pending.take()?;

            // Optimistically pin.
            if let Some(conv) = app.store.conversations.get_mut(&pending.conv_id)
                && let Some(idx) = conv.find_msg_idx(pending.target_timestamp)
            {
                conv.messages[idx].is_pinned = true;
            }
            app.db_warn_visible(
                app.db
                    .set_message_pinned(&pending.conv_id, pending.target_timestamp, true),
                "set_message_pinned",
            );
            app.scroll.offset = 0;
            app.scroll.focused_index = None;
            let body = "you pinned a message";
            let now = Utc::now();
            let now_ms = now.timestamp_millis();
            super::signal::handle_system_message(app, &pending.conv_id, body, now, now_ms);

            Some(SendRequest::Pin {
                recipient: pending.conv_id,
                is_group: pending.is_group,
                target_author: pending.target_author,
                target_timestamp: pending.target_timestamp,
                pin_duration: duration,
            })
        }
        ListKeyAction::Close => {
            app.close_overlay();
            app.pin_duration.pending = None;
            None
        }
        _ => None,
    }
}

/// Handle a key press while the poll vote overlay is open.
pub fn handle_poll_vote_key(app: &mut App, code: KeyCode) -> Option<SendRequest> {
    let (option_count, allow_multiple) = {
        let pending = app.poll_vote.pending.as_ref()?;
        (pending.options.len(), pending.allow_multiple)
    };
    let action = classify_list_key(code, false);
    if crate::list_overlay::apply_nav(&action, &mut app.poll_vote.index, option_count) {
        return None;
    }
    match code {
        KeyCode::Char(' ') => {
            if allow_multiple {
                if let Some(sel) = app.poll_vote.selections.get_mut(app.poll_vote.index) {
                    *sel = !*sel;
                }
            } else {
                // Single select: clear all, select current.
                for sel in &mut app.poll_vote.selections {
                    *sel = false;
                }
                if let Some(sel) = app.poll_vote.selections.get_mut(app.poll_vote.index) {
                    *sel = true;
                }
            }
            None
        }
        KeyCode::Enter => {
            let selected: Vec<i64> = app
                .poll_vote
                .selections
                .iter()
                .enumerate()
                .filter(|&(_, &sel)| sel)
                .map(|(i, _)| i as i64)
                .collect();
            if selected.is_empty() {
                return None;
            }
            let pending = app.poll_vote.pending.take()?;
            app.close_overlay();

            // Optimistic local vote.
            let voter = app.account.clone();
            super::signal::handle_poll_vote(
                app,
                &pending.conv_id,
                pending.poll_timestamp,
                &voter,
                None,
                &selected,
                1,
            );

            Some(SendRequest::PollVote {
                recipient: pending.conv_id,
                is_group: pending.is_group,
                poll_author: pending.poll_author,
                poll_timestamp: pending.poll_timestamp,
                option_indexes: selected,
                vote_count: 1,
            })
        }
        KeyCode::Esc => {
            app.close_overlay();
            app.poll_vote.pending = None;
            None
        }
        _ => None,
    }
}

/// Handle a key press while the forward-message picker overlay is open.
pub fn handle_forward_key(app: &mut App, code: KeyCode) -> Option<SendRequest> {
    let action = classify_list_key(code, true);
    if crate::list_overlay::apply_nav(&action, &mut app.forward.index, app.forward.filtered.len()) {
        return None;
    }
    match action {
        ListKeyAction::Select => {
            if let Some((conv_id, name)) = app.forward.filtered.get(app.forward.index).cloned() {
                let is_group = app
                    .store
                    .conversations
                    .get(&conv_id)
                    .map(|c| c.is_group)
                    .unwrap_or(false);
                let body = format!("[Forwarded]\n{}", app.forward.body);
                let local_ts_ms = chrono::Utc::now().timestamp_millis();
                app.close_overlay();
                app.status_message = format!("Forwarded to {name}");
                app.store.move_conversation_to_top(&conv_id);
                return Some(SendRequest::Message {
                    recipient: conv_id,
                    body,
                    is_group,
                    local_ts_ms,
                    mentions: Vec::new(),
                    attachment: None,
                    quote_timestamp: None,
                    quote_author: None,
                    quote_body: None,
                });
            }
        }
        ListKeyAction::Close => {
            app.close_overlay();
        }
        ListKeyAction::FilterPush(c) => {
            if !c.is_control() {
                app.forward.filter.push(c);
                app.update_forward_filter();
            }
        }
        ListKeyAction::FilterPop => {
            app.forward.filter.pop();
            app.update_forward_filter();
        }
        ListKeyAction::None | ListKeyAction::Up | ListKeyAction::Down => {}
    }
    None
}

/// Handle a key press while the contacts picker overlay is open.
pub fn handle_contacts_key(app: &mut App, code: KeyCode) {
    let action = classify_list_key(code, true);
    if crate::list_overlay::apply_nav(
        &action,
        &mut app.contacts_overlay.index,
        app.contacts_overlay.filtered.len(),
    ) {
        return;
    }
    match action {
        ListKeyAction::Select => {
            if let Some((number, _)) = app
                .contacts_overlay
                .filtered
                .get(app.contacts_overlay.index)
            {
                let number = number.clone();
                app.close_overlay();
                app.contacts_overlay.filter.clear();
                app.join_conversation(&number);
            }
        }
        ListKeyAction::Close => {
            app.close_overlay();
            app.contacts_overlay.filter.clear();
        }
        ListKeyAction::FilterPush(c) => {
            app.contacts_overlay.filter.push(c);
            app.refresh_contacts_filter();
        }
        ListKeyAction::FilterPop => {
            app.contacts_overlay.filter.pop();
            app.refresh_contacts_filter();
        }
        ListKeyAction::None | ListKeyAction::Up | ListKeyAction::Down => {}
    }
}

/// Handle a key press in the message delete confirmation overlay.
pub fn handle_delete_confirm_key(app: &mut App, code: KeyCode) -> Option<SendRequest> {
    match code {
        KeyCode::Char('y') => {
            app.close_overlay();
            let conv_id = app.active_conversation.clone()?;
            let conv = app.store.conversations.get(&conv_id)?;
            let is_group = conv.is_group;
            let index = app
                .scroll
                .focused_index
                .unwrap_or_else(|| conv.messages.len().saturating_sub(1));
            let msg = conv.messages.get(index)?;
            let is_outgoing = msg.is_outgoing();
            let target_timestamp = msg.timestamp_ms;

            // Apply local delete
            let conv = app.store.conversations.get_mut(&conv_id)?;
            let msg = conv.messages.get_mut(index)?;
            msg.is_deleted = true;
            msg.body = "[deleted]".to_string();
            msg.reactions.clear();
            app.db_warn_visible(
                app.db.mark_message_deleted(&conv_id, target_timestamp),
                "mark_message_deleted",
            );

            // Send remote delete only for outgoing messages
            if is_outgoing {
                return Some(SendRequest::RemoteDelete {
                    recipient: conv_id,
                    is_group,
                    target_timestamp,
                });
            }
            None
        }
        KeyCode::Char('l') => {
            // Local-only delete (for outgoing messages)
            app.close_overlay();
            let conv_id = app.active_conversation.clone()?;
            let conv = app.store.conversations.get(&conv_id)?;
            let index = app
                .scroll
                .focused_index
                .unwrap_or_else(|| conv.messages.len().saturating_sub(1));
            let msg = conv.messages.get(index)?;
            let target_timestamp = msg.timestamp_ms;

            let conv = app.store.conversations.get_mut(&conv_id)?;
            let msg = conv.messages.get_mut(index)?;
            msg.is_deleted = true;
            msg.body = "[deleted]".to_string();
            msg.reactions.clear();
            app.db_warn_visible(
                app.db.mark_message_deleted(&conv_id, target_timestamp),
                "mark_message_deleted",
            );
            None
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.close_overlay();
            None
        }
        _ => None,
    }
}

/// Handle a key press in the delete-conversation confirmation overlay.
pub fn handle_delete_conversation_confirm_key(app: &mut App, code: KeyCode) -> Option<SendRequest> {
    match code {
        KeyCode::Char('y') => {
            app.close_overlay();
            app.delete_active_conversation()
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.close_overlay();
            None
        }
        _ => None,
    }
}

/// Handle a key press while the safety-number verify overlay is open.
pub fn handle_verify_key(app: &mut App, code: KeyCode) -> Option<SendRequest> {
    let action = classify_list_key(code, false);
    // Any navigation cancels a pending confirmation, matching the original
    // per-key reset.
    if crate::list_overlay::apply_nav(&action, &mut app.verify.index, app.verify.identities.len()) {
        app.verify.confirming = false;
        return None;
    }
    if action == ListKeyAction::Close {
        app.verify.confirming = false;
        app.close_overlay();
        return None;
    }
    // 'v' and Enter both trigger verification; every other key cancels a
    // pending confirm.
    if action == ListKeyAction::Select || code == KeyCode::Char('v') {
        if let Some(id) = app.verify.identities.get(app.verify.index) {
            if id.safety_number.is_empty() {
                app.status_message = "Safety number not available — cannot verify".to_string();
                return None;
            }
            if app.verify.confirming {
                // Second press: actually trust with the specific safety number
                if let Some(ref number) = id.number {
                    let recipient = number.clone();
                    let safety_number = id.safety_number.clone();
                    app.verify.confirming = false;
                    return Some(SendRequest::TrustIdentity {
                        recipient,
                        safety_number,
                    });
                }
            } else {
                // First press: ask for confirmation
                app.verify.confirming = true;
            }
        }
    } else {
        app.verify.confirming = false;
    }
    None
}

/// Handle a key press while the profile editor overlay is open.
pub fn handle_profile_key(app: &mut App, code: KeyCode) -> Option<SendRequest> {
    const FIELD_COUNT: usize = 4;
    const SAVE_INDEX: usize = FIELD_COUNT;

    if app.profile.editing {
        // Editing a field
        match code {
            KeyCode::Esc => {
                // Cancel edit, discard buffer
                app.profile.editing = false;
            }
            KeyCode::Enter => {
                // Confirm edit, write buffer back to field
                app.profile.fields[app.profile.index] = app.profile.edit_buffer.clone();
                app.profile.editing = false;
            }
            KeyCode::Backspace => {
                app.profile.edit_buffer.pop();
            }
            KeyCode::Char(c) => {
                app.profile.edit_buffer.push(c);
            }
            _ => {}
        }
        return None;
    }

    // Navigation mode
    let action = classify_list_key(code, false);
    // The list is the editable fields plus the Save button (SAVE_INDEX).
    if crate::list_overlay::apply_nav(&action, &mut app.profile.index, SAVE_INDEX + 1) {
        return None;
    }
    match code {
        KeyCode::Enter => {
            if app.profile.index < FIELD_COUNT {
                // Start editing the selected field
                app.profile.editing = true;
                app.profile.edit_buffer = app.profile.fields[app.profile.index].clone();
            } else {
                // Save button
                let [given_name, family_name, about, about_emoji] = app.profile.fields.clone();
                if given_name.trim().is_empty() {
                    app.status_message = "Given name is required".to_string();
                    return None;
                }
                app.close_overlay();
                return Some(SendRequest::UpdateProfile {
                    given_name,
                    family_name,
                    about,
                    about_emoji,
                });
            }
        }
        KeyCode::Esc => {
            app.close_overlay();
        }
        _ => {}
    }
    None
}
