//! Conversation scroll state and message focus cursor.
//!
//! Tracks the current viewport offset (`offset`, where 0 means scrolled
//! to the bottom), per-conversation saved positions (`positions`), the
//! message under the focus cursor when scrolled up (`focused_index` /
//! `focused_time`), the jump-back stack used by quote navigation, and a
//! render-side flag that signals when the active conversation is
//! scrolled to its oldest message.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

/// State for the messages-pane scroll viewport and message focus cursor.
#[derive(Default)]
pub struct ScrollState {
    /// Scroll offset for messages (`0` = bottom of conversation).
    pub offset: usize,
    /// Saved scroll positions per conversation: `(offset, focused_index)`.
    pub positions: HashMap<String, (usize, Option<usize>)>,
    /// Set by the renderer when the active conversation is scrolled to the top
    /// and there are more messages above (the "load more" hint).
    pub at_top: bool,
    /// Timestamp of the message at the scroll cursor (set during draw,
    /// cleared when `offset == 0`).
    pub focused_time: Option<DateTime<Utc>>,
    /// Index of the focused message in the active conversation (set during draw).
    pub focused_index: Option<usize>,
    /// Jump-back stack: saved `(offset, focused_index)` pairs from quote-jump
    /// navigation. Esc pops to restore.
    pub jump_stack: Vec<(usize, Option<usize>)>,
}
