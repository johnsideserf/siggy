//! Shared helpers for j/k/Enter/Esc list overlays.
//!
//! Most overlays in [`crate::domain`] expose the same navigation pattern;
//! this module centralizes key resolution ([`ListKeyAction`]) and the
//! styled-row rendering helpers used by the UI layer.

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

/// Apply the Up/Down arms of a `ListKeyAction` to a list overlay's cursor.
/// Returns `true` if the action was a navigation arm (consumed), `false`
/// otherwise so callers can fall through to mode-specific arms.
///
/// Down on an empty list (`len == 0`) is a consumed no-op rather than an
/// underflow risk -- previous handlers had inconsistent empty-list guards.
pub fn apply_nav(action: &ListKeyAction, index: &mut usize, len: usize) -> bool {
    match action {
        ListKeyAction::Up => {
            *index = index.saturating_sub(1);
            true
        }
        ListKeyAction::Down => {
            if len > 0 {
                *index = (*index + 1).min(len - 1);
            }
            true
        }
        _ => false,
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

/// Filter `(id, name)` pairs by a case-insensitive substring match on either
/// field (an empty filter matches all), sorted by name. This is the shared
/// body of the contacts / add-member / remove-member type-to-filter pickers
/// (#499); callers pre-build the source pairs with their own exclusions
/// (non-empty name, existing members, self) and pass them in.
pub fn filter_name_number_pairs(
    pairs: impl IntoIterator<Item = (String, String)>,
    filter: &str,
) -> Vec<(String, String)> {
    let filter = filter.to_lowercase();
    let mut out: Vec<(String, String)> = pairs
        .into_iter()
        .filter(|(number, name)| {
            filter.is_empty()
                || name.to_lowercase().contains(&filter)
                || number.to_lowercase().contains(&filter)
        })
        .collect();
    out.sort_by_key(|(_, name)| name.to_lowercase());
    out
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

/// Score `haystack` against a fuzzy `needle` (case-insensitive subsequence
/// match, greedy left-to-right). Returns `None` when the needle is not a
/// subsequence of the haystack; higher scores are better. Scoring favors
/// consecutive runs, matches at word starts, and matches that begin early.
/// An empty needle matches everything with score 0. Used by the command
/// palette (#614).
pub fn fuzzy_score(needle: &str, haystack: &str) -> Option<i64> {
    if needle.trim().is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = haystack.to_lowercase().chars().collect();
    let mut score: i64 = 0;
    let mut hi = 0usize;
    let mut prev_match: Option<usize> = None;
    let mut first_match: Option<usize> = None;
    for nc in needle.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        while hi < hay.len() && hay[hi] != nc {
            hi += 1;
        }
        if hi >= hay.len() {
            return None;
        }
        score += 1;
        if prev_match == Some(hi.wrapping_sub(1)) && hi > 0 {
            score += 3; // consecutive-run bonus
        }
        if hi == 0 || !hay[hi - 1].is_alphanumeric() {
            score += 2; // word-start bonus
        }
        first_match.get_or_insert(hi);
        prev_match = Some(hi);
        hi += 1;
    }
    // Prefer matches that start early and shorter haystacks overall.
    score -= (first_match.unwrap_or(0) as i64).min(8);
    score -= (hay.len() as i64) / 16;
    Some(score)
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
        assert_eq!(classify_list_key(KeyCode::Down, false), ListKeyAction::Down);
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
    fn fuzzy_score_subsequence_and_ordering() {
        // Non-subsequence rejects; subsequence matches.
        assert_eq!(fuzzy_score("xyz", "contacts"), None);
        assert!(fuzzy_score("cts", "contacts").is_some());
        // Case-insensitive; empty needle matches everything.
        assert!(fuzzy_score("ALI", "Alice").is_some());
        assert_eq!(fuzzy_score("", "anything"), Some(0));
        assert_eq!(fuzzy_score("   ", "anything"), Some(0));

        // Prefix beats scattered subsequence.
        assert!(
            fuzzy_score("con", "/contacts").unwrap()
                > fuzzy_score("con", "welcome once more").unwrap()
        );
        // Word-start match beats mid-word match.
        assert!(fuzzy_score("al", "Alice").unwrap() > fuzzy_score("al", "Natalie").unwrap());
        // Consecutive beats gapped.
        assert!(
            fuzzy_score("arch", "/archive").unwrap()
                > fuzzy_score("arch", "a routine chore here").unwrap()
        );
    }

    #[test]
    fn esc_closes() {
        assert_eq!(classify_list_key(KeyCode::Esc, false), ListKeyAction::Close);
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
    fn filter_name_number_pairs_matches_name_or_number_and_sorts() {
        let pairs = vec![
            ("+199".to_string(), "Zoe".to_string()),
            ("+1234".to_string(), "Alice".to_string()),
            ("+1999".to_string(), "Bob".to_string()),
        ];
        // Empty filter: all, sorted by name.
        let all = filter_name_number_pairs(pairs.clone(), "");
        assert_eq!(
            all.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>(),
            ["Alice", "Bob", "Zoe"]
        );
        // Name match (case-insensitive).
        let by_name = filter_name_number_pairs(pairs.clone(), "ALI");
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].1, "Alice");
        // Number match: "+199" appears in Zoe (+199) and Bob (+1999).
        let by_num = filter_name_number_pairs(pairs, "+199");
        assert_eq!(
            by_num.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>(),
            ["Bob", "Zoe"]
        );
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

    #[test]
    fn apply_nav_up_decrements_with_floor() {
        let mut idx = 3;
        assert!(apply_nav(&ListKeyAction::Up, &mut idx, 10));
        assert_eq!(idx, 2);

        let mut idx = 0;
        assert!(apply_nav(&ListKeyAction::Up, &mut idx, 10));
        assert_eq!(idx, 0, "saturating: must not underflow");
    }

    #[test]
    fn apply_nav_down_increments_with_ceiling() {
        let mut idx = 3;
        assert!(apply_nav(&ListKeyAction::Down, &mut idx, 10));
        assert_eq!(idx, 4);

        let mut idx = 9;
        assert!(apply_nav(&ListKeyAction::Down, &mut idx, 10));
        assert_eq!(idx, 9, "must clamp to len - 1");
    }

    #[test]
    fn apply_nav_down_on_empty_list_is_noop() {
        // Regression for the bug-risk flagged by the unification review:
        // empty filtered list must not panic on a Down keypress.
        let mut idx = 0;
        assert!(apply_nav(&ListKeyAction::Down, &mut idx, 0));
        assert_eq!(idx, 0);
    }

    #[test]
    fn apply_nav_non_nav_actions_return_false() {
        let mut idx = 5;
        assert!(!apply_nav(&ListKeyAction::Select, &mut idx, 10));
        assert!(!apply_nav(&ListKeyAction::Close, &mut idx, 10));
        assert!(!apply_nav(&ListKeyAction::FilterPush('a'), &mut idx, 10));
        assert!(!apply_nav(&ListKeyAction::FilterPop, &mut idx, 10));
        assert!(!apply_nav(&ListKeyAction::None, &mut idx, 10));
        assert_eq!(idx, 5, "non-nav actions must not move the cursor");
    }
}
