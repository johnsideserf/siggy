//! Message search overlay: query state, results, and navigation.
//!
//! `query` is the live filter text, `results` is the rolling 50-row
//! match list pulled from SQLite (per-conversation when an active
//! conversation is set, otherwise across all conversations), and
//! `index` is the cursor over results. `handle_key` returns a
//! `SearchAction` for the App to dispatch (jump, status, cancel).
//! `jump_to_result` powers `n`/`N` traversal within the active
//! conversation with wrap-around.

use std::time::{Duration, Instant};

use crossterm::event::KeyCode;

use crate::db::Database;

/// Idle time after the last keystroke before a debounced search runs, so a fast
/// typist triggers one scan instead of one per character (#491).
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);
/// Minimum query length before scanning. A leading-wildcard LIKE scans the
/// whole messages table, and a one-character query matches almost everything,
/// so it is not worth running (#491).
const MIN_SEARCH_LEN: usize = 2;

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
    /// User cancelled the overlay (Esc) - caller should close it.
    Cancel,
    /// No action needed.
    None,
}

/// State for the search overlay.
#[derive(Default)]
pub struct SearchState {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub index: usize,
    /// A query edit is pending a (debounced) run.
    dirty: bool,
    /// When the query was last edited, for the debounce window.
    last_edit: Option<Instant>,
}

impl SearchState {
    /// Configure the search overlay with an initial query and run the query.
    /// Caller must also call `App::open_overlay` to make the overlay visible.
    pub fn open(&mut self, query: String, active_conversation: Option<&str>, db: &Database) {
        self.query = query;
        self.index = 0;
        self.dirty = false;
        self.run(active_conversation, db);
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
                if !self.results.is_empty() && self.index < self.results.len() - 1 =>
            {
                self.index += 1;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.index = self.index.saturating_sub(1);
            }
            KeyCode::Enter => {
                // Flush any pending debounced query so Enter selects from the
                // current query's results, not a stale set.
                if self.dirty {
                    self.dirty = false;
                    self.run(active_conversation, db);
                }
                if let Some(result) = self.results.get(self.index) {
                    let conv_id = result.conv_id.clone();
                    let target_ts = result.timestamp_ms;
                    // Keep query for n/N navigation status display.
                    // Caller closes the overlay on Select.
                    return SearchAction::Select {
                        conv_id,
                        timestamp_ms: target_ts,
                        status: None,
                    };
                }
            }
            KeyCode::Esc => {
                self.query.clear();
                return SearchAction::Cancel;
            }
            KeyCode::Backspace if !self.query.is_empty() => {
                self.query.pop();
                self.mark_dirty();
            }
            KeyCode::Char(c) => {
                self.query.push(c);
                self.mark_dirty();
            }
            _ => {}
        }
        SearchAction::None
    }

    /// Record a query edit as pending; the actual scan runs later via
    /// `run_if_due` once typing pauses (#491).
    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.last_edit = Some(Instant::now());
    }

    /// Run a pending debounced query if typing has paused. Called each main
    /// loop tick while the search overlay is open. Returns true if it ran.
    pub fn run_if_due(&mut self, active_conversation: Option<&str>, db: &Database) -> bool {
        if !self.dirty {
            return false;
        }
        if self
            .last_edit
            .is_some_and(|t| t.elapsed() < SEARCH_DEBOUNCE)
        {
            return false;
        }
        self.dirty = false;
        self.run(active_conversation, db);
        true
    }

    /// Execute the current search query against the database.
    pub fn run(&mut self, active_conversation: Option<&str>, db: &Database) {
        if self.query.chars().count() < MIN_SEARCH_LEN {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn db_with(conv: &str, bodies: &[&str]) -> Database {
        let db = Database::open_in_memory().unwrap();
        db.upsert_conversation(conv, "Alice", false).unwrap();
        for (i, body) in bodies.iter().enumerate() {
            db.insert_message(
                conv,
                "Alice",
                &format!("2025-01-01T00:{i:02}:00Z"),
                body,
                false,
                None,
                1000 + i as i64,
            )
            .unwrap();
        }
        db
    }

    #[test]
    fn run_skips_queries_below_min_length() {
        let db = db_with("+1", &["hello world"]);
        let mut s = SearchState {
            query: "h".to_string(),
            ..Default::default()
        };
        s.run(Some("+1"), &db);
        assert!(s.results.is_empty(), "1-char query should not scan");
        s.query = "he".to_string();
        s.run(Some("+1"), &db);
        assert_eq!(s.results.len(), 1);
    }

    #[test]
    fn typing_defers_search_until_due() {
        let db = db_with("+1", &["hello world"]);
        let mut s = SearchState::default();
        for c in ['h', 'e', 'l', 'l', 'o'] {
            s.handle_key(KeyCode::Char(c), Some("+1"), &db);
        }
        assert!(s.results.is_empty(), "search must not run per keystroke");
        assert!(s.dirty);
        // Within the debounce window: not due.
        assert!(!s.run_if_due(Some("+1"), &db));
        assert!(s.results.is_empty());
        // Simulate the debounce window elapsing.
        s.last_edit = None;
        assert!(s.run_if_due(Some("+1"), &db));
        assert_eq!(s.results.len(), 1);
        assert!(!s.dirty);
    }

    #[test]
    fn enter_flushes_pending_query() {
        let db = db_with("+1", &["hello world"]);
        let mut s = SearchState::default();
        for c in ['w', 'o', 'r', 'l', 'd'] {
            s.handle_key(KeyCode::Char(c), Some("+1"), &db);
        }
        assert!(s.results.is_empty());
        let action = s.handle_key(KeyCode::Enter, Some("+1"), &db);
        assert_eq!(s.results.len(), 1, "Enter flushes the pending search");
        assert!(matches!(action, SearchAction::Select { .. }));
    }
}
