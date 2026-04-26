//! Theme picker overlay.
//!
//! Lists available themes with `[*]` marking the active one and three
//! coloured swatches (accent / success / error) per row so the user
//! can preview before applying. Uses `list_overlay` helpers for
//! scroll layout and footer.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::{centered_popup, truncate};
use crate::app::App;
use crate::list_overlay;

pub(in crate::ui) fn draw_theme_picker(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let max_visible = 12usize.min(app.theme_picker.available_themes.len());
    let pref_height = max_visible as u16 + 5; // border + title + footer

    let (popup_area, block) = centered_popup(frame, area, 50, pref_height, " Theme ", theme);

    let inner_height = popup_area.height.saturating_sub(2) as usize;
    let (visible_rows, scroll_offset) =
        list_overlay::scroll_layout(inner_height, 2, app.theme_picker.index);

    let mut lines: Vec<Line> = Vec::new();

    let end = (scroll_offset + visible_rows).min(app.theme_picker.available_themes.len());
    for (i, t) in app.theme_picker.available_themes[scroll_offset..end]
        .iter()
        .enumerate()
    {
        let actual_index = scroll_offset + i;
        let is_selected = actual_index == app.theme_picker.index;
        let is_active = t.name == app.theme.name;

        let marker = if is_active { "[*]" } else { "[ ]" };
        let row_style = if is_selected {
            list_overlay::selection_style(theme.bg_selected, theme.fg)
        } else {
            Style::default().fg(theme.fg)
        };
        let marker_style = if is_selected {
            Style::default().bg(theme.bg_selected).fg(if is_active {
                theme.success
            } else {
                theme.fg_muted
            })
        } else {
            Style::default().fg(if is_active {
                theme.success
            } else {
                theme.fg_muted
            })
        };

        // Color swatches: show accent, success, error as colored blocks
        let swatch_bg = if is_selected {
            theme.bg_selected
        } else {
            theme.bg
        };
        let swatch_accent = Span::styled(
            "\u{2588}\u{2588}",
            Style::default().fg(t.accent).bg(swatch_bg),
        );
        let swatch_success = Span::styled(
            "\u{2588}\u{2588}",
            Style::default().fg(t.success).bg(swatch_bg),
        );
        let swatch_error = Span::styled(
            "\u{2588}\u{2588}",
            Style::default().fg(t.error).bg(swatch_bg),
        );

        // Pad name to align swatches
        let name_width = 28;
        let display_name = truncate(&t.name, name_width);
        let padded_name = format!("{display_name:width$}", width = name_width);

        lines.push(Line::from(vec![
            Span::styled(format!("  {marker} "), marker_style),
            Span::styled(padded_name, row_style),
            Span::raw(" "),
            swatch_accent,
            Span::raw(" "),
            swatch_success,
            Span::raw(" "),
            swatch_error,
        ]));
    }

    list_overlay::append_footer(
        &mut lines,
        visible_rows,
        "  j/k navigate  |  Enter apply  |  Esc cancel",
        theme.fg_muted,
    );

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}
