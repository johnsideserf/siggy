//! Profile editor overlay.
//!
//! Edit your own Signal profile: given/family name, about line, and
//! about-emoji. Each field can be selected and entered for editing
//! (block-cursor visible while typing). A `[ Save ]` button at index
//! 4 commits all four fields.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::{PROFILE_POPUP_WIDTH, centered_popup};
use crate::app::App;

pub(in crate::ui) fn draw_profile(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let labels = ["Given name", "Family name", "About", "About emoji"];

    let mut lines: Vec<Line> = Vec::new();

    for (i, label) in labels.iter().enumerate() {
        let is_selected = i == app.profile.index;
        let is_editing = is_selected && app.profile.editing;

        let label_style = if is_selected {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg_secondary)
        };

        let value = if is_editing {
            format!("{}\u{2588}", app.profile.edit_buffer) // block cursor
        } else {
            let v = &app.profile.fields[i];
            if v.is_empty() {
                "(empty)".to_string()
            } else {
                v.clone()
            }
        };

        let value_style = if is_editing || is_selected {
            Style::default().bg(theme.bg_selected).fg(theme.fg)
        } else if app.profile.fields[i].is_empty() {
            Style::default().fg(theme.fg_muted)
        } else {
            Style::default().fg(theme.fg)
        };

        let row_style = if is_selected {
            Style::default().bg(theme.bg_selected)
        } else {
            Style::default()
        };

        lines.push(Line::from(vec![
            Span::styled(format!("  {:<14} ", label), label_style),
            Span::styled(value, value_style),
            // Pad remaining width with bg color for selected row
            Span::styled("", row_style),
        ]));
    }

    // Blank line before Save
    lines.push(Line::from(""));

    // Save button
    let save_selected = app.profile.index == 4;
    let save_style = if save_selected {
        Style::default()
            .bg(theme.bg_selected)
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg_secondary)
    };
    lines.push(Line::from(Span::styled("  [ Save ]", save_style)));

    // Footer
    lines.push(Line::from(""));
    let footer = if app.profile.editing {
        "  Type to edit | Enter confirm | Esc cancel"
    } else {
        "  j/k navigate | Enter edit | Esc close"
    };
    lines.push(Line::from(Span::styled(
        footer,
        Style::default().fg(theme.fg_muted),
    )));

    let pref_height = lines.len() as u16 + 2; // +2 for borders
    let (popup_area, block) = centered_popup(
        frame,
        area,
        PROFILE_POPUP_WIDTH,
        pref_height,
        " Edit Profile ",
        theme,
    );

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}
