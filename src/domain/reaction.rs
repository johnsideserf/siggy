//! Reaction display preferences and reaction picker state.
//!
//! `picker_index` tracks the cursor in the reaction picker overlay.
//! The remaining flags shape rendering: `show_reactions` toggles the
//! reaction badge entirely, `verbose` swaps counts for usernames, and
//! `emoji_to_text` rewrites emoji as text shortcodes for terminals
//! that render emoji poorly.

/// State for reaction display preferences and the reaction picker overlay.
#[derive(Default)]
pub struct ReactionState {
    /// Selected index in the reaction picker
    pub picker_index: usize,
    /// Convert emoji to text emoticons/shortcodes in display
    pub emoji_to_text: bool,
    /// Show emoji reactions on messages
    pub show_reactions: bool,
    /// Show verbose reaction display (usernames instead of counts)
    pub verbose: bool,
}

impl ReactionState {
    pub fn new() -> Self {
        Self {
            show_reactions: true,
            ..Default::default()
        }
    }
}
