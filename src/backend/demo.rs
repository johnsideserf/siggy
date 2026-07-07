//! Demo-mode adapter (plan KTD-1, #640).
//!
//! `--demo` runs the full UI against fixture data with no signal-cli
//! involved: every outgoing [`SendRequest`] is swallowed, no events are ever
//! produced, and there is no connection to lose or reconnect. This replaces
//! the no-op arms of the retired `MessagingBackend::Demo` enum variant in
//! `main.rs`, byte for byte.

use crate::app::{App, SendRequest};
use crate::config::Config;

use super::Backend;

pub struct DemoBackend;

impl Backend for DemoBackend {
    fn display_name(&self) -> &'static str {
        "demo"
    }

    /// Demo mode swallows sends entirely; the composer's optimistic local
    /// echo (added by the input handlers before dispatch) is all the user
    /// sees, exactly as with the old enum's no-op arm.
    async fn dispatch(&mut self, _app: &mut App, _req: SendRequest) {}

    async fn startup(&mut self, app: &mut App) {
        app.is_demo = true;
        app.connected = true;
        app.loading = false;
        app.status_message = "connected | demo mode".to_string();
        app.populate_demo_data(chrono::Utc::now().date_naive());
    }

    fn drain_events(&mut self, _app: &mut App) -> bool {
        false
    }

    fn supports_reconnect(&self) -> bool {
        false
    }

    async fn try_reconnect(&mut self, _config: &Config) -> bool {
        false
    }

    async fn resync_after_reconnect(&mut self) {}
}
