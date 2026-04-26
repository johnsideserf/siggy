//! Contacts browser overlay.
//!
//! Type-to-filter list of phone-number / display-name pairs. Shows a
//! green checkmark next to contacts that already have a conversation
//! and tints those names slightly muted. Selecting one opens that
//! conversation.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::{CONTACTS_MAX_VISIBLE, CONTACTS_POPUP_WIDTH, centered_popup, truncate};
use crate::app::App;
use crate::list_overlay;

pub(in crate::ui) fn draw_contacts(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let max_visible = CONTACTS_MAX_VISIBLE.min(app.contacts_overlay.filtered.len());
    let pref_height = max_visible as u16 + 5; // +3 border/title +2 footer/filter

    let title = if app.contacts_overlay.filter.is_empty() {
        " Contacts ".to_string()
    } else {
        format!(" Contacts [{}] ", app.contacts_overlay.filter)
    };

    let (popup_area, block) = centered_popup(
        frame,
        area,
        CONTACTS_POPUP_WIDTH,
        pref_height,
        &title,
        theme,
    );

    let inner_height = popup_area.height.saturating_sub(2) as usize; // minus borders
    let (visible_rows, scroll_offset) =
        list_overlay::scroll_layout(inner_height, 2, app.contacts_overlay.index);

    let mut lines: Vec<Line> = Vec::new();

    if app.contacts_overlay.filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No contacts found",
            Style::default().fg(theme.fg_muted),
        )));
    } else {
        let end = (scroll_offset + visible_rows).min(app.contacts_overlay.filtered.len());
        let inner_w = popup_area.width.saturating_sub(2) as usize;

        for (i, (number, name)) in app.contacts_overlay.filtered[scroll_offset..end]
            .iter()
            .enumerate()
        {
            let actual_index = scroll_offset + i;
            let is_selected = actual_index == app.contacts_overlay.index;
            let has_conversation = app.store.conversation_order.contains(number);

            // Checkmark for contacts that already have a conversation
            let marker = if has_conversation { " \u{2713}" } else { "  " };
            let marker_style = if has_conversation {
                Style::default().fg(theme.success)
            } else {
                Style::default()
            };

            // Truncate name to fit with number and marker
            let number_display = format!("  {}", number);
            let name_max = inner_w.saturating_sub(number_display.len() + marker.len() + 2);
            let display_name = truncate(name, name_max);

            let name_style = if is_selected {
                list_overlay::selection_style(theme.bg_selected, theme.fg)
            } else if has_conversation {
                Style::default().fg(theme.fg_secondary)
            } else {
                Style::default().fg(theme.fg)
            };
            let number_style = if is_selected {
                Style::default().bg(theme.bg_selected).fg(theme.accent)
            } else {
                Style::default().fg(theme.fg_muted)
            };
            let marker_bg = if is_selected {
                marker_style.bg(theme.bg_selected)
            } else {
                marker_style
            };

            lines.push(Line::from(vec![
                Span::styled(format!("  {}", display_name), name_style),
                Span::styled(number_display, number_style),
                Span::styled(marker.to_string(), marker_bg),
            ]));
        }
    }

    list_overlay::append_footer(
        &mut lines,
        visible_rows,
        "  j/k navigate  |  Enter select  |  Esc close",
        theme.fg_muted,
    );

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}
