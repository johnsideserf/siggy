use crossterm::event::KeyCode;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Semantic action from a key press in a list overlay.
#[derive(Debug, PartialEq)]
pub enum ListKeyAction {
    Up,
    Down,
    Select,
    Close,
    FilterPush(char),
    FilterPop,
    None,
}

/// Classify a key press into a list navigation action.
/// When `filterable` is true, printable chars (other than j/k) become FilterPush
/// and Backspace becomes FilterPop. When false, they return None.
pub fn classify_list_key(code: KeyCode, filterable: bool) -> ListKeyAction {
    match code {
        KeyCode::Char('j') | KeyCode::Down => ListKeyAction::Down,
        KeyCode::Char('k') | KeyCode::Up => ListKeyAction::Up,
        KeyCode::Enter => ListKeyAction::Select,
        KeyCode::Esc => ListKeyAction::Close,
        KeyCode::Backspace if filterable => ListKeyAction::FilterPop,
        KeyCode::Char(c) if filterable => ListKeyAction::FilterPush(c),
        _ => ListKeyAction::None,
    }
}

/// Calculate visible rows and scroll offset for a list overlay.
/// `inner_height` is the popup inner area height (after border subtraction).
/// `footer_lines` is lines reserved for footer (typically 2: blank + help text).
/// Returns (visible_rows, scroll_offset).
pub fn scroll_layout(
    inner_height: usize,
    footer_lines: usize,
    selected_index: usize,
) -> (usize, usize) {
    let visible_rows = inner_height.saturating_sub(footer_lines);
    let scroll_offset = if selected_index >= visible_rows {
        selected_index - visible_rows + 1
    } else {
        0
    };
    (visible_rows, scroll_offset)
}

/// Pad lines to fill visible_rows, then append a blank line and styled footer.
pub fn append_footer(
    lines: &mut Vec<Line<'static>>,
    visible_rows: usize,
    footer_text: &str,
    muted_color: Color,
) {
    while lines.len() < visible_rows {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        footer_text.to_string(),
        Style::default().fg(muted_color),
    )));
}

/// Clamp a selection index to stay within list bounds.
pub fn clamp_index(index: &mut usize, list_len: usize) {
    if list_len == 0 {
        *index = 0;
    } else if *index >= list_len {
        *index = list_len - 1;
    }
}

/// Standard selection highlight style for list overlays.
pub fn selection_style(bg_selected: Color, fg: Color) -> Style {
    Style::default()
        .bg(bg_selected)
        .fg(fg)
        .add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jk_navigate() {
        assert_eq!(
            classify_list_key(KeyCode::Char('j'), false),
            ListKeyAction::Down
        );
        assert_eq!(
            classify_list_key(KeyCode::Char('k'), false),
            ListKeyAction::Up
        );
        assert_eq!(
            classify_list_key(KeyCode::Char('j'), true),
            ListKeyAction::Down
        );
        assert_eq!(
            classify_list_key(KeyCode::Char('k'), true),
            ListKeyAction::Up
        );
    }

    #[test]
    fn arrow_keys_navigate() {
        assert_eq!(
            classify_list_key(KeyCode::Down, false),
            ListKeyAction::Down
        );
        assert_eq!(classify_list_key(KeyCode::Up, false), ListKeyAction::Up);
    }

    #[test]
    fn enter_selects() {
        assert_eq!(
            classify_list_key(KeyCode::Enter, false),
            ListKeyAction::Select
        );
    }

    #[test]
    fn esc_closes() {
        assert_eq!(
            classify_list_key(KeyCode::Esc, false),
            ListKeyAction::Close
        );
    }

    #[test]
    fn filterable_chars() {
        assert_eq!(
            classify_list_key(KeyCode::Char('a'), true),
            ListKeyAction::FilterPush('a')
        );
        assert_eq!(
            classify_list_key(KeyCode::Backspace, true),
            ListKeyAction::FilterPop
        );
    }

    #[test]
    fn non_filterable_chars_are_none() {
        assert_eq!(
            classify_list_key(KeyCode::Char('a'), false),
            ListKeyAction::None
        );
        assert_eq!(
            classify_list_key(KeyCode::Backspace, false),
            ListKeyAction::None
        );
    }

    #[test]
    fn scroll_layout_fits_viewport() {
        let (visible, offset) = scroll_layout(10, 2, 3);
        assert_eq!(visible, 8);
        assert_eq!(offset, 0);
    }

    #[test]
    fn scroll_layout_needs_scroll() {
        let (visible, offset) = scroll_layout(10, 2, 10);
        assert_eq!(visible, 8);
        assert_eq!(offset, 3); // 10 - 8 + 1
    }

    #[test]
    fn clamp_index_empty_list() {
        let mut idx = 5;
        clamp_index(&mut idx, 0);
        assert_eq!(idx, 0);
    }

    #[test]
    fn clamp_index_within_bounds() {
        let mut idx = 3;
        clamp_index(&mut idx, 10);
        assert_eq!(idx, 3);
    }

    #[test]
    fn clamp_index_over_bounds() {
        let mut idx = 15;
        clamp_index(&mut idx, 10);
        assert_eq!(idx, 9);
    }
}
