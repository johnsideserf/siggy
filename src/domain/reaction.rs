/// State for reaction display preferences and the reaction picker overlay.
#[derive(Default)]
pub struct ReactionState {
    /// Reaction picker overlay visible
    pub show_picker: bool,
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
