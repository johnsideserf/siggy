//! Emoji picker overlay (full search).
//!
//! Category tab row across the top, scrolling grid in the middle,
//! and a name/shortcode preview line. Search filter narrows across
//! all categories. Reachable from the input composer (`:`) or the
//! quick reaction picker (`e`).

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::{EMOJI_POPUP_HEIGHT, EMOJI_POPUP_WIDTH, centered_popup};
use crate::app::App;
use crate::domain::CATEGORIES;
use crate::list_overlay;

pub(in crate::ui) fn draw_emoji_picker(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    let title = if app.emoji_picker.filter.is_empty() {
        " Emoji ".to_string()
    } else {
        format!(" Emoji [{}] ", app.emoji_picker.filter)
    };

    let (popup_area, block) = centered_popup(
        frame,
        area,
        EMOJI_POPUP_WIDTH,
        EMOJI_POPUP_HEIGHT,
        &title,
        theme,
    );

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if inner.height < 5 || inner.width < 10 {
        return;
    }

    let cols = app.emoji_picker.cols;

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Category tab row
    let mut cat_spans: Vec<Span<'static>> = Vec::new();
    for (i, (icon, _label)) in CATEGORIES.iter().enumerate() {
        let style = if i == app.emoji_picker.category_index {
            Style::default()
                .bg(theme.bg_selected)
                .fg(theme.fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg_muted)
        };
        cat_spans.push(Span::styled(format!(" {icon} "), style));
    }
    lines.push(Line::from(cat_spans));

    // Separator
    let sep = "\u{2500}".repeat(inner.width as usize);
    lines.push(Line::from(Span::styled(
        sep,
        Style::default().fg(theme.fg_muted),
    )));

    // Grid dimensions
    let footer_lines = 3; // blank + preview + help
    let grid_height = (inner.height as usize).saturating_sub(lines.len() + footer_lines);

    // Scroll to keep selection visible
    let selected_row = app.emoji_picker.selected_index / cols;
    let scroll_offset = if selected_row >= grid_height {
        selected_row - grid_height + 1
    } else {
        0
    };

    // Render grid rows
    let total_rows = app.emoji_picker.filtered.len().div_ceil(cols);
    for row_idx in scroll_offset..(scroll_offset + grid_height).min(total_rows) {
        let mut row_spans: Vec<Span<'static>> = vec![Span::raw(" ".to_string())];
        for col_idx in 0..cols {
            let emoji_idx = row_idx * cols + col_idx;
            if emoji_idx >= app.emoji_picker.filtered.len() {
                break;
            }
            let entry = &app.emoji_picker.filtered[emoji_idx];
            let style = if emoji_idx == app.emoji_picker.selected_index {
                list_overlay::selection_style(theme.bg_selected, theme.fg)
            } else {
                Style::default()
            };
            row_spans.push(Span::styled(format!("{} ", entry.emoji), style));
        }
        lines.push(Line::from(row_spans));
    }

    // Pad remaining grid rows
    while lines.len() < (inner.height as usize).saturating_sub(footer_lines) {
        lines.push(Line::from(""));
    }

    // Preview line: name + shortcode of selected emoji
    if let Some(entry) = app
        .emoji_picker
        .filtered
        .get(app.emoji_picker.selected_index)
    {
        let preview = if let Some(sc) = entry.shortcode {
            format!("{} :{sc}: - {}", entry.emoji, entry.name)
        } else {
            format!("{} - {}", entry.emoji, entry.name)
        };
        lines.push(Line::from(Span::styled(
            preview,
            Style::default().fg(theme.accent),
        )));
    } else {
        lines.push(Line::from(""));
    }

    // Footer
    let footer = if app.emoji_picker.filtered.is_empty() {
        " no matches | Tab: category | Esc: close"
    } else {
        " Tab: category | arrows/hjkl: nav | type to filter | Esc"
    };
    lines.push(Line::from(Span::styled(
        footer.to_string(),
        Style::default().fg(theme.fg_muted),
    )));

    let popup = Paragraph::new(lines);
    frame.render_widget(popup, inner);
}
