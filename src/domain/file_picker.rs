//! File browser overlay used to pick attachments.
//!
//! Opens at the user's home directory and walks the filesystem with a
//! sorted directory-first listing. Type-to-filter narrows the visible
//! entries; Enter descends into a directory or selects a file (returned
//! as `FilePickerOutcome::Selected`). Backspace pops the filter or
//! navigates up when the filter is empty.

use std::path::PathBuf;

use crossterm::event::KeyCode;

/// Outcome of a key press inside the file browser overlay.
pub enum FilePickerOutcome {
    /// Picker should stay open for further navigation.
    Continue,
    /// User selected a file; caller should consume the path and close the overlay.
    Selected(PathBuf),
    /// User cancelled (Esc); caller should close the overlay.
    Cancelled,
}

/// State for the file browser overlay used to select attachments.
pub struct FilePickerState {
    /// Current directory in file browser
    pub dir: PathBuf,
    /// Directory entries: (name, is_dir, size_bytes)
    pub entries: Vec<(String, bool, u64)>,
    /// Cursor position in file browser
    pub index: usize,
    /// Type-to-filter text for file browser
    pub filter: String,
    /// Filtered indices into entries
    pub filtered: Vec<usize>,
    /// Error message from directory read
    pub error: Option<String>,
}

impl Default for FilePickerState {
    fn default() -> Self {
        Self {
            dir: dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
            entries: Vec::new(),
            index: 0,
            filter: String::new(),
            filtered: Vec::new(),
            error: None,
        }
    }
}

impl FilePickerState {
    /// Reset state for a fresh browse. Caller must also call
    /// `App::open_overlay` to make the picker visible.
    pub fn open(&mut self) {
        self.dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        self.index = 0;
        self.filter.clear();
        self.error = None;
        self.refresh_entries();
    }

    /// Read the current directory and populate entries.
    pub fn refresh_entries(&mut self) {
        self.entries.clear();
        self.error = None;
        match std::fs::read_dir(&self.dir) {
            Ok(read_entries) => {
                let mut dirs: Vec<(String, bool, u64)> = Vec::new();
                let mut files: Vec<(String, bool, u64)> = Vec::new();
                for entry in read_entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let meta = entry.metadata();
                    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                    if is_dir {
                        dirs.push((name, true, 0));
                    } else {
                        files.push((name, false, size));
                    }
                }
                dirs.sort_by_key(|a| a.0.to_lowercase());
                files.sort_by_key(|a| a.0.to_lowercase());
                self.entries.extend(dirs);
                self.entries.extend(files);
            }
            Err(e) => {
                self.error = Some(format!("Cannot read directory: {e}"));
            }
        }
        self.refresh_filter();
    }

    /// Rebuild the filtered index list based on current filter text.
    pub fn refresh_filter(&mut self) {
        let filter_lower = self.filter.to_lowercase();
        self.filtered = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, (name, _, _))| {
                filter_lower.is_empty() || name.to_lowercase().contains(&filter_lower)
            })
            .map(|(i, _)| i)
            .collect();
        if self.filtered.is_empty() {
            self.index = 0;
        } else if self.index >= self.filtered.len() {
            self.index = self.filtered.len() - 1;
        }
    }

    /// Handle a key press while the file browser overlay is open.
    pub fn handle_key(&mut self, code: KeyCode) -> FilePickerOutcome {
        match code {
            KeyCode::Char('j') | KeyCode::Down
                if !self.filtered.is_empty() && self.index < self.filtered.len() - 1 =>
            {
                self.index += 1;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.index = self.index.saturating_sub(1);
            }
            KeyCode::Enter => {
                if let Some(&entry_idx) = self.filtered.get(self.index) {
                    let (name, is_dir, _) = self.entries[entry_idx].clone();
                    if is_dir {
                        self.dir = self.dir.join(&name);
                        self.index = 0;
                        self.filter.clear();
                        self.refresh_entries();
                    } else {
                        return FilePickerOutcome::Selected(self.dir.join(&name));
                    }
                }
            }
            KeyCode::Backspace => {
                if !self.filter.is_empty() {
                    self.filter.pop();
                    self.refresh_filter();
                } else {
                    self.navigate_up();
                }
            }
            KeyCode::Char('-') => {
                self.navigate_up();
            }
            KeyCode::Esc => return FilePickerOutcome::Cancelled,
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.refresh_filter();
            }
            _ => {}
        }
        FilePickerOutcome::Continue
    }

    /// Navigate to the parent directory in the file browser.
    fn navigate_up(&mut self) {
        if let Some(parent) = self.dir.parent() {
            let parent = parent.to_path_buf();
            if parent != self.dir {
                self.dir = parent;
                self.index = 0;
                self.filter.clear();
                self.refresh_entries();
            }
        }
    }
}
