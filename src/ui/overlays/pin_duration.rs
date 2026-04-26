//! Pin duration picker overlay.
//!
//! Lists the choices from `PIN_DURATIONS` (5m / 1h / 1d / 1w / forever)
//! with a cursor marker; the chosen duration is applied when Enter is
//! pressed and tells signal-cli how long to keep the message pinned.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::centered_popup;
use crate::app::{App, PIN_DURATIONS};
use crate::list_overlay;

pub(in crate::ui) fn draw_pin_duration_picker(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let item_count = PIN_DURATIONS.len();
    let popup_height = item_count as u16 + 4; // borders + footer

    let (popup_area, block) =
        centered_popup(frame, area, 24, popup_height, " Pin Duration ", theme);

    let mut lines: Vec<Line> = Vec::new();

    for (i, (_seconds, label)) in PIN_DURATIONS.iter().enumerate() {
        let style = if i == app.pin_duration.index {
            list_overlay::selection_style(theme.bg_selected, theme.fg)
        } else {
            Style::default().fg(theme.fg)
        };
        let marker = if i == app.pin_duration.index {
            ">"
        } else {
            " "
        };
        lines.push(Line::from(Span::styled(
            format!(" {marker} {label}"),
            style,
        )));
    }

    list_overlay::append_footer(&mut lines, item_count, " j/k  Enter  Esc", theme.fg_muted);

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}
