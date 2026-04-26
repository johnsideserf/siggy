//! Delete-message confirmation overlay.
//!
//! Prompts to delete the focused message. For outgoing messages the
//! prompt offers "delete for everyone" (`y`), "local only" (`l`), or
//! cancel (`n`); for incoming messages only "local only" is valid.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::centered_popup;
use crate::app::App;

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
