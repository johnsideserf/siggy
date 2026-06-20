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
