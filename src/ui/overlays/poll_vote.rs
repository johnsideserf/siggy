//! Poll vote overlay.
//!
//! Multi-select (or single-choice) checkbox list driven by
//! `app.poll_vote.{pending, selections, index}`. Width is sized to
//! the longest option text; Space toggles the checkbox under the
//! cursor and Enter submits.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::centered_popup;
use crate::app::App;

pub(in crate::ui) fn draw_poll_vote_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let pending = match &app.poll_vote.pending {
        Some(p) => p,
        None => return,
    };

    let option_count = pending.options.len();
    let max_text_len = pending
        .options
        .iter()
        .map(|o| o.text.len())
        .max()
        .unwrap_or(8);
    let popup_width = (max_text_len as u16 + 12)
        .max(24)
        .min(area.width.saturating_sub(4));
    let popup_height = option_count as u16 + 5;

    let (popup_area, block) =
        centered_popup(frame, area, popup_width, popup_height, " Vote ", theme);

    let mut lines: Vec<Line> = Vec::new();

    for (i, opt) in pending.options.iter().enumerate() {
        let selected = app.poll_vote.selections.get(i).copied().unwrap_or(false);
        let marker = if i == app.poll_vote.index { ">" } else { " " };
        let checkbox = if selected { "[x]" } else { "[ ]" };
        let style = if i == app.poll_vote.index {
            Style::default()
                .bg(theme.bg_selected)
                .fg(theme.fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };
        lines.push(Line::from(Span::styled(
            format!(" {marker} {checkbox} {}", opt.text),
            style,
        )));
    }

    lines.push(Line::from(""));
    let mode_hint = if pending.allow_multiple {
        "Space: toggle"
    } else {
        "Space: select"
    };
    lines.push(Line::from(Span::styled(
        format!(" {mode_hint}  Enter: submit  Esc"),
        Style::default().fg(theme.fg_muted),
    )));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}
