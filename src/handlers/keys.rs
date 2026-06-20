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

use crate::app::{
    ActionMenuHint, App, InputMode, OverlayKind, PIN_DURATIONS, PinPending, QUICK_REACTIONS,
    SETTINGS, SETTINGS_VISUAL_ORDER, SendRequest,
};
use crate::domain::EmojiPickerSource;
use crate::keybindings;
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
