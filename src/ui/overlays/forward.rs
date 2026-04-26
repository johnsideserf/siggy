//! Forward-message picker overlay.
//!
//! Type-to-filter conversation list driven by `app.forward.{filter,
//! filtered, index}`. Enter forwards the carried message body to the
//! selected conversation.

use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::centered_popup;
use crate::app::App;
use crate::list_overlay;

pub(in crate::ui) fn draw_forward(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let max_rows = 10usize;
    let list_height = app.forward.filtered.len().min(max_rows);
    let pref_height = (list_height + 4) as u16; // filter line + blank + list + footer
    let (popup_area, block) = centered_popup(frame, area, 45, pref_height, " Forward to ", theme);
    let inner = popup_area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });

    let mut lines: Vec<Line> = Vec::new();

    // Filter input
    let filter_display = if app.forward.filter.is_empty() {
        "type to filter...".to_string()
    } else {
        app.forward.filter.clone()
    };
    let filter_style = if app.forward.filter.is_empty() {
        Style::default().fg(theme.fg_muted)
    } else {
        Style::default().fg(theme.fg)
    };
    lines.push(Line::from(Span::styled(
        format!("  > {filter_display}"),
        filter_style,
    )));
    lines.push(Line::from(""));

    // Conversation list
    let visible_rows = inner.height.saturating_sub(3) as usize;
    let scroll_offset = if app.forward.index >= visible_rows {
        app.forward.index - visible_rows + 1
    } else {
        0
    };
    let end = (scroll_offset + visible_rows).min(app.forward.filtered.len());

    if app.forward.filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No conversations found",
            Style::default().fg(theme.fg_muted),
        )));
    } else {
        for (i, (_id, name)) in app.forward.filtered[scroll_offset..end].iter().enumerate() {
            let actual_idx = scroll_offset + i;
            let is_selected = actual_idx == app.forward.index;
            let prefix = if is_selected { "> " } else { "  " };
            let style = if is_selected {
                list_overlay::selection_style(theme.bg_selected, theme.fg)
            } else {
                Style::default().fg(theme.fg)
            };
            lines.push(Line::from(Span::styled(format!("{prefix}{name}"), style)));
        }
    }

    // Footer
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Enter: forward | Esc: cancel",
        Style::default().fg(theme.fg_muted),
    )));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}
