use std::collections::HashMap;
use std::time::{Duration, Instant};

/// State for typing indicators (both inbound and outbound).
#[derive(Default)]
pub struct TypingState {
    /// Per-conversation typing indicators: conv_id → { sender_phone → timestamp }.
    /// Populated: TypingIndicator events (is_typing=true inserts, is_typing=false removes).
    /// Invalidation: entries expire after 5 seconds via `cleanup()` called each tick.
    pub indicators: HashMap<String, HashMap<String, Instant>>,
    /// Whether we've sent a typing-started indicator for the current input.
    pub sent: bool,
    /// When the last keypress happened (for typing timeout).
    pub last_keypress: Option<Instant>,
}

impl TypingState {
    /// Remove typing indicators older than 5 seconds. Returns true if any were removed.
    pub fn cleanup(&mut self) -> bool {
        let now = Instant::now();
        let mut changed = false;
        for senders in self.indicators.values_mut() {
            let before = senders.len();
            senders.retain(|_, ts| now.duration_since(*ts).as_secs() < 5);
            if senders.len() != before {
                changed = true;
            }
        }
        // Remove conversations with no remaining typers
        self.indicators.retain(|_, senders| !senders.is_empty());
        changed
    }

    /// Check if the outgoing typing indicator has timed out (5s since last keypress).
    /// Resets state and returns true if a stop request should be sent.
    pub fn check_timeout(&mut self) -> bool {
        if !self.sent {
            return false;
        }
        let elapsed = self
            .last_keypress
            .map(|t| t.elapsed() > Duration::from_secs(5))
            .unwrap_or(false);
        if elapsed {
            self.sent = false;
            self.last_keypress = None;
            return true;
        }
        false
    }

    /// Reset outgoing typing state. Returns true if we were actively typing
    /// (caller should queue a stop request).
    pub fn reset(&mut self) -> bool {
        let was_typing = self.sent;
        self.sent = false;
        self.last_keypress = None;
        was_typing
    }
}
