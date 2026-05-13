//! Per-message action menu overlay + delete confirmation prompt.
//!
//! Lists the actions available on the focused message (reply, react,
//! edit, delete, copy, forward, etc.) with a highlighted cursor row,
//! Nerd Font icons when enabled, and right-aligned key hints.
//!
//! `draw_delete_confirm` is the y/l/n confirmation prompt the action
//! menu's "delete" choice spawns. It lives here rather than in its
//! own file because the action menu is its only entry point. Outgoing
//! messages get the full y/l/n prompt; incoming messages can only be
//! deleted locally so the prompt collapses to y/n.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::centered_popup;
use crate::app::App;

pub(in crate::ui) fn draw_action_menu(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let items = app.action_menu_items();
    if items.is_empty() {
        return;
    }

    let popup_width: u16 = 30;
    let popup_height = items.len() as u16 + 4;

    let (popup_area, block) =
        centered_popup(frame, area, popup_width, popup_height, " Actions ", theme);

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let content_width = inner.width as usize;

    let mut lines: Vec<Line> = Vec::new();
    for (i, action) in items.iter().enumerate() {
        let is_selected = i == app.action_menu.index;
        let icon = if app.nerd_fonts {
            format!("{} ", action.nerd_icon)
        } else {
            String::new()
        };

        let label_part = format!("  {icon}{}", action.label);
        let hint_char = action.key_hint.key_char();
        // hint is rendered as a single char; account for that plus a trailing space
        let pad = content_width.saturating_sub(label_part.chars().count() + 1 + 2);
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
            Span::styled(format!("{hint_char} "), hint_style),
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

pub(in crate::ui) fn draw_delete_confirm(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let msg = app.selected_message();
    let is_outgoing = msg.is_some_and(|m| m.sender == "you");

    let (popup_area, block) = centered_popup(frame, area, 44, 5, " Delete Message ", theme);

    let prompt = if is_outgoing {
        "Delete for everyone? (y)es / (l)ocal / (n)o"
    } else {
        "Delete locally? (y)es / (n)o"
    };

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {prompt}"),
            Style::default().fg(theme.fg),
        )),
    ];
    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}
