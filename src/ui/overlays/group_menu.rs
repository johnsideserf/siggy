//! Group management overlay (multi-screen).
//!
//! Six sub-screens driven by `app.group_menu.state`:
//! - `Menu`: per-group action list (members, add/remove, rename, leave)
//! - `Members`: scrollable member list with `(you)` marker
//! - `AddMember` / `RemoveMember`: type-to-filter contact pickers,
//!   sized to `CONTACTS_POPUP_WIDTH` to match the contacts overlay
//! - `Rename` / `Create`: text-input popup with block cursor
//! - `LeaveConfirm`: y/n confirmation prompt

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::{
    CONTACTS_POPUP_WIDTH, GROUP_MEMBER_MAX_VISIBLE, GROUP_MENU_POPUP_WIDTH, centered_popup,
    truncate,
};
use crate::app::{App, GroupMenuState};

pub(in crate::ui) fn draw_group_menu(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let state = match &app.group_menu.state {
        Some(s) => s,
        None => return,
    };
    match state {
        GroupMenuState::Menu => {
            let items = app.group_menu_items();
            if items.is_empty() {
                return;
            }
            let popup_height = items.len() as u16 + 4;
            let title = app
                .active_conversation
                .as_ref()
                .and_then(|id| app.store.conversations.get(id))
                .filter(|c| c.is_group)
                .map(|c| format!(" #{} ", c.name))
                .unwrap_or_else(|| " Group ".to_string());
            let (popup_area, block) = centered_popup(
                frame,
                area,
                GROUP_MENU_POPUP_WIDTH,
                popup_height,
                &title,
                theme,
            );
            let inner = block.inner(popup_area);
            frame.render_widget(block, popup_area);
            let content_width = inner.width as usize;
            let mut lines: Vec<Line> = Vec::new();
            for (i, action) in items.iter().enumerate() {
                let is_selected = i == app.group_menu.index;
                let icon = if app.nerd_fonts {
                    format!("{} ", action.nerd_icon)
                } else {
                    String::new()
                };
                let label_part = format!("  {icon}{}", action.label);
                let hint_width = action.key_hint.len();
                let pad = content_width.saturating_sub(label_part.chars().count() + hint_width + 2);
                let padding = " ".repeat(pad);
                let row_style = if is_selected {
                    Style::default().bg(theme.bg_selected)
                } else {
                    Style::default()
                };
                let hint_style = if is_selected {
                    Style::default()
                        .bg(theme.bg_selected)
                        .fg(theme.fg_muted)
                        .add_modifier(Modifier::DIM)
                } else {
                    Style::default().fg(theme.fg_muted)
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{label_part}{padding}"), row_style),
                    Span::styled(format!("{} ", action.key_hint), hint_style),
                ]));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Esc to close",
                Style::default().fg(theme.fg_muted),
            )));
            let popup = Paragraph::new(lines);
            frame.render_widget(popup, inner);
        }
        GroupMenuState::Members => {
            let max_visible = GROUP_MEMBER_MAX_VISIBLE.min(app.group_menu.filtered.len().max(1));
            let pref_height = max_visible as u16 + 5;
            let title = " Members ".to_string();
            let (popup_area, block) = centered_popup(
                frame,
                area,
                GROUP_MENU_POPUP_WIDTH,
                pref_height,
                &title,
                theme,
            );
            let inner_height = popup_area.height.saturating_sub(2) as usize;
            let footer_lines = 2;
            let visible_rows = inner_height.saturating_sub(footer_lines);
            let scroll_offset = if app.group_menu.index >= visible_rows {
                app.group_menu.index - visible_rows + 1
            } else {
                0
            };
            let mut lines: Vec<Line> = Vec::new();
            if app.group_menu.filtered.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  No members",
                    Style::default().fg(theme.fg_muted),
                )));
            } else {
                let end = (scroll_offset + visible_rows).min(app.group_menu.filtered.len());
                for (i, (phone, name)) in app.group_menu.filtered[scroll_offset..end]
                    .iter()
                    .enumerate()
                {
                    let actual_index = scroll_offset + i;
                    let is_selected = actual_index == app.group_menu.index;
                    let is_self = *phone == app.account;
                    let display = if is_self {
                        format!("  {} (you)", name)
                    } else {
                        format!("  {}", name)
                    };
                    let name_style = if is_selected {
                        Style::default()
                            .bg(theme.bg_selected)
                            .fg(theme.fg)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.fg)
                    };
                    let phone_style = if is_selected {
                        Style::default().bg(theme.bg_selected).fg(theme.fg_muted)
                    } else {
                        Style::default().fg(theme.fg_muted)
                    };
                    lines.push(Line::from(vec![
                        Span::styled(display, name_style),
                        Span::styled(format!("  {}", phone), phone_style),
                    ]));
                }
            }
            while lines.len() < visible_rows {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Esc to go back",
                Style::default().fg(theme.fg_muted),
            )));
            let popup = Paragraph::new(lines).block(block);
            frame.render_widget(popup, popup_area);
        }
        GroupMenuState::AddMember | GroupMenuState::RemoveMember => {
            let is_add = *state == GroupMenuState::AddMember;
            let max_visible = GROUP_MEMBER_MAX_VISIBLE.min(app.group_menu.filtered.len().max(1));
            let pref_height = max_visible as u16 + 5;
            let title = if is_add {
                if app.group_menu.filter.is_empty() {
                    " Add Member ".to_string()
                } else {
                    format!(" Add Member [{}] ", app.group_menu.filter)
                }
            } else if app.group_menu.filter.is_empty() {
                " Remove Member ".to_string()
            } else {
                format!(" Remove Member [{}] ", app.group_menu.filter)
            };
            let (popup_area, block) = centered_popup(
                frame,
                area,
                CONTACTS_POPUP_WIDTH,
                pref_height,
                &title,
                theme,
            );
            let inner_height = popup_area.height.saturating_sub(2) as usize;
            let footer_lines = 2;
            let visible_rows = inner_height.saturating_sub(footer_lines);
            let scroll_offset = if app.group_menu.index >= visible_rows {
                app.group_menu.index - visible_rows + 1
            } else {
                0
            };
            let mut lines: Vec<Line> = Vec::new();
            if app.group_menu.filtered.is_empty() {
                let msg = if is_add {
                    "  No contacts to add"
                } else {
                    "  No members to remove"
                };
                lines.push(Line::from(Span::styled(
                    msg,
                    Style::default().fg(theme.fg_muted),
                )));
            } else {
                let end = (scroll_offset + visible_rows).min(app.group_menu.filtered.len());
                let inner_w = popup_area.width.saturating_sub(2) as usize;
                for (i, (phone, name)) in app.group_menu.filtered[scroll_offset..end]
                    .iter()
                    .enumerate()
                {
                    let actual_index = scroll_offset + i;
                    let is_selected = actual_index == app.group_menu.index;
                    let number_display = format!("  {}", phone);
                    let name_max = inner_w.saturating_sub(number_display.len() + 2);
                    let display_name = truncate(name, name_max);
                    let name_style = if is_selected {
                        Style::default()
                            .bg(theme.bg_selected)
                            .fg(theme.fg)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.fg)
                    };
                    let number_style = if is_selected {
                        Style::default().bg(theme.bg_selected).fg(theme.accent)
                    } else {
                        Style::default().fg(theme.fg_muted)
                    };
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {}", display_name), name_style),
                        Span::styled(number_display, number_style),
                    ]));
                }
            }
            while lines.len() < visible_rows {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Enter to select \u{00b7} Esc to cancel",
                Style::default().fg(theme.fg_muted),
            )));
            let popup = Paragraph::new(lines).block(block);
            frame.render_widget(popup, popup_area);
        }
        GroupMenuState::Rename | GroupMenuState::Create => {
            let is_rename = *state == GroupMenuState::Rename;
            let title = if is_rename {
                " Rename Group "
            } else {
                " Create Group "
            };
            let (popup_area, block) =
                centered_popup(frame, area, GROUP_MENU_POPUP_WIDTH, 6, title, theme);
            let inner = block.inner(popup_area);
            frame.render_widget(block, popup_area);
            let mut lines: Vec<Line> = Vec::new();
            let input_display = format!("  {}\u{2588}", app.group_menu.input);
            lines.push(Line::from(Span::styled(
                input_display,
                Style::default().fg(theme.fg),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Enter to confirm \u{00b7} Esc to cancel",
                Style::default().fg(theme.fg_muted),
            )));
            let popup = Paragraph::new(lines);
            frame.render_widget(popup, inner);
        }
        GroupMenuState::LeaveConfirm => {
            let group_name = app
                .active_conversation
                .as_ref()
                .and_then(|id| app.store.conversations.get(id))
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "this group".to_string());
            let prompt = format!("Leave #{}?", group_name);
            let (popup_area, block) = centered_popup(
                frame,
                area,
                GROUP_MENU_POPUP_WIDTH,
                5,
                " Leave Group ",
                theme,
            );
            let inner = block.inner(popup_area);
            frame.render_widget(block, popup_area);
            let mut lines: Vec<Line> = Vec::new();
            lines.push(Line::from(Span::styled(
                format!("  {}", prompt),
                Style::default().fg(theme.warning),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  (y)es / (n)o",
                Style::default().fg(theme.fg_muted),
            )));
            let popup = Paragraph::new(lines);
            frame.render_widget(popup, inner);
        }
    }
}
