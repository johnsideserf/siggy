//! About overlay: version, author, license, repo link.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::{ABOUT_POPUP_WIDTH, centered_popup};
use crate::app::App;

pub(in crate::ui) fn draw_about(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let version = env!("CARGO_PKG_VERSION");

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  siggy v{version}"),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  A terminal Signal messenger client",
            Style::default().fg(theme.fg),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Created by John Sideserf",
            Style::default().fg(theme.fg_secondary),
        )),
        Line::from(Span::styled(
            "  License: GPL-3.0",
            Style::default().fg(theme.fg_secondary),
        )),
        Line::from(Span::styled(
            "  github.com/johnsideserf/siggy",
            Style::default().fg(theme.link),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Press any key to close",
            Style::default().fg(theme.fg_muted),
        )),
    ];

    let pref_height = lines.len() as u16 + 2; // +2 for borders
    let (popup_area, block) = centered_popup(
        frame,
        area,
        ABOUT_POPUP_WIDTH,
        pref_height,
        " About ",
        theme,
    );

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}
