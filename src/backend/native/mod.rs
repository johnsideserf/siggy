//! Native in-process Signal engine (#642, plan U9-U12).
//!
//! U9 landed the skeleton (pinned presage tree, per-account store paths in
//! [`store`], the `!Send` runtime shim in [`runtime`]); U10 adds device
//! linking and the real store-backed link-state probe ([`linking`]). The
//! [`Backend`] impl remains an honest stub for receive/send: those land
//! with U11/U12.

pub mod linking;
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
    /// `SignalCliBackend::link_state` (plan U5, flow gap G1). Reads the
    /// account's store registration row (#642 U10); note this reflects
    /// LOCAL state - a device unlinked from the phone still reads Linked
    /// until the engine's next connection attempt fails (same caveat as
    /// signal-cli's probe).
    pub async fn link_state(config: &Config) -> LinkState {
        linking::link_state(config).await
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
