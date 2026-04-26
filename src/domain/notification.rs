//! Notification preferences and clipboard auto-clear timer.
//!
//! Holds the per-frame `pending_bell` flag plus user preferences for
//! direct/group terminal bells, OS desktop notifications, and the
//! `notification_preview` level. Also tracks the clipboard auto-clear
//! window (`clipboard_clear_seconds`) and the `clipboard_set_at`
//! timestamp used to scrub copied data after it expires.

/// State for notification preferences and clipboard auto-clear.
#[derive(Default)]
pub struct NotificationState {
    /// Bell pending for the current frame
    pub pending_bell: bool,
    /// Terminal bell for 1:1 messages in background conversations
    pub notify_direct: bool,
    /// Terminal bell for group messages in background conversations
    pub notify_group: bool,
    /// OS-level desktop notifications for incoming messages
    pub desktop_notifications: bool,
    /// Notification preview level: "full", "sender", or "minimal"
    pub notification_preview: String,
    /// Seconds before clipboard is auto-cleared after copying (0 = disabled)
    pub clipboard_clear_seconds: u64,
    /// Timestamp when clipboard was last set
    pub clipboard_set_at: Option<std::time::Instant>,
}

impl NotificationState {
    pub fn new() -> Self {
        Self {
            notify_direct: true,
            notify_group: true,
            notification_preview: "full".to_string(),
            clipboard_clear_seconds: 30,
            ..Default::default()
        }
    }
}
