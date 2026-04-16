use chrono::{DateTime, Utc};

/// Mute status for a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuteState {
    Permanent,
    Until(DateTime<Utc>),
}

impl MuteState {
    /// True for permanent mutes, or timed mutes whose expiry is still in the future.
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        match self {
            Self::Permanent => true,
            Self::Until(t) => *t > now,
        }
    }

    /// Sidebar suffix for the mute badge. `None` for timed mutes past their expiry
    /// (the periodic sweep will drop them on the next tick).
    pub fn sidebar_indicator(&self, now: DateTime<Utc>) -> Option<String> {
        match self {
            Self::Permanent => Some(" ~".to_string()),
            Self::Until(t) => {
                let remaining = t.signed_duration_since(now).num_seconds();
                (remaining > 0)
                    .then(|| format!(" {}", crate::input::format_mute_remaining(remaining)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(y: i32, mo: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, 0, 0, 0).unwrap()
    }

    #[test]
    fn permanent_is_always_active() {
        assert!(MuteState::Permanent.is_active(t(2099, 1, 1)));
    }

    #[test]
    fn timed_active_while_future() {
        let expiry = t(2030, 1, 1);
        let state = MuteState::Until(expiry);
        assert!(state.is_active(t(2029, 12, 31)));
        assert!(!state.is_active(t(2030, 1, 1))); // strictly greater
        assert!(!state.is_active(t(2030, 1, 2)));
    }

    #[test]
    fn sidebar_indicator_permanent() {
        assert_eq!(
            MuteState::Permanent.sidebar_indicator(t(2030, 1, 1)),
            Some(" ~".to_string())
        );
    }

    #[test]
    fn sidebar_indicator_timed() {
        let expiry = t(2030, 1, 1) + chrono::Duration::hours(2);
        let state = MuteState::Until(expiry);
        assert_eq!(
            state.sidebar_indicator(t(2030, 1, 1)),
            Some(" ~2h".to_string())
        );
    }

    #[test]
    fn sidebar_indicator_expired_is_none() {
        let state = MuteState::Until(t(2020, 1, 1));
        assert_eq!(state.sidebar_indicator(t(2030, 1, 1)), None);
    }
}
