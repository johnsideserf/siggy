//! Confirmation prompt for `/delete` (remove the active conversation).
//!
//! Accepted conversations show "Delete conversation locally?" because no
//! network response is sent. Unaccepted message requests show "Delete and
//! decline request?" because the underlying handler emits a
//! `MessageRequestResponse { response_type: "delete" }` to signal-cli.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::centered_popup;
use crate::app::App;

pub(in crate::ui) fn draw_delete_conversation_confirm(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let accepted = app
        .active_conversation
        .as_deref()
        .and_then(|id| app.store.conversations.get(id))
        .map(|c| c.accepted)
        .unwrap_or(true);

    let prompt = if accepted {
        "Delete conversation locally? (y)es / (n)o"
    } else {
        "Delete conversation and decline request? (y)es / (n)o"
    };

    let width = (prompt.len() as u16).saturating_add(6);
    let (popup_area, block) = centered_popup(frame, area, width, 5, " Delete Conversation ", theme);

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
