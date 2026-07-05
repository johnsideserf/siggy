//! Fuzzy command palette overlay (#614).
//!
//! Ctrl-P style finder over conversations and slash commands. Every
//! printable key filters (fuzzy subsequence match, best-first); Up/Down
//! navigate; Enter jumps to the conversation or invokes the command.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::{PALETTE_MAX_VISIBLE, PALETTE_POPUP_WIDTH, centered_popup, truncate};
use crate::app::App;
use crate::domain::PaletteItem;
use crate::list_overlay;

pub(in crate::ui) fn draw_palette(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let max_visible = PALETTE_MAX_VISIBLE.min(app.palette.filtered.len()).max(1);
    let pref_height = max_visible as u16 + 5; // +3 border/title +2 footer

    let title = if app.palette.query.is_empty() {
        " Palette ".to_string()
    } else {
        format!(" Palette [{}] ", app.palette.query)
    };

    let (popup_area, block) =
        centered_popup(frame, area, PALETTE_POPUP_WIDTH, pref_height, &title, theme);

    let inner_height = popup_area.height.saturating_sub(2) as usize;
    let (visible_rows, scroll_offset) =
        list_overlay::scroll_layout(inner_height, 2, app.palette.index);

    let mut lines: Vec<Line> = Vec::new();

    if app.palette.filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No matches",
            Style::default().fg(theme.fg_muted),
        )));
    } else {
        let end = (scroll_offset + visible_rows).min(app.palette.filtered.len());
        let inner_w = popup_area.width.saturating_sub(2) as usize;

        for (i, item) in app.palette.filtered[scroll_offset..end].iter().enumerate() {
            let is_selected = scroll_offset + i == app.palette.index;
            let base = if is_selected {
                list_overlay::selection_style(theme.bg_selected, theme.fg)
            } else {
                Style::default().fg(theme.fg)
            };
            let muted = if is_selected {
                Style::default().bg(theme.bg_selected).fg(theme.fg_muted)
            } else {
                Style::default().fg(theme.fg_muted)
            };
            let accent = if is_selected {
                Style::default().bg(theme.bg_selected).fg(theme.accent)
            } else {
                Style::default().fg(theme.accent)
            };

            match item {
                PaletteItem::Conversation { name, is_group, .. } => {
                    let prefix = if *is_group { "  # " } else { "    " };
                    let name_max = inner_w.saturating_sub(prefix.len() + 7);
                    lines.push(Line::from(vec![
                        Span::styled(prefix.to_string(), muted),
                        Span::styled(truncate(name, name_max), base),
                        Span::styled("  chat", muted),
                    ]));
                }
                PaletteItem::Command {
                    name, description, ..
                } => {
                    let desc_max = inner_w.saturating_sub(name.len() + 5);
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {name}"), accent),
                        Span::styled(format!("  {}", truncate(description, desc_max)), muted),
                    ]));
                }
            }
        }
    }

    list_overlay::append_footer(
        &mut lines,
        visible_rows,
        "  type to filter  |  Up/Down navigate  |  Enter select  |  Esc close",
        theme.fg_muted,
    );

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}
