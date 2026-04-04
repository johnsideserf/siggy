/// Which autocomplete mode is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutocompleteMode {
    #[default]
    Command,
    Mention,
    Join,
}

/// Autocomplete popup state: candidates, selection index, and pending mentions.
pub struct AutocompleteState {
    /// Autocomplete popup visible
    pub visible: bool,
    /// Indices into COMMANDS for current matches
    pub command_candidates: Vec<usize>,
    /// Selected item in autocomplete popup
    pub index: usize,
    /// Current autocomplete mode (Command vs Mention vs Join)
    pub mode: AutocompleteMode,
    /// Mention autocomplete candidates: (phone, display_name, uuid)
    pub mention_candidates: Vec<(String, String, Option<String>)>,
    /// Join autocomplete candidates: (display_text, completion_value)
    pub join_candidates: Vec<(String, String)>,
    /// Byte offset of the '@' trigger in input_buffer
    pub mention_trigger_pos: usize,
    /// Completed mentions for the current input: (display_name, uuid)
    pub pending_mentions: Vec<(String, Option<String>)>,
}

impl Default for AutocompleteState {
    fn default() -> Self {
        Self::new()
    }
}

impl AutocompleteState {
    pub fn new() -> Self {
        Self {
            visible: false,
            command_candidates: Vec::new(),
            index: 0,
            mode: AutocompleteMode::Command,
            mention_candidates: Vec::new(),
            join_candidates: Vec::new(),
            mention_trigger_pos: 0,
            pending_mentions: Vec::new(),
        }
    }

    /// Whether there are no candidates in the current mode.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of candidates in the current mode.
    pub fn len(&self) -> usize {
        match self.mode {
            AutocompleteMode::Command => self.command_candidates.len(),
            AutocompleteMode::Mention => self.mention_candidates.len(),
            AutocompleteMode::Join => self.join_candidates.len(),
        }
    }

    /// Clear all candidates and hide the popup.
    pub fn clear(&mut self) {
        self.visible = false;
        self.command_candidates.clear();
        self.mention_candidates.clear();
        self.join_candidates.clear();
        self.index = 0;
    }
}
