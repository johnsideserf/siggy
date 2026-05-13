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

use crate::app::{App, OverlayKind, PIN_DURATIONS, PinPending, SendRequest};
use crate::list_overlay::{ListKeyAction, classify_list_key};

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
    let author_phone = msg.sender_id.clone();
    let conv_id = app.active_conversation.clone()?;
    let is_group = app
        .store
        .conversations
        .get(&conv_id)
        .map(|c| c.is_group)
        .unwrap_or(false);

    let target_author = if author_phone.is_empty() || author_phone == "you" {
        app.account.clone()
    } else {
        author_phone
    };

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
    match classify_list_key(code, false) {
        ListKeyAction::Down => {
            if app.pin_duration.index < PIN_DURATIONS.len() - 1 {
                app.pin_duration.index += 1;
            }
            None
        }
        ListKeyAction::Up => {
            app.pin_duration.index = app.pin_duration.index.saturating_sub(1);
            None
        }
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
    let pending = app.poll_vote.pending.as_ref()?;
    let option_count = pending.options.len();
    match code {
        KeyCode::Char('j') | KeyCode::Down => {
            if app.poll_vote.index < option_count.saturating_sub(1) {
                app.poll_vote.index += 1;
            }
            None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.poll_vote.index = app.poll_vote.index.saturating_sub(1);
            None
        }
        KeyCode::Char(' ') => {
            let allow_multiple = pending.allow_multiple;
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
