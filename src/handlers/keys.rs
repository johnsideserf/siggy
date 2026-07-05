//! Overlay key and action handlers extracted from `App`.
//!
//! These are user-initiated actions on existing messages -- pinning, unpinning,
//! and voting in polls. They sit alongside `handlers/input.rs` (composer text
//! dispatch) and `handlers/signal.rs` (signal-cli event dispatch). Splitting
//! them out lets `handlers::signal::handle_system_message` and
//! `handlers::signal::handle_poll_vote` return to private visibility -- those
//! calls are now internal to `handlers::keys` rather than crossing the
//! `app.rs` boundary.

use std::time::Instant;

use chrono::Utc;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::app::{
    ActionMenuHint, App, GroupMenuHint, GroupMenuState, InputMode, OverlayKind, PIN_DURATIONS,
    PinPending, QUICK_REACTIONS, SETTINGS, SETTINGS_VISUAL_ORDER, SendRequest, floor_char_boundary,
    is_in_rect,
};
use crate::autocomplete::AutocompleteMode;
use crate::domain::{EmojiPickerAction, EmojiPickerSource};
use crate::input::{next_char_pos, prev_char_pos};
use crate::keybindings::{self, BindingMode, KeyAction};
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

/// Handle a mouse event: scroll the message list, translate scroll to j/k while
/// an overlay is open, and route left-clicks to links / sidebar / composer.
pub fn handle_mouse_event(app: &mut App, event: MouseEvent) -> Option<SendRequest> {
    if !app.mouse.enabled {
        return None;
    }

    // When overlays are open, translate scroll to j/k navigation and Esc on outside click
    if app.has_overlay() {
        match event.kind {
            MouseEventKind::ScrollUp => app.handle_overlay_key(KeyCode::Char('k')),
            MouseEventKind::ScrollDown => app.handle_overlay_key(KeyCode::Char('j')),
            _ => (false, None),
        };
        return None;
    }

    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            handle_left_click(app, event.column, event.row);
        }
        MouseEventKind::ScrollUp
            if is_in_rect(event.column, event.row, app.mouse.messages_area) =>
        {
            app.sync.user_scrolled = true;
            app.scroll.offset = app.scroll.offset.saturating_add(3);
            app.scroll.focused_index = None;
        }
        MouseEventKind::ScrollDown
            if is_in_rect(event.column, event.row, app.mouse.messages_area) =>
        {
            app.sync.user_scrolled = true;
            app.scroll.offset = app.scroll.offset.saturating_sub(3);
            app.scroll.focused_index = None;
        }
        _ => {}
    }
    None
}

fn handle_left_click(app: &mut App, col: u16, row: u16) {
    // 1. Check link regions first (highest priority -- links overlay everything)
    for link in &app.image.link_regions {
        if row == link.y && col >= link.x && col < link.x + link.width {
            let url = link.url.clone();
            app.open_url(&url);
            return;
        }
    }

    // 2. Sidebar click -- switch conversation
    if let Some(inner) = app.mouse.sidebar_inner
        && is_in_rect(col, row, inner)
    {
        let index = (row - inner.y) as usize;
        // Use the exact order the sidebar rendered last frame (stale and
        // archived conversations are hidden from the normal view, so indexing
        // conversation_order directly would select the wrong row).
        let sidebar_list = if !app.mouse.sidebar_display_order.is_empty() {
            &app.mouse.sidebar_display_order
        } else if app.is_overlay(OverlayKind::SidebarFilter) && !app.sidebar_filtered.is_empty() {
            &app.sidebar_filtered
        } else {
            &app.store.conversation_order
        };
        if index < sidebar_list.len() {
            let conv_id = sidebar_list[index].clone();
            app.clear_sidebar_filter();
            app.join_conversation(&conv_id);
        }
        return;
    }

    // 3. Input area click -- position cursor and enter Insert mode
    if is_in_rect(col, row, app.mouse.input_area) {
        app.mode = InputMode::Insert;
        // Content starts after left border (1) + prefix
        let content_start_col = app.mouse.input_area.x + 1 + app.mouse.input_prefix_len;
        if col >= content_start_col {
            let text_width = (app.mouse.input_area.width.saturating_sub(2)) as usize
                - app.mouse.input_prefix_len as usize;
            let input_scroll = floor_char_boundary(
                &app.input.buffer,
                app.input.cursor.saturating_sub(text_width),
            );
            let target_col = (col - content_start_col) as usize;
            // Walk characters to find the byte offset for the target column
            let mut byte_pos = input_scroll;
            for (col_pos, ch) in app.input.buffer[input_scroll..].chars().enumerate() {
                if col_pos >= target_col {
                    break;
                }
                byte_pos += ch.len_utf8();
            }
            app.input.cursor = byte_pos.min(app.input.buffer.len());
        } else {
            app.input.cursor = 0;
        }
    }
}

/// Handle an Insert-mode key. Edits the composer and drives typing-indicator
/// dispatch; alternative keybinding profiles may also bind message actions here.
pub fn handle_insert_key(
    app: &mut App,
    modifiers: KeyModifiers,
    code: KeyCode,
) -> Option<SendRequest> {
    match app
        .keybindings
        .resolve(modifiers, code, BindingMode::Insert)
    {
        Some(KeyAction::ExitInsert) => {
            app.mode = InputMode::Normal;
            app.pending_normal_key = None; // defensive reset
            app.close_overlay();
            app.reply_target = None;
            app.editing_message = None;
            if app.typing.reset() {
                return app.build_typing_request(true);
            }
            None
        }
        Some(KeyAction::InsertNewline) => {
            app.input.buffer.insert(app.input.cursor, '\n');
            app.input.cursor += 1;
            app.close_overlay();
            app.typing.last_keypress = Some(Instant::now());
            if !app.typing.sent
                && !app.input.buffer.starts_with('/')
                && app
                    .active_conversation
                    .as_ref()
                    .is_some_and(|id| !app.blocked_conversations.contains(id))
            {
                app.typing.sent = true;
                return app.build_typing_request(false);
            }
            None
        }
        Some(KeyAction::SendMessage) => {
            let was_typing = app.typing.reset();
            let result = app.handle_input();
            if result.is_some() {
                result
            } else if was_typing {
                app.build_typing_request(true)
            } else {
                None
            }
        }
        Some(KeyAction::DeleteWordBack) => {
            app.delete_word_back();
            None
        }
        // Actions that alternative profiles (Emacs/Minimal) may bind in Insert mode
        Some(KeyAction::ScrollDown) => {
            app.sync.user_scrolled = true;
            app.scroll.offset = app.scroll.offset.saturating_sub(1);
            app.scroll.focused_index = None;
            None
        }
        Some(KeyAction::ScrollUp) => {
            app.sync.user_scrolled = true;
            app.scroll.offset = app.scroll.offset.saturating_add(1);
            app.scroll.focused_index = None;
            None
        }
        Some(KeyAction::CursorLeft) => {
            app.input.cursor = prev_char_pos(&app.input.buffer, app.input.cursor);
            None
        }
        Some(KeyAction::CursorRight) => {
            app.input.cursor = next_char_pos(&app.input.buffer, app.input.cursor);
            None
        }
        Some(KeyAction::LineStart) => {
            app.input.cursor = app.current_line_start();
            None
        }
        Some(KeyAction::LineEnd) => {
            app.input.cursor = app.current_line_end();
            None
        }
        Some(KeyAction::DeleteChar) => {
            if app.input.cursor < app.input.buffer.len() {
                app.input.buffer.remove(app.input.cursor);
            }
            None
        }
        Some(KeyAction::DeleteToEnd) => {
            let line_end = app.current_line_end();
            app.input.buffer.drain(app.input.cursor..line_end);
            None
        }
        Some(KeyAction::CopyMessage) => {
            app.copy_selected_message(false);
            None
        }
        Some(KeyAction::CopyAllMessages) => {
            app.copy_selected_message(true);
            None
        }
        Some(KeyAction::React) => execute_react(app),
        Some(KeyAction::Quote) => execute_reply(app),
        Some(KeyAction::EditMessage) => execute_edit(app),
        Some(KeyAction::ForwardMessage) => execute_forward(app),
        Some(KeyAction::DeleteMessage) => execute_delete_confirm(app),
        Some(KeyAction::NextSearchResult) => execute_search_jump(app, true),
        Some(KeyAction::PrevSearchResult) => execute_search_jump(app, false),
        Some(KeyAction::OpenActionMenu) => execute_open_action_menu(app),
        Some(KeyAction::PinMessage) => execute_pin_toggle(app),
        Some(KeyAction::JumpToQuote) => {
            app.jump_to_quote();
            None
        }
        Some(KeyAction::JumpBack) => {
            app.jump_back();
            None
        }
        _ => {
            let needs_ac_update = matches!(
                code,
                KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_)
            );
            app.apply_input_edit(code);
            if needs_ac_update {
                app.update_autocomplete();
            }
            if matches!(
                code,
                KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete
            ) {
                app.typing.last_keypress = Some(Instant::now());
                if app.input.buffer.is_empty() && app.typing.sent {
                    app.typing.sent = false;
                    app.typing.last_keypress = None;
                    return app.build_typing_request(true);
                }
                if !app.typing.sent
                    && !app.input.buffer.is_empty()
                    && !app.input.buffer.starts_with('/')
                    && app
                        .active_conversation
                        .as_ref()
                        .is_some_and(|id| !app.blocked_conversations.contains(id))
                {
                    app.typing.sent = true;
                    return app.build_typing_request(false);
                }
            }
            None
        }
    }
}

/// Handle a Normal-mode key. Dispatches to scroll, cursor/edit, or message
/// action sub-behaviors, and tracks the `g`/`d` prefix for `gg`/`dd` sequences.
pub fn handle_normal_key(
    app: &mut App,
    modifiers: KeyModifiers,
    code: KeyCode,
) -> Option<SendRequest> {
    // Handle pending prefix key (gg, dd sequences)
    if let Some(prev) = app.pending_normal_key.take() {
        match (prev, code) {
            ('g', KeyCode::Char('g')) => {
                // gg = scroll to top
                if let Some(ref id) = app.active_conversation
                    && let Some(conv) = app.store.conversations.get(id)
                {
                    app.scroll.offset = conv.messages.len();
                }
                app.scroll.focused_index = None;
                return None;
            }
            ('d', KeyCode::Char('d')) => {
                // dd = delete message
                return execute_delete_confirm(app);
            }
            (_, KeyCode::Esc) => {
                // Esc cancels pending prefix
                return None;
            }
            _ => {
                // Not a valid sequence -- fall through to process this key normally
            }
        }
    }

    match app
        .keybindings
        .resolve(modifiers, code, BindingMode::Normal)
    {
        // Scroll
        Some(KeyAction::ScrollDown) => {
            app.sync.user_scrolled = true;
            app.scroll.offset = app.scroll.offset.saturating_sub(1);
            app.scroll.focused_index = None;
            None
        }
        Some(KeyAction::ScrollUp) => {
            app.sync.user_scrolled = true;
            app.scroll.offset = app.scroll.offset.saturating_add(1);
            app.scroll.focused_index = None;
            None
        }
        Some(KeyAction::FocusNextMessage) => {
            app.sync.user_scrolled = true;
            app.jump_to_adjacent_message(false);
            None
        }
        Some(KeyAction::FocusPrevMessage) => {
            app.sync.user_scrolled = true;
            app.jump_to_adjacent_message(true);
            None
        }
        Some(KeyAction::HalfPageDown) => {
            app.sync.user_scrolled = true;
            app.scroll.offset = app.scroll.offset.saturating_sub(10);
            app.scroll.focused_index = None;
            None
        }
        Some(KeyAction::HalfPageUp) => {
            app.sync.user_scrolled = true;
            app.scroll.offset = app.scroll.offset.saturating_add(10);
            app.scroll.focused_index = None;
            None
        }
        Some(KeyAction::ScrollToBottom) => {
            app.sync.user_scrolled = true;
            app.scroll.offset = 0;
            app.scroll.focused_index = None;
            None
        }
        // Edit/mode-switch
        Some(KeyAction::InsertAtCursor) => {
            app.mode = InputMode::Insert;
            None
        }
        Some(KeyAction::InsertAfterCursor) => {
            app.input.cursor = next_char_pos(&app.input.buffer, app.input.cursor);
            app.mode = InputMode::Insert;
            None
        }
        Some(KeyAction::InsertLineStart) => {
            app.input.cursor = app.current_line_start();
            app.mode = InputMode::Insert;
            None
        }
        Some(KeyAction::InsertLineEnd) => {
            app.input.cursor = app.current_line_end();
            app.mode = InputMode::Insert;
            None
        }
        Some(KeyAction::OpenLineBelow) => {
            let line_end = app.current_line_end();
            app.input.cursor = line_end;
            app.input.buffer.insert(app.input.cursor, '\n');
            app.input.cursor += 1;
            app.mode = InputMode::Insert;
            None
        }
        Some(KeyAction::CursorLeft) => {
            app.input.cursor = prev_char_pos(&app.input.buffer, app.input.cursor);
            None
        }
        Some(KeyAction::CursorRight) => {
            app.input.cursor = next_char_pos(&app.input.buffer, app.input.cursor);
            None
        }
        Some(KeyAction::LineStart) => {
            app.input.cursor = app.current_line_start();
            None
        }
        Some(KeyAction::LineEnd) => {
            app.input.cursor = app.current_line_end();
            None
        }
        Some(KeyAction::WordForward) => {
            let buf = &app.input.buffer;
            let mut pos = app.input.cursor;
            while pos < buf.len() {
                let c = buf[pos..].chars().next().unwrap();
                if c.is_whitespace() {
                    break;
                }
                pos += c.len_utf8();
            }
            while pos < buf.len() {
                let c = buf[pos..].chars().next().unwrap();
                if !c.is_whitespace() {
                    break;
                }
                pos += c.len_utf8();
            }
            app.input.cursor = pos;
            None
        }
        Some(KeyAction::WordBack) => {
            let buf = &app.input.buffer;
            let mut pos = app.input.cursor;
            while pos > 0 {
                let prev = buf[..pos].chars().next_back().unwrap();
                if !prev.is_whitespace() {
                    break;
                }
                pos -= prev.len_utf8();
            }
            while pos > 0 {
                let prev = buf[..pos].chars().next_back().unwrap();
                if prev.is_whitespace() {
                    break;
                }
                pos -= prev.len_utf8();
            }
            app.input.cursor = pos;
            None
        }
        Some(KeyAction::DeleteChar) => {
            if app.input.cursor < app.input.buffer.len() {
                app.input.buffer.remove(app.input.cursor);
                if app.input.cursor > 0 && app.input.cursor >= app.input.buffer.len() {
                    app.input.cursor = prev_char_pos(&app.input.buffer, app.input.buffer.len());
                }
            }
            None
        }
        Some(KeyAction::DeleteToEnd) => {
            let line_end = app.current_line_end();
            app.input.buffer.drain(app.input.cursor..line_end);
            None
        }
        Some(KeyAction::StartSearch) => {
            app.input.buffer = "/".to_string();
            app.input.cursor = 1;
            app.mode = InputMode::Insert;
            app.update_autocomplete();
            None
        }
        Some(KeyAction::SidebarSearch) => {
            app.sidebar_visible = true;
            app.open_overlay(OverlayKind::SidebarFilter);
            app.sidebar_filter.clear();
            app.sidebar_filtered.clear();
            None
        }
        Some(KeyAction::ClearInput) => {
            if !app.input.buffer.is_empty() {
                app.input.buffer.clear();
                app.input.cursor = 0;
                app.autocomplete.pending_mentions.clear();
            }
            None
        }
        // Actions
        Some(KeyAction::CopyMessage) => {
            app.copy_selected_message(false);
            None
        }
        Some(KeyAction::CopyAllMessages) => {
            app.copy_selected_message(true);
            None
        }
        Some(KeyAction::React) => execute_react(app),
        Some(KeyAction::Quote) => execute_reply(app),
        Some(KeyAction::EditMessage) => execute_edit(app),
        Some(KeyAction::ForwardMessage) => execute_forward(app),
        Some(KeyAction::NextSearchResult) => execute_search_jump(app, true),
        Some(KeyAction::PrevSearchResult) => execute_search_jump(app, false),
        Some(KeyAction::OpenActionMenu) => execute_open_action_menu(app),
        Some(KeyAction::PinMessage) => execute_pin_toggle(app),
        Some(KeyAction::JumpToQuote) => {
            app.jump_to_quote();
            None
        }
        Some(KeyAction::JumpBack) => {
            app.jump_back();
            None
        }
        _ => {
            // Handle prefix keys that aren't in the binding map
            if let KeyCode::Char(c @ ('g' | 'd')) = code
                && modifiers.is_empty()
            {
                app.pending_normal_key = Some(c);
            }
            None
        }
    }
}

/// Handle a global keybinding (active in both Normal and Insert mode). Returns
/// true if the key was consumed. When locked, all keys route to the lock screen.
pub fn handle_global_key(app: &mut App, modifiers: KeyModifiers, code: KeyCode) -> bool {
    if app.lock.is_locked() {
        return app.handle_lock_key(code);
    }
    let action = app
        .keybindings
        .resolve(modifiers, code, BindingMode::Global);
    if app.quit_confirm && !matches!(action, Some(KeyAction::Quit)) {
        app.quit_confirm = false;
        app.update_status();
    }
    match action {
        Some(KeyAction::Quit) => {
            if app.input.buffer.is_empty() || app.quit_confirm {
                app.should_quit = true;
            } else {
                app.quit_confirm = true;
            }
            true
        }
        Some(KeyAction::NextConversation) if !app.is_overlay(OverlayKind::Autocomplete) => {
            app.next_conversation();
            true
        }
        Some(KeyAction::PrevConversation) => {
            app.prev_conversation();
            true
        }
        Some(KeyAction::ResizeSidebarLeft) => {
            app.resize_sidebar(-2);
            true
        }
        Some(KeyAction::ResizeSidebarRight) => {
            app.resize_sidebar(2);
            true
        }
        Some(KeyAction::PageScrollUp) => {
            app.sync.user_scrolled = true;
            app.scroll.offset = app.scroll.offset.saturating_add(5);
            app.scroll.focused_index = None;
            true
        }
        Some(KeyAction::PageScrollDown) => {
            app.sync.user_scrolled = true;
            app.scroll.offset = app.scroll.offset.saturating_sub(5);
            app.scroll.focused_index = None;
            true
        }
        Some(KeyAction::SidebarSearch) => {
            app.sidebar_visible = true;
            app.open_overlay(OverlayKind::SidebarFilter);
            app.sidebar_filter.clear();
            app.sidebar_filtered.clear();
            true
        }
        Some(KeyAction::Lock) => {
            app.lock_now();
            true
        }
        // Overlay-open / toggle actions, mirroring the matching slash commands
        // so they can be driven from a keybinding (#202).
        Some(KeyAction::OpenContacts) => {
            app.open_overlay(OverlayKind::Contacts);
            app.contacts_overlay.index = 0;
            app.contacts_overlay.filter.clear();
            app.refresh_contacts_filter();
            true
        }
        Some(KeyAction::OpenSettings) => {
            app.open_overlay(OverlayKind::Settings);
            app.settings_overlay.index = 0;
            app.settings_overlay.mouse_snapshot = app.mouse.enabled;
            true
        }
        Some(KeyAction::OpenHelp) => {
            app.open_overlay(OverlayKind::Help);
            true
        }
        Some(KeyAction::ToggleSidebar) => {
            app.sidebar_visible = !app.sidebar_visible;
            true
        }
        Some(KeyAction::Attach) => {
            app.open_file_browser();
            true
        }
        _ => false,
    }
}

/// Route a key press to the handler for the currently active overlay. Returns
/// `(handled, maybe_send)` -- `handled` is false only when no overlay is open.
pub fn handle_overlay_key(app: &mut App, code: KeyCode) -> (bool, Option<SendRequest>) {
    let Some(kind) = app.active_overlay() else {
        return (false, None);
    };
    match kind {
        OverlayKind::SidebarFilter => {
            app.handle_sidebar_filter_key(code);
            (true, None)
        }
        OverlayKind::PollVote => {
            let send = app.handle_poll_vote_key(code);
            (true, send)
        }
        OverlayKind::PinDuration => {
            let send = app.handle_pin_duration_key(code);
            (true, send)
        }
        OverlayKind::ActionMenu => {
            let send = app.handle_action_menu_key(code);
            (true, send)
        }
        OverlayKind::DeleteConfirm => {
            let send = app.handle_delete_confirm_key(code);
            (true, send)
        }
        OverlayKind::DeleteConversationConfirm => {
            let send = app.handle_delete_conversation_confirm_key(code);
            (true, send)
        }
        OverlayKind::FilePicker => {
            app.handle_file_browser_key(code);
            (true, None)
        }
        OverlayKind::EmojiPicker => match app.emoji_picker.handle_key(code) {
            EmojiPickerAction::Select(emoji) => {
                let source = app.emoji_picker.source;
                app.emoji_picker.close();
                match source {
                    EmojiPickerSource::Input => {
                        app.input.buffer.insert_str(app.input.cursor, &emoji);
                        app.input.cursor += emoji.len();
                        (true, None)
                    }
                    EmojiPickerSource::Reaction => {
                        let send = app.prepare_reaction_send_emoji(&emoji);
                        (true, send)
                    }
                }
            }
            EmojiPickerAction::Close => {
                let was_reaction = app.emoji_picker.source == EmojiPickerSource::Reaction;
                app.emoji_picker.close();
                if was_reaction {
                    app.open_overlay(OverlayKind::ReactionPicker);
                }
                (true, None)
            }
            EmojiPickerAction::None => (true, None),
        },
        OverlayKind::ReactionPicker => {
            let send = app.handle_reaction_picker_key(code);
            (true, send)
        }
        OverlayKind::MessageRequest => {
            let send = app.handle_message_request_key(code);
            (true, send)
        }
        OverlayKind::GroupMenu => {
            let send = app.handle_group_menu_key(code);
            (true, send)
        }
        OverlayKind::About => {
            app.close_overlay();
            (true, None)
        }
        OverlayKind::Profile => {
            let send = app.handle_profile_key(code);
            (true, send)
        }
        OverlayKind::Help => {
            app.close_overlay();
            (true, None)
        }
        OverlayKind::Verify => {
            let send = app.handle_verify_key(code);
            (true, send)
        }
        OverlayKind::Forward => {
            let send = app.handle_forward_key(code);
            (true, send)
        }
        OverlayKind::Contacts => {
            app.handle_contacts_key(code);
            (true, None)
        }
        OverlayKind::Search => {
            app.handle_search_key(code);
            (true, None)
        }
        OverlayKind::SettingsProfiles => {
            app.handle_settings_profile_manager_key(code);
            (true, None)
        }
        OverlayKind::ThemePicker => {
            app.handle_theme_key(code);
            (true, None)
        }
        OverlayKind::Keybindings => {
            app.handle_keybindings_key(code);
            (true, None)
        }
        OverlayKind::Customize => {
            app.handle_customize_key(code);
            (true, None)
        }
        OverlayKind::Settings => {
            app.handle_settings_key(code);
            (true, None)
        }
        OverlayKind::Autocomplete => {
            let send = app.handle_autocomplete_key(code);
            (true, send)
        }
    }
}

/// Handle a key press on the session lock screen. Returns true to signal the
/// lock owns the key (it always does while the lock is up).
pub fn handle_lock_key(app: &mut App, code: KeyCode) -> bool {
    // Any char/Backspace clears the transient error so the user can retry.
    // Leave error visible on Enter so a submit failure stays on screen.
    if matches!(code, KeyCode::Char(_) | KeyCode::Backspace) {
        app.lock.error = None;
    }
    match code {
        KeyCode::Char(c) => {
            app.lock.input_buffer.push(c);
            true
        }
        KeyCode::Backspace => {
            app.lock.input_buffer.pop();
            true
        }
        KeyCode::Esc => {
            // Esc clears the current input but does not unlock or change phase.
            app.lock.input_buffer.clear();
            true
        }
        KeyCode::Enter => {
            app.submit_lock_input();
            true
        }
        _ => true, // swallow all other keys; lock screen owns input fully
    }
}

/// Handle keybinding capture: intercepts ALL keys when capturing a new binding.
pub fn handle_keybinding_capture(app: &mut App, modifiers: KeyModifiers, code: KeyCode) {
    if code == KeyCode::Esc && modifiers == KeyModifiers::NONE {
        app.keybindings_overlay.capturing = false;
        app.status_message.clear();
        return;
    }

    let (mode, action) = app.keybindings_overlay_item(app.keybindings_overlay.index);
    let Some(action) = action else {
        app.keybindings_overlay.capturing = false;
        return;
    };

    // Strip SHIFT for Char keys -- case is encoded in the character itself
    let modifiers = if matches!(code, KeyCode::Char(_)) {
        modifiers - KeyModifiers::SHIFT
    } else {
        modifiers
    };
    let combo = keybindings::KeyCombo { modifiers, code };
    let displaced = app.keybindings.rebind(mode, action, combo.clone());
    app.keybindings_overlay.capturing = false;

    if let Some(displaced_action) = displaced
        && displaced_action != action
    {
        app.status_message = format!(
            "'{}' was bound to {}. Accept? (y/n)",
            keybindings::format_key_combo(&combo),
            keybindings::action_label(displaced_action)
        );
        app.keybindings_overlay.conflict = Some((displaced_action, combo));
        return;
    }
    app.status_message = format!(
        "{} \u{2192} {}",
        keybindings::action_label(action),
        keybindings::format_key_combo(&combo)
    );
}

/// Handle a key press while the /group menu (and its sub-states) is open.
pub fn handle_group_menu_key(app: &mut App, code: KeyCode) -> Option<SendRequest> {
    let state = app.group_menu.state.clone()?;
    match state {
        GroupMenuState::Menu => {
            let items = app.group_menu_items();
            let item_count = items.len();
            match code {
                KeyCode::Char('j') | KeyCode::Down
                    if app.group_menu.index < item_count.saturating_sub(1) =>
                {
                    app.group_menu.index += 1;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    app.group_menu.index = app.group_menu.index.saturating_sub(1);
                }
                KeyCode::Enter => {
                    if let Some(action) = items.get(app.group_menu.index) {
                        app.transition_group_menu(action.key_hint);
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(hint) = GroupMenuHint::from_char(c)
                        && items.iter().any(|a| a.key_hint == hint)
                    {
                        app.transition_group_menu(hint);
                    }
                }
                KeyCode::Esc => {
                    app.group_menu.state = None;
                    app.close_overlay();
                }
                _ => {}
            }
            None
        }
        GroupMenuState::Members => {
            let member_count = app.group_menu.filtered.len();
            match code {
                KeyCode::Char('j') | KeyCode::Down
                    if app.group_menu.index < member_count.saturating_sub(1) =>
                {
                    app.group_menu.index += 1;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    app.group_menu.index = app.group_menu.index.saturating_sub(1);
                }
                KeyCode::Esc => {
                    app.open_overlay(OverlayKind::GroupMenu);
                    app.group_menu.state = Some(GroupMenuState::Menu);
                    app.group_menu.index = 0;
                }
                _ => {}
            }
            None
        }
        GroupMenuState::AddMember => {
            match code {
                KeyCode::Char('j') | KeyCode::Down
                    if !app.group_menu.filtered.is_empty()
                        && app.group_menu.index < app.group_menu.filtered.len() - 1 =>
                {
                    app.group_menu.index += 1;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    app.group_menu.index = app.group_menu.index.saturating_sub(1);
                }
                KeyCode::Enter => {
                    if let Some((phone, _)) = app.group_menu.filtered.get(app.group_menu.index) {
                        let phone = phone.clone();
                        let group_id = app.active_conversation.clone()?;
                        app.group_menu.state = None;
                        app.close_overlay();
                        app.group_menu.filter.clear();
                        return Some(SendRequest::AddGroupMembers {
                            group_id,
                            members: vec![phone],
                        });
                    }
                }
                KeyCode::Esc => {
                    app.open_overlay(OverlayKind::GroupMenu);
                    app.group_menu.state = Some(GroupMenuState::Menu);
                    app.group_menu.index = 0;
                    app.group_menu.filter.clear();
                }
                KeyCode::Backspace => {
                    app.group_menu.filter.pop();
                    app.group_menu.index = 0;
                    app.refresh_group_add_filter();
                }
                KeyCode::Char(c) if c != 'j' && c != 'k' => {
                    app.group_menu.filter.push(c);
                    app.group_menu.index = 0;
                    app.refresh_group_add_filter();
                }
                _ => {}
            }
            None
        }
        GroupMenuState::RemoveMember => {
            match code {
                KeyCode::Char('j') | KeyCode::Down
                    if !app.group_menu.filtered.is_empty()
                        && app.group_menu.index < app.group_menu.filtered.len() - 1 =>
                {
                    app.group_menu.index += 1;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    app.group_menu.index = app.group_menu.index.saturating_sub(1);
                }
                KeyCode::Enter => {
                    if let Some((phone, _)) = app.group_menu.filtered.get(app.group_menu.index) {
                        let phone = phone.clone();
                        let group_id = app.active_conversation.clone()?;
                        app.group_menu.state = None;
                        app.close_overlay();
                        app.group_menu.filter.clear();
                        return Some(SendRequest::RemoveGroupMembers {
                            group_id,
                            members: vec![phone],
                        });
                    }
                }
                KeyCode::Esc => {
                    app.open_overlay(OverlayKind::GroupMenu);
                    app.group_menu.state = Some(GroupMenuState::Menu);
                    app.group_menu.index = 0;
                    app.group_menu.filter.clear();
                }
                KeyCode::Backspace => {
                    app.group_menu.filter.pop();
                    app.group_menu.index = 0;
                    app.refresh_group_remove_filter();
                }
                KeyCode::Char(c) if c != 'j' && c != 'k' => {
                    app.group_menu.filter.push(c);
                    app.group_menu.index = 0;
                    app.refresh_group_remove_filter();
                }
                _ => {}
            }
            None
        }
        GroupMenuState::Rename => {
            match code {
                KeyCode::Enter => {
                    let name = app.group_menu.input.trim().to_string();
                    if !name.is_empty() {
                        let group_id = app.active_conversation.clone()?;
                        app.group_menu.state = None;
                        app.close_overlay();
                        app.group_menu.input.clear();
                        return Some(SendRequest::RenameGroup { group_id, name });
                    }
                }
                KeyCode::Esc => {
                    app.open_overlay(OverlayKind::GroupMenu);
                    app.group_menu.state = Some(GroupMenuState::Menu);
                    app.group_menu.index = 0;
                    app.group_menu.input.clear();
                }
                KeyCode::Backspace => {
                    app.group_menu.input.pop();
                }
                KeyCode::Char(c) => {
                    app.group_menu.input.push(c);
                }
                _ => {}
            }
            None
        }
        GroupMenuState::Create => {
            match code {
                KeyCode::Enter => {
                    let name = app.group_menu.input.trim().to_string();
                    if !name.is_empty() {
                        app.group_menu.state = None;
                        app.close_overlay();
                        app.group_menu.input.clear();
                        return Some(SendRequest::CreateGroup { name });
                    }
                }
                KeyCode::Esc => {
                    app.group_menu.state = None;
                    app.close_overlay();
                    app.group_menu.input.clear();
                }
                KeyCode::Backspace => {
                    app.group_menu.input.pop();
                }
                KeyCode::Char(c) => {
                    app.group_menu.input.push(c);
                }
                _ => {}
            }
            None
        }
        GroupMenuState::LeaveConfirm => {
            match code {
                KeyCode::Char('y') => {
                    let group_id = app.active_conversation.clone()?;
                    app.group_menu.state = None;
                    app.close_overlay();
                    return Some(SendRequest::LeaveGroup { group_id });
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    app.open_overlay(OverlayKind::GroupMenu);
                    app.group_menu.state = Some(GroupMenuState::Menu);
                    app.group_menu.index = 0;
                }
                _ => {}
            }
            None
        }
    }
}

/// Handle a key press while the settings profile manager overlay is open.
pub fn handle_settings_profile_manager_key(app: &mut App, code: KeyCode) {
    // Save-as text input mode
    if app.settings_profiles.save_as {
        match code {
            KeyCode::Enter => {
                let name = app.settings_profiles.save_as_input.trim().to_string();
                if name.is_empty() {
                    app.status_message = "Profile name cannot be empty".to_string();
                } else if crate::settings_profile::is_builtin(&name) {
                    app.status_message = "Cannot overwrite built-in profile".to_string();
                } else {
                    let profile =
                        crate::settings_profile::SettingsProfile::from_app(app, name.clone());
                    match crate::settings_profile::save_custom_profile(&profile) {
                        Ok(()) => {
                            app.settings_profiles.name = name;
                            app.settings_profiles.available =
                                crate::settings_profile::all_settings_profiles();
                            app.settings_profiles.index = app
                                .settings_profiles
                                .available
                                .iter()
                                .position(|p| p.name == app.settings_profiles.name)
                                .unwrap_or(0);
                            app.save_settings();
                            app.status_message = "Profile saved".to_string();
                        }
                        Err(e) => {
                            app.status_message = format!("Save failed: {e}");
                        }
                    }
                    app.settings_profiles.save_as = false;
                }
            }
            KeyCode::Esc => {
                app.settings_profiles.save_as = false;
            }
            KeyCode::Backspace => {
                app.settings_profiles.save_as_input.pop();
            }
            KeyCode::Char(c) if app.settings_profiles.save_as_input.len() < 30 => {
                app.settings_profiles.save_as_input.push(c);
            }
            _ => {}
        }
        return;
    }

    // List navigation mode
    match code {
        KeyCode::Char('j') | KeyCode::Down
            if app.settings_profiles.index
                < app.settings_profiles.available.len().saturating_sub(1) =>
        {
            app.settings_profiles.index += 1;
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.settings_profiles.index = app.settings_profiles.index.saturating_sub(1);
        }
        KeyCode::Enter => {
            // Load the selected profile (stay open for preview)
            if let Some(profile) = app
                .settings_profiles
                .available
                .get(app.settings_profiles.index)
                .cloned()
            {
                app.apply_settings_profile_deferred(&profile);
                app.save_settings();
                app.status_message = format!("Loaded profile: {}", profile.name);
            }
        }
        KeyCode::Char('s') => {
            // Save over current custom profile (only if custom and settings differ)
            if let Some(profile) = app
                .settings_profiles
                .available
                .get(app.settings_profiles.index)
            {
                if crate::settings_profile::is_builtin(&profile.name) {
                    return;
                }
                if profile.matches_app(app) {
                    return;
                }
                let updated =
                    crate::settings_profile::SettingsProfile::from_app(app, profile.name.clone());
                match crate::settings_profile::save_custom_profile(&updated) {
                    Ok(()) => {
                        app.settings_profiles.name = updated.name.clone();
                        app.settings_profiles.available =
                            crate::settings_profile::all_settings_profiles();
                        app.settings_profiles.index = app
                            .settings_profiles
                            .available
                            .iter()
                            .position(|p| p.name == app.settings_profiles.name)
                            .unwrap_or(0);
                        app.save_settings();
                        app.status_message = "Profile saved".to_string();
                    }
                    Err(e) => {
                        app.status_message = format!("Save failed: {e}");
                    }
                }
            }
        }
        KeyCode::Char('S') => {
            // Save-as: open name input
            let has_changes = !app
                .settings_profiles
                .available
                .iter()
                .any(|p| p.name == app.settings_profiles.name && p.matches_app(app));
            if has_changes {
                app.settings_profiles.save_as = true;
                app.settings_profiles.save_as_input.clear();
            }
        }
        KeyCode::Char('d') => {
            // Delete custom profile
            if let Some(profile) = app
                .settings_profiles
                .available
                .get(app.settings_profiles.index)
            {
                if crate::settings_profile::is_builtin(&profile.name) {
                    return;
                }
                let name = profile.name.clone();
                match crate::settings_profile::delete_custom_profile(&name) {
                    Ok(()) => {
                        if app.settings_profiles.name == name {
                            app.settings_profiles.name = "Default".to_string();
                        }
                        app.settings_profiles.available =
                            crate::settings_profile::all_settings_profiles();
                        if app.settings_profiles.index >= app.settings_profiles.available.len() {
                            app.settings_profiles.index =
                                app.settings_profiles.available.len().saturating_sub(1);
                        }
                        app.save_settings();
                        app.status_message = format!("Deleted profile: {name}");
                    }
                    Err(e) => {
                        app.status_message = format!("Delete failed: {e}");
                    }
                }
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            app.close_overlay();
            app.fire_deferred_settings_hooks();
        }
        _ => {}
    }
}

/// Handle a key press while the autocomplete popup is visible.
/// Returns `Some(SendRequest)` when the user submits a command that requires
/// sending a message. Returns `None` otherwise.
pub fn handle_autocomplete_key(app: &mut App, code: KeyCode) -> Option<SendRequest> {
    let list_len = app.autocomplete.len();
    match code {
        KeyCode::Up => {
            if list_len > 0 {
                app.autocomplete.index = if app.autocomplete.index == 0 {
                    list_len - 1
                } else {
                    app.autocomplete.index - 1
                };
            }
        }
        KeyCode::Down => {
            if list_len > 0 {
                app.autocomplete.index = (app.autocomplete.index + 1) % list_len;
            }
        }
        KeyCode::Tab => {
            app.apply_autocomplete();
        }
        KeyCode::Esc => {
            app.autocomplete.clear();
            if app.is_overlay(OverlayKind::Autocomplete) {
                app.close_overlay();
            }
        }
        KeyCode::Enter => {
            if app.autocomplete.mode == AutocompleteMode::Mention {
                app.apply_autocomplete();
                // Don't submit on Enter for mentions -- just complete
            } else {
                // Command and Join: apply + submit
                app.apply_autocomplete();
                return app.handle_input();
            }
        }
        _ => {
            app.apply_input_edit(code);
            app.update_autocomplete();
        }
    }
    None
}

/// Handle a key press while the message search overlay is open.
pub fn handle_search_key(app: &mut App, code: KeyCode) {
    let active = app.active_conversation.as_deref().map(str::to_owned);
    let action = app.search.handle_key(code, active.as_deref(), &app.db);
    app.dispatch_search_action(action);
}

/// Handle a key press while the file browser overlay is open.
pub fn handle_file_browser_key(app: &mut App, code: KeyCode) {
    match app.file_picker.handle_key(code) {
        crate::domain::FilePickerOutcome::Continue => {}
        crate::domain::FilePickerOutcome::Selected(path) => {
            app.pending_attachment = Some(path);
            app.close_overlay();
        }
        crate::domain::FilePickerOutcome::Cancelled => {
            app.close_overlay();
        }
    }
}

/// Handle a key press while the settings overlay is open.
pub fn handle_settings_key(app: &mut App, code: KeyCode) {
    let preview_index = SETTINGS.len();
    let image_mode_index = SETTINGS.len() + 1;
    let customize_index = SETTINGS.len() + 2;

    // Find current position in visual order
    let visual_pos = SETTINGS_VISUAL_ORDER
        .iter()
        .position(|&i| i == app.settings_overlay.index)
        .unwrap_or(0);

    match code {
        KeyCode::Char('j') | KeyCode::Down if visual_pos + 1 < SETTINGS_VISUAL_ORDER.len() => {
            app.settings_overlay.index = SETTINGS_VISUAL_ORDER[visual_pos + 1];
        }
        KeyCode::Char('k') | KeyCode::Up if visual_pos > 0 => {
            app.settings_overlay.index = SETTINGS_VISUAL_ORDER[visual_pos - 1];
        }
        KeyCode::Char(' ') | KeyCode::Enter | KeyCode::Tab => {
            if app.settings_overlay.index == preview_index {
                app.notifications.notification_preview =
                    app.notifications.notification_preview.cycle();
            } else if app.settings_overlay.index == image_mode_index {
                app.image.image_mode = app.image.image_mode.cycle();
            } else if app.settings_overlay.index == customize_index {
                app.open_overlay(OverlayKind::Customize);
                app.settings_overlay.customize_index = 0;
            } else {
                app.toggle_setting(app.settings_overlay.index);
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            app.close_overlay();
            app.save_settings();
            app.fire_deferred_settings_hooks();
        }
        _ => {}
    }
}

/// Handle a key press in the Customize sub-menu (Theme, Keybindings, Profile).
pub fn handle_customize_key(app: &mut App, code: KeyCode) {
    const ITEMS: usize = 3; // Theme, Keybindings, Profile
    let action = classify_list_key(code, false);
    if crate::list_overlay::apply_nav(&action, &mut app.settings_overlay.customize_index, ITEMS) {
        return;
    }
    match code {
        KeyCode::Char(' ') | KeyCode::Enter | KeyCode::Tab => {
            app.close_overlay();
            app.save_settings();
            match app.settings_overlay.customize_index {
                0 => {
                    app.open_overlay(OverlayKind::ThemePicker);
                    app.theme_picker.index = app
                        .theme_picker
                        .available_themes
                        .iter()
                        .position(|t| t.name == app.theme.name)
                        .unwrap_or(0);
                }
                1 => {
                    app.open_overlay(OverlayKind::Keybindings);
                    app.keybindings_overlay.index = 0;
                }
                2 => {
                    app.open_settings_profile_manager();
                }
                _ => {}
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            app.close_overlay();
        }
        _ => {}
    }
}

/// Handle a key press while the per-message action menu overlay is open.
pub fn handle_action_menu_key(app: &mut App, code: KeyCode) -> Option<SendRequest> {
    let item_count = app.action_menu_items().len();
    if item_count == 0 {
        app.close_overlay();
        return None;
    }
    let action = classify_list_key(code, false);
    if crate::list_overlay::apply_nav(&action, &mut app.action_menu.index, item_count) {
        return None;
    }
    match action {
        ListKeyAction::Select => {
            let items = app.action_menu_items();
            if let Some(action) = items.get(app.action_menu.index) {
                let hint = action.key_hint;
                app.close_overlay();
                app.execute_action_by_hint(hint)
            } else {
                app.close_overlay();
                None
            }
        }
        ListKeyAction::Close => {
            app.close_overlay();
            None
        }
        ListKeyAction::None => {
            // Action menu shortcut keys: parse the keypress into a hint, then
            // check that the hint corresponds to an action currently present in
            // the menu (the menu items vary by message state).
            let KeyCode::Char(c) = code else { return None };
            let hint = ActionMenuHint::from_char(c)?;
            let items = app.action_menu_items();
            if items.iter().any(|a| a.key_hint == hint) {
                app.close_overlay();
                app.execute_action_by_hint(hint)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Handle a key press while the theme picker overlay is open.
pub fn handle_theme_key(app: &mut App, code: KeyCode) {
    // Theme-specific: space selects, q closes
    let code = match code {
        KeyCode::Char(' ') => KeyCode::Enter,
        KeyCode::Char('q') => KeyCode::Esc,
        other => other,
    };
    let action = classify_list_key(code, false);
    if crate::list_overlay::apply_nav(
        &action,
        &mut app.theme_picker.index,
        app.theme_picker.available_themes.len(),
    ) {
        return;
    }
    match action {
        ListKeyAction::Select => {
            if let Some(selected) = app
                .theme_picker
                .available_themes
                .get(app.theme_picker.index)
            {
                app.theme = selected.clone();
                app.save_settings();
            }
            app.close_overlay();
        }
        ListKeyAction::Close => {
            app.close_overlay();
        }
        _ => {}
    }
}

/// Handle a key press while the keybindings overlay is open.
pub fn handle_keybindings_key(app: &mut App, code: KeyCode) {
    if app.keybindings_overlay.profile_picker {
        match code {
            KeyCode::Char('j') | KeyCode::Down
                if app.keybindings_overlay.profile_index
                    < app
                        .keybindings_overlay
                        .available_profiles
                        .len()
                        .saturating_sub(1) =>
            {
                app.keybindings_overlay.profile_index += 1;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.keybindings_overlay.profile_index =
                    app.keybindings_overlay.profile_index.saturating_sub(1);
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                if let Some(name) = app
                    .keybindings_overlay
                    .available_profiles
                    .get(app.keybindings_overlay.profile_index)
                {
                    let mut kb = keybindings::find_profile(name);
                    let overrides = keybindings::load_overrides();
                    kb.apply_overrides(&overrides);
                    app.keybindings = kb;
                    app.save_settings();
                }
                app.keybindings_overlay.profile_picker = false;
            }
            KeyCode::Esc => {
                app.keybindings_overlay.profile_picker = false;
            }
            _ => {}
        }
        return;
    }

    if let Some((displaced_action, _combo)) = app.keybindings_overlay.conflict.take() {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                // Accept: the displaced action loses its binding
                app.status_message = format!(
                    "{} is now unbound",
                    keybindings::action_label(displaced_action)
                );
            }
            _ => {
                // Undo the rebind -- restore both
                let (mode, action) = app.keybindings_overlay_item(app.keybindings_overlay.index);
                if let Some(action) = action {
                    app.keybindings.reset_action(mode, action);
                    app.keybindings.reset_action(mode, displaced_action);
                }
                app.status_message.clear();
            }
        }
        return;
    }

    let total = app.keybindings_overlay_total();
    match code {
        KeyCode::Char('j') | KeyCode::Down => {
            if app.keybindings_overlay.index < total.saturating_sub(1) {
                app.keybindings_overlay.index += 1;
            }
            // Skip section headers
            while app.keybindings_overlay.index < total
                && app
                    .keybindings_overlay_item(app.keybindings_overlay.index)
                    .1
                    .is_none()
            {
                app.keybindings_overlay.index += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.keybindings_overlay.index = app.keybindings_overlay.index.saturating_sub(1);
            // Skip section headers (index 0 is the profile row -- always selectable)
            while app.keybindings_overlay.index > 0
                && app
                    .keybindings_overlay_item(app.keybindings_overlay.index)
                    .1
                    .is_none()
            {
                app.keybindings_overlay.index = app.keybindings_overlay.index.saturating_sub(1);
            }
        }
        KeyCode::Enter => {
            if app.keybindings_overlay.index == 0 {
                // Profile row -> open profile picker
                app.keybindings_overlay.profile_picker = true;
                app.keybindings_overlay.profile_index = app
                    .keybindings_overlay
                    .available_profiles
                    .iter()
                    .position(|n| *n == app.keybindings.profile_name)
                    .unwrap_or(0);
            } else {
                let (_, action) = app.keybindings_overlay_item(app.keybindings_overlay.index);
                if action.is_some() {
                    app.keybindings_overlay.capturing = true;
                    app.status_message = "Press a key combo...".to_string();
                }
            }
        }
        KeyCode::Backspace => {
            // Reset to profile default
            let (mode, action) = app.keybindings_overlay_item(app.keybindings_overlay.index);
            if let Some(action) = action {
                app.keybindings.reset_action(mode, action);
                app.status_message = format!("Reset {}", keybindings::action_label(action));
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            app.close_overlay();
            app.save_settings();
        }
        _ => {}
    }
}

/// Handle a key press in the message-request (accept/delete) overlay.
pub fn handle_message_request_key(app: &mut App, code: KeyCode) -> Option<SendRequest> {
    let conv_id = match app.active_conversation.clone() {
        Some(id) => id,
        None => {
            app.close_overlay();
            return None;
        }
    };
    match code {
        KeyCode::Char('a') => {
            let is_group = app
                .store
                .conversations
                .get(&conv_id)
                .map(|c| c.is_group)
                .unwrap_or(false);
            if let Some(conv) = app.store.conversations.get_mut(&conv_id) {
                conv.accepted = true;
            }
            app.db_warn_visible(app.db.update_accepted(&conv_id, true), "update_accepted");
            app.close_overlay();
            Some(SendRequest::MessageRequestResponse {
                recipient: conv_id,
                is_group,
                response_type: "accept".to_string(),
            })
        }
        KeyCode::Char('d') => {
            app.close_overlay();
            app.delete_active_conversation()
        }
        KeyCode::Esc => {
            app.close_overlay();
            app.active_conversation = None;
            None
        }
        _ => None,
    }
}

/// Handle a key press in the quick-reaction picker overlay.
pub fn handle_reaction_picker_key(app: &mut App, code: KeyCode) -> Option<SendRequest> {
    match code {
        KeyCode::Char('h') | KeyCode::Left => {
            app.reactions.picker_index = app.reactions.picker_index.saturating_sub(1);
            None
        }
        KeyCode::Char('l') | KeyCode::Right => {
            if app.reactions.picker_index < QUICK_REACTIONS.len() - 1 {
                app.reactions.picker_index += 1;
            }
            None
        }
        KeyCode::Char(c @ '1'..='8') => {
            let idx = (c as u8 - b'1') as usize;
            if idx < QUICK_REACTIONS.len() {
                app.reactions.picker_index = idx;
                app.close_overlay();
                app.prepare_reaction_send()
            } else {
                None
            }
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            app.close_overlay();
            app.prepare_reaction_send()
        }
        KeyCode::Char('e') | KeyCode::Char('/') => {
            // Open full emoji picker from reaction context
            app.emoji_picker.open(EmojiPickerSource::Reaction, None);
            app.open_overlay(OverlayKind::EmojiPicker);
            None
        }
        KeyCode::Esc => {
            app.close_overlay();
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
                    text_styles: Vec::new(),
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
