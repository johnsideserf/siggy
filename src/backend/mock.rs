//! Test double for the [`Backend`] trait (plan U4 test scenarios).
//!
//! Records every dispatched [`SendRequest`] so tests can assert that each
//! variant family routes through the trait seam unchanged. All other trait
//! methods are inert, mirroring [`super::demo::DemoBackend`].

use crate::app::{App, SendRequest};
use crate::config::Config;

use super::Backend;

#[derive(Default)]
pub struct MockBackend {
    /// Every request routed through [`Backend::dispatch`], in order.
    pub dispatched: Vec<SendRequest>,
    /// Whether the reconnect supervisor applies (plan U5 supervisor tests).
    pub reconnectable: bool,
    /// What [`Backend::try_reconnect`] reports back to the supervisor.
    pub reconnect_ok: bool,
    /// How many times [`Backend::try_reconnect`] was called through the
    /// trait (plan U5: the supervisor must route every attempt here).
    pub reconnect_calls: u32,
    /// How many times [`Backend::resync_after_reconnect`] ran.
    pub resync_calls: u32,
}

impl Backend for MockBackend {
    fn display_name(&self) -> &'static str {
        "mock"
    }

    async fn dispatch(&mut self, _app: &mut App, req: SendRequest) {
        self.dispatched.push(req);
    }

    async fn startup(&mut self, _app: &mut App) {}

    fn drain_events(&mut self, _app: &mut App) -> bool {
        false
    }

    fn supports_reconnect(&self) -> bool {
        self.reconnectable
    }

    async fn try_reconnect(&mut self, _config: &Config) -> bool {
        self.reconnect_calls += 1;
        self.reconnect_ok
    }

    async fn resync_after_reconnect(&mut self) {
        self.resync_calls += 1;
    }
}
