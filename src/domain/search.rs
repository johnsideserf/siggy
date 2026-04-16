use crossterm::event::KeyCode;

use crate::db::Database;

/// A search result entry.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub sender: String,
    pub body: String,
    pub timestamp_ms: i64,
    pub conv_id: String,
    pub conv_name: String,
}

/// Action returned by `SearchState::handle_key` / `jump_to_result` for App to dispatch.
pub enum SearchAction {
    /// User selected a result — jump to this conversation + timestamp.
    Select {
        conv_id: String,
        timestamp_ms: i64,
        status: Option<String>,
    },
    /// Status message to display.
    Status(String),
    /// No action needed.
    None,
}

/// State for the search overlay.
#[derive(Default)]
pub struct SearchState {
    pub visible: bool,
    pub query: String,
    pub results: Vec<SearchResult>,
    pub index: usize,
}

impl SearchState {
    /// Open the search overlay with an initial query.
    pub fn open(&mut self, query: String, active_conversation: Option<&str>, db: &Database) {
        self.query = query;
        self.index = 0;
        self.run(active_conversation, db);
        self.visible = true;
    }

    /// Handle a key press while the search overlay is open.
    pub fn handle_key(
        &mut self,
        code: KeyCode,
        active_conversation: Option<&str>,
        db: &Database,
    ) -> SearchAction {
        match code {
            KeyCode::Char('j') | KeyCode::Down
                if !self.results.is_empty() && self.index < self.results.len() - 1 => {
                    self.index += 1;
                }
            KeyCode::Char('k') | KeyCode::Up => {
                self.index = self.index.saturating_sub(1);
            }
            KeyCode::Enter => {
                if let Some(result) = self.results.get(self.index) {
                    let conv_id = result.conv_id.clone();
                    let target_ts = result.timestamp_ms;
                    self.visible = false;
                    // Keep query for n/N navigation status display
                    return SearchAction::Select {
                        conv_id,
                        timestamp_ms: target_ts,
                        status: None,
                    };
                }
            }
            KeyCode::Esc => {
                self.visible = false;
                self.query.clear();
            }
            KeyCode::Backspace
                if !self.query.is_empty() => {
                    self.query.pop();
                    self.run(active_conversation, db);
                }
            KeyCode::Char(c) => {
                self.query.push(c);
                self.run(active_conversation, db);
            }
            _ => {}
        }
        SearchAction::None
    }

    /// Execute the current search query against the database.
    pub fn run(&mut self, active_conversation: Option<&str>, db: &Database) {
        if self.query.is_empty() {
            self.results.clear();
            self.index = 0;
            return;
        }
        let results = if let Some(conv_id) = active_conversation {
            db.search_messages(conv_id, &self.query, 50)
        } else {
            db.search_all_messages(&self.query, 50)
        };
        match results {
            Ok(rows) => {
                self.results = rows
                    .into_iter()
                    .map(
                        |(sender, body, timestamp_ms, conv_id, conv_name)| SearchResult {
                            sender,
                            body,
                            timestamp_ms,
                            conv_id,
                            conv_name,
                        },
                    )
                    .collect();
            }
            Err(e) => {
                crate::debug_log::logf(format_args!("search error: {e}"));
                self.results.clear();
            }
        }
        // Clamp index
        if self.results.is_empty() {
            self.index = 0;
        } else if self.index >= self.results.len() {
            self.index = self.results.len() - 1;
        }
    }

    /// Jump to the next/previous search result in the active conversation.
    /// `forward` = true means next (older), false means previous (newer).
    pub fn jump_to_result(
        &mut self,
        forward: bool,
        active_conversation: Option<&str>,
    ) -> SearchAction {
        let conv_id = match active_conversation {
            Some(id) => id,
            None => return SearchAction::None,
        };
        // Filter results to current conversation only
        let conv_results: Vec<usize> = self
            .results
            .iter()
            .enumerate()
            .filter(|(_, r)| r.conv_id == *conv_id)
            .map(|(i, _)| i)
            .collect();
        if conv_results.is_empty() {
            return SearchAction::Status("no matches in this conversation".to_string());
        }

        // Find the current position relative to conv_results
        let current_pos = conv_results.iter().position(|&i| i == self.index);
        let next_idx = match current_pos {
            Some(pos) => {
                if forward {
                    if pos + 1 < conv_results.len() {
                        conv_results[pos + 1]
                    } else {
                        conv_results[0] // wrap around
                    }
                } else if pos > 0 {
                    conv_results[pos - 1]
                } else {
                    conv_results[conv_results.len() - 1] // wrap around
                }
            }
            None => conv_results[0],
        };

        self.index = next_idx;
        if let Some(result) = self.results.get(next_idx) {
            let ts = result.timestamp_ms;
            let pos = conv_results
                .iter()
                .position(|&i| i == next_idx)
                .unwrap_or(0)
                + 1;
            let status = format!(
                "match {}/{} for \"{}\"",
                pos,
                conv_results.len(),
                self.query
            );
            return SearchAction::Select {
                conv_id: result.conv_id.clone(),
                timestamp_ms: ts,
                status: Some(status),
            };
        }
        SearchAction::None
    }
}
