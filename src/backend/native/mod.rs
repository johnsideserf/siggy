//! Native in-process Signal engine (#642, plan U9-U12).
//!
//! U9 skeleton: the pinned presage dependency tree compiles, the per-account
//! store path resolves ([`store`]), and the `!Send` runtime shim exists
//! ([`runtime`]). The [`Backend`] impl is an honest stub: a native build
//! launches, says so in the status line, and does nothing else. Linking
//! lands with U10, receive with U11, send with U12.

pub mod runtime;
pub mod store;

use crate::app::{App, SendRequest};
use crate::config::Config;
use crate::signal::types::LinkState;

use super::Backend;

/// User-facing engine name (flow gap G7).
pub const ENGINE_NAME: &str = "native";

/// The native engine adapter. U9 holds no state; U10-U12 add the
/// [`runtime::EngineThread`] handle and the mpsc pair the trait methods
/// drive.
pub struct NativeBackend;

impl NativeBackend {
    pub fn new() -> Self {
        Self
    }

    /// Instance-free link-state probe, same signature as
    /// `SignalCliBackend::link_state` (plan U5, flow gap G1).
    ///
    /// U9 stub: always [`LinkState::Unlinked`]. Truthful for the skeleton
    /// (nothing can have linked through it), but it does NOT yet inspect the
    /// store; U10 replaces this with a real registration read so relinked /
    /// pre-existing stores answer correctly.
    pub async fn link_state(_config: &Config) -> LinkState {
        LinkState::Unlinked
    }
}

impl Default for NativeBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for NativeBackend {
    fn display_name(&self) -> &'static str {
        ENGINE_NAME
    }

    /// Group admin operations need presage APIs that are still upstream
    /// work (plan KTD-10); the native engine answers false until then.
    fn supports_group_admin(&self) -> bool {
        false
    }

    /// U12 routes the send vocabulary through the engine thread. Until
    /// then, surface the truth instead of silently dropping the request.
    async fn dispatch(&mut self, app: &mut App, _req: SendRequest) {
        app.status_message = "native engine: sending not implemented yet (#642)".to_string();
    }

    async fn startup(&mut self, app: &mut App) {
        app.connected = false;
        app.loading = false;
        app.status_message =
            "native engine skeleton (#642): linking/receive/send not implemented yet".to_string();
    }

    fn drain_events(&mut self, _app: &mut App) -> bool {
        false
    }

    /// No connection exists to lose; U11's stream supervisor flips this.
    fn supports_reconnect(&self) -> bool {
        false
    }

    async fn try_reconnect(&mut self, _config: &Config) -> bool {
        false
    }

    async fn resync_after_reconnect(&mut self) {}
}
