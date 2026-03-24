use crossterm::event::KeyCode;

/// Context the emoji picker was opened from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmojiPickerSource {
    /// Insert emoji into the message input buffer at cursor.
    Input,
    /// Use selected emoji as a reaction on the focused message.
    Reaction,
}

/// Action returned by the emoji picker key handler.
pub enum EmojiPickerAction {
    /// User selected an emoji string.
    Select(String),
    /// User closed the picker without selecting.
    Close,
    /// No action (internal navigation / filter change).
    None,
}

/// A cached entry in the filtered emoji list.
#[derive(Clone, Debug)]
pub struct EmojiEntry {
    pub emoji: &'static str,
    pub shortcode: Option<&'static str>,
    pub name: &'static str,
}

/// Category labels and representative icons for the tab bar.
/// Index 0 is "All", 1-9 map to `emojis::Group` variants in order.
pub const CATEGORIES: &[(&str, &str)] = &[
    ("*", "All"),
    ("\u{1f600}", "Smileys & Emotion"),
    ("\u{1f44b}", "People & Body"),
    ("\u{1f43b}", "Animals & Nature"),
    ("\u{1f354}", "Food & Drink"),
    ("\u{2708}\u{fe0f}", "Travel & Places"),
    ("\u{26bd}", "Activities"),
    ("\u{1f4a1}", "Objects"),
    ("\u{2764}\u{fe0f}", "Symbols"),
    ("\u{1f3c1}", "Flags"),
];

/// Map a category index (1-9) to the corresponding `emojis::Group`.
fn category_to_group(index: usize) -> Option<emojis::Group> {
    match index {
        1 => Some(emojis::Group::SmileysAndEmotion),
        2 => Some(emojis::Group::PeopleAndBody),
        3 => Some(emojis::Group::AnimalsAndNature),
        4 => Some(emojis::Group::FoodAndDrink),
        5 => Some(emojis::Group::TravelAndPlaces),
        6 => Some(emojis::Group::Activities),
        7 => Some(emojis::Group::Objects),
        8 => Some(emojis::Group::Symbols),
        9 => Some(emojis::Group::Flags),
        _ => None, // 0 = All
    }
}

/// State for the emoji picker overlay.
pub struct EmojiPickerState {
    pub visible: bool,
    pub source: EmojiPickerSource,
    pub filter: String,
    pub selected_index: usize,
    pub category_index: usize,
    pub filtered: Vec<EmojiEntry>,
    /// Grid columns for keyboard navigation (derived from EMOJI_POPUP_WIDTH).
    pub cols: usize,
}

impl Default for EmojiPickerState {
    fn default() -> Self {
        Self {
            visible: false,
            source: EmojiPickerSource::Input,
            filter: String::new(),
            selected_index: 0,
            category_index: 0,
            filtered: Vec::new(),
            cols: 16,
        }
    }
}

impl EmojiPickerState {
    /// Open the picker for a given source context, with an optional initial search filter.
    pub fn open(&mut self, source: EmojiPickerSource, filter: Option<String>) {
        self.visible = true;
        self.source = source;
        self.filter = filter.unwrap_or_default();
        self.selected_index = 0;
        self.category_index = 0;
        self.refresh_filter();
    }

    /// Close the picker and reset state.
    pub fn close(&mut self) {
        self.visible = false;
        self.filter.clear();
        self.selected_index = 0;
        self.category_index = 0;
        self.filtered.clear();
    }

    /// Rebuild the filtered emoji list from the current category and filter text.
    pub fn refresh_filter(&mut self) {
        let filter_lower = self.filter.to_lowercase();
        let group = if filter_lower.is_empty() {
            category_to_group(self.category_index)
        } else {
            // When filtering by text, ignore category and search all
            None
        };

        self.filtered.clear();

        for emoji in emojis::iter() {
            // Skip skin-tone variants (they clutter the grid)
            if emoji.skin_tone().is_some() {
                continue;
            }

            // Category filter
            if let Some(ref g) = group {
                if emoji.group() != *g {
                    continue;
                }
            }

            // Text filter (match against name and shortcode)
            if !filter_lower.is_empty() {
                let name_match = emoji.name().to_lowercase().contains(&filter_lower);
                let sc_match = emoji
                    .shortcode()
                    .is_some_and(|sc| sc.to_lowercase().contains(&filter_lower));
                if !name_match && !sc_match {
                    continue;
                }
            }

            self.filtered.push(EmojiEntry {
                emoji: emoji.as_str(),
                shortcode: emoji.shortcode(),
                name: emoji.name(),
            });
        }

        // Clamp selection
        if self.filtered.is_empty() {
            self.selected_index = 0;
        } else if self.selected_index >= self.filtered.len() {
            self.selected_index = self.filtered.len() - 1;
        }
    }

    /// Return the emoji string at the current selection, if any.
    pub fn selected_emoji(&self) -> Option<&str> {
        self.filtered.get(self.selected_index).map(|e| e.emoji)
    }

    /// Handle a key press. Returns an action for the caller to dispatch.
    pub fn handle_key(&mut self, code: KeyCode) -> EmojiPickerAction {
        match code {
            // Confirm selection
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(emoji) = self.selected_emoji() {
                    EmojiPickerAction::Select(emoji.to_string())
                } else {
                    EmojiPickerAction::None
                }
            }
            // Close
            KeyCode::Esc => EmojiPickerAction::Close,

            // Grid navigation (h/j/k/l are consumed here and cannot appear in filter text;
            // arrow keys also work for navigation while typing filter text)
            KeyCode::Char('h') | KeyCode::Left => {
                self.selected_index = self.selected_index.saturating_sub(1);
                EmojiPickerAction::None
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if !self.filtered.is_empty() && self.selected_index < self.filtered.len() - 1 {
                    self.selected_index += 1;
                }
                EmojiPickerAction::None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let new_idx = self.selected_index + self.cols;
                if new_idx < self.filtered.len() {
                    self.selected_index = new_idx;
                }
                EmojiPickerAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected_index = self.selected_index.saturating_sub(self.cols);
                EmojiPickerAction::None
            }

            // Category cycling
            KeyCode::Tab => {
                self.category_index = (self.category_index + 1) % CATEGORIES.len();
                self.refresh_filter();
                EmojiPickerAction::None
            }
            KeyCode::BackTab => {
                if self.category_index == 0 {
                    self.category_index = CATEGORIES.len() - 1;
                } else {
                    self.category_index -= 1;
                }
                self.refresh_filter();
                EmojiPickerAction::None
            }

            // Type-to-filter (any other printable char)
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.refresh_filter();
                EmojiPickerAction::None
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.refresh_filter();
                EmojiPickerAction::None
            }

            _ => EmojiPickerAction::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_populates_filtered_list() {
        let mut state = EmojiPickerState::default();
        state.open(EmojiPickerSource::Input, None);
        assert!(state.visible);
        assert!(!state.filtered.is_empty());
        assert_eq!(state.selected_index, 0);
        assert_eq!(state.category_index, 0);
    }

    #[test]
    fn filter_narrows_results() {
        let mut state = EmojiPickerState::default();
        state.open(EmojiPickerSource::Input, None);
        let all_count = state.filtered.len();

        state.filter = "rocket".to_string();
        state.refresh_filter();
        assert!(state.filtered.len() < all_count);
        assert!(state.filtered.iter().any(|e| e.emoji == "\u{1f680}"));
    }

    #[test]
    fn category_filters_emojis() {
        let mut state = EmojiPickerState::default();
        state.open(EmojiPickerSource::Input, None);
        let all_count = state.filtered.len();

        // Switch to Flags category (index 9)
        state.category_index = 9;
        state.refresh_filter();
        assert!(state.filtered.len() < all_count);
        // All entries should be flags
        for entry in &state.filtered {
            let emoji = emojis::get(entry.emoji).unwrap();
            assert_eq!(emoji.group(), emojis::Group::Flags);
        }
    }

    #[test]
    fn text_filter_ignores_category() {
        let mut state = EmojiPickerState::default();
        state.open(EmojiPickerSource::Input, None);

        // Set a category first
        state.category_index = 9; // Flags
        state.filter = "smile".to_string();
        state.refresh_filter();

        // Should find smiley emojis even though category is Flags
        assert!(state.filtered.iter().any(|e| e.name.to_lowercase().contains("smile")));
    }

    #[test]
    fn grid_navigation_down_moves_by_cols() {
        let mut state = EmojiPickerState::default();
        state.open(EmojiPickerSource::Input, None);
        state.cols = 8;
        assert_eq!(state.selected_index, 0);

        state.handle_key(KeyCode::Down);
        assert_eq!(state.selected_index, 8);

        state.handle_key(KeyCode::Down);
        assert_eq!(state.selected_index, 16);
    }

    #[test]
    fn grid_navigation_up_moves_by_cols() {
        let mut state = EmojiPickerState::default();
        state.open(EmojiPickerSource::Input, None);
        state.cols = 8;
        state.selected_index = 16;

        state.handle_key(KeyCode::Up);
        assert_eq!(state.selected_index, 8);

        state.handle_key(KeyCode::Up);
        assert_eq!(state.selected_index, 0);

        // Can't go below 0
        state.handle_key(KeyCode::Up);
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn grid_navigation_left_right() {
        let mut state = EmojiPickerState::default();
        state.open(EmojiPickerSource::Input, None);
        assert_eq!(state.selected_index, 0);

        state.handle_key(KeyCode::Right);
        assert_eq!(state.selected_index, 1);

        state.handle_key(KeyCode::Left);
        assert_eq!(state.selected_index, 0);

        // Can't go below 0
        state.handle_key(KeyCode::Left);
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn enter_returns_select() {
        let mut state = EmojiPickerState::default();
        state.open(EmojiPickerSource::Input, None);

        let action = state.handle_key(KeyCode::Enter);
        assert!(matches!(action, EmojiPickerAction::Select(_)));
    }

    #[test]
    fn esc_returns_close() {
        let mut state = EmojiPickerState::default();
        state.open(EmojiPickerSource::Input, None);

        let action = state.handle_key(KeyCode::Esc);
        assert!(matches!(action, EmojiPickerAction::Close));
    }

    #[test]
    fn tab_cycles_categories() {
        let mut state = EmojiPickerState::default();
        state.open(EmojiPickerSource::Input, None);
        assert_eq!(state.category_index, 0);

        state.handle_key(KeyCode::Tab);
        assert_eq!(state.category_index, 1);

        // Cycle to end and wrap
        for _ in 0..CATEGORIES.len() - 1 {
            state.handle_key(KeyCode::Tab);
        }
        assert_eq!(state.category_index, 0);
    }

    #[test]
    fn backtab_cycles_categories_reverse() {
        let mut state = EmojiPickerState::default();
        state.open(EmojiPickerSource::Input, None);
        assert_eq!(state.category_index, 0);

        state.handle_key(KeyCode::BackTab);
        assert_eq!(state.category_index, CATEGORIES.len() - 1);
    }

    #[test]
    fn close_resets_state() {
        let mut state = EmojiPickerState::default();
        state.open(EmojiPickerSource::Reaction, None);
        state.filter = "test".to_string();
        state.selected_index = 5;
        state.category_index = 3;

        state.close();
        assert!(!state.visible);
        assert!(state.filter.is_empty());
        assert_eq!(state.selected_index, 0);
        assert_eq!(state.category_index, 0);
        assert!(state.filtered.is_empty());
    }

    #[test]
    fn selected_emoji_returns_none_when_empty() {
        let mut state = EmojiPickerState {
            filter: "zzzzzznotanemoji".to_string(),
            ..EmojiPickerState::default()
        };
        state.refresh_filter();
        assert!(state.filtered.is_empty());
        assert!(state.selected_emoji().is_none());
    }

    #[test]
    fn typing_char_appends_to_filter() {
        let mut state = EmojiPickerState::default();
        state.open(EmojiPickerSource::Input, None);

        // 'r', 'o', 'c' are not h/l/j/k so they go to filter
        // but wait — these are mapped to navigation via Char match arms
        // Actually 'r', 'o', 'c' are NOT h/l/j/k so they hit the Char(c) fallthrough
        state.handle_key(KeyCode::Char('r'));
        assert_eq!(state.filter, "r");

        state.handle_key(KeyCode::Char('o'));
        assert_eq!(state.filter, "ro");
    }
}
