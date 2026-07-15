//! Messaging backend boundary (plan KTD-1, #640).
//!
//! Houses the [`Backend`] trait that the main loop drives, plus one adapter
//! per engine: [`signal_cli::SignalCliBackend`] wraps the existing signal-cli
//! JSON-RPC bridge and [`demo::DemoBackend`] swallows sends for `--demo`
//! mode. The trait exists for contract documentation and test doubles; the
//! main loop is generic over it and monomorphizes, so there is no `dyn` and
//! no runtime dispatch cost (KTD-1). Engine-specific wire quirks (bare-string
//! vs array recipients, JSON-RPC correlation) stay inside the adapters and
//! `signal/`; nothing engine-shaped leaks past this module.
//!
//! Link-state probing (`--check`, startup registration gating, the wizard's
//! linking step) goes through each adapter's inherent `link_state()`
//! associated function rather than a trait method: the probe must run before
//! any engine instance or connection exists, so tying it to `&mut self` would
//! misrepresent how it actually works (plan U5, flow gap G1; the choice is
//! documented on [`Backend`]). The reconnect supervisor's retry policy is
//! adapter-supplied via [`ReconnectPolicy`] (flow gap G2), and user-facing
//! engine naming goes through [`Backend::display_name`] or, instance-free,
//! [`ACTIVE_ENGINE_NAME`] (flow gap G7).

pub mod demo;
#[cfg(test)]
pub mod mock;
#[cfg(feature = "native-backend")]
pub mod native;
#[cfg(feature = "signal-cli-backend")]
pub mod signal_cli;

use std::time::Duration;

use crate::app::{App, SendRequest};
use crate::config::Config;
use crate::signal::types::LinkState;

pub use demo::DemoBackend;

/// Instance-free link-state probe for the compiled-in engine (#642 U9).
/// Callers that run before any engine instance exists (`--check`, the
/// wizard's linking step) go through this instead of naming an adapter, so
/// they compile identically under either feature.
pub async fn probe_link_state(config: &Config) -> LinkState {
    #[cfg(feature = "signal-cli-backend")]
    {
        signal_cli::SignalCliBackend::link_state(config).await
    }
    #[cfg(feature = "native-backend")]
    {
        native::NativeBackend::link_state(config).await
    }
}

/// The concrete backend the binary is built with, selected at compile time
/// by the mutually exclusive `signal-cli-backend` / `native-backend`
/// features (#640 U7, plan KTD-1).
#[cfg(feature = "signal-cli-backend")]
pub type ActiveBackend<'a> = signal_cli::SignalCliBackend<'a>;

/// Native engine skeleton (#642 U9). The lifetime parameter is unused here
/// (the native adapter owns its engine thread rather than borrowing a
/// client) but kept so main-loop code writes `ActiveBackend<'_>` uniformly
/// across both engines.
#[cfg(feature = "native-backend")]
pub type ActiveBackend<'a> = native::NativeBackend;

/// User-facing name of the compiled-in engine, for surfaces that run before
/// any backend instance exists (`--check`'s backend line, spawn-failure
/// copy). Always matches `ActiveBackend`'s [`Backend::display_name`] (flow
/// gap G7). Cfg-selected alongside the alias above; U9 adds the native arm.
#[cfg(feature = "signal-cli-backend")]
pub const ACTIVE_ENGINE_NAME: &str = signal_cli::ENGINE_NAME;
#[cfg(feature = "native-backend")]
pub const ACTIVE_ENGINE_NAME: &str = native::ENGINE_NAME;

/// Whether the compiled-in engine drives an external binary the setup wizard
/// must locate (plan U5 wizard hook, #640). True for signal-cli; U10's
/// native backend flips this so the wizard skips its binary-detection step.
#[cfg(feature = "signal-cli-backend")]
pub const NEEDS_CLI_BINARY: bool = true;
/// The native engine is in-process: no external binary to locate (U10 makes
/// the wizard consume this).
#[cfg(feature = "native-backend")]
pub const NEEDS_CLI_BINARY: bool = false;

/// Retry policy for the main loop's reconnect supervisor (plan U5, flow gap
/// G2, #640). Adapters expose it via [`Backend::reconnect_policy`] so the
/// policy is an engine property, not a main-loop constant: U11 lets the
/// native adapter retry network-class errors indefinitely while signal-cli
/// keeps its 6-attempt respawn cap (#497).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectPolicy {
    /// How many attempts before giving up and leaving the disconnect error
    /// on screen.
    pub max_attempts: u32,
}

impl Default for ReconnectPolicy {
    /// signal-cli's policy (#497): 6 respawn attempts, exponential backoff.
    fn default() -> Self {
        Self { max_attempts: 6 }
    }
}

impl ReconnectPolicy {
    /// Exponential backoff before the next attempt, capped at 30s. `attempt`
    /// is 1-based (the delay applied *after* attempt N fails, before attempt
    /// N+1). The first attempt fires immediately on disconnect, so this is
    /// only consulted after a failure. Byte-identical to the old
    /// `reconnect_backoff` in `main.rs`.
    pub fn backoff(&self, attempt: u32) -> Duration {
        Duration::from_secs((1u64 << attempt.min(6)).min(30))
    }
}

/// A messaging engine the main loop can drive.
///
/// Shaped around siggy's needs (one `dispatch` entry point for the whole
/// [`SendRequest`] vocabulary), not around signal-cli's RPC granularity.
/// Methods are grouped:
///
/// - identity and capabilities: [`display_name`](Backend::display_name),
///   [`supports_group_admin`](Backend::supports_group_admin)
/// - send path: [`dispatch`](Backend::dispatch)
/// - main-loop plumbing carried over from the retired `MessagingBackend`
///   enum: [`startup`](Backend::startup), [`drain_events`](Backend::drain_events),
///   [`supports_reconnect`](Backend::supports_reconnect),
///   [`reconnect_policy`](Backend::reconnect_policy),
///   [`try_reconnect`](Backend::try_reconnect),
///   [`resync_after_reconnect`](Backend::resync_after_reconnect)
///
/// Deliberately *not* a trait method: link-state probing. Every "am I
/// linked?" call site (`--check`, startup gating in `run_main_flow`, the
/// wizard's linking step) runs before an engine instance or connection
/// exists, so each adapter instead exposes an inherent associated function
/// (`SignalCliBackend::link_state(config)`; U10 adds the native twin with
/// the same signature). This is the least invasive shape that truthfully
/// models today's probes (plan U5, KTD-3, flow gap G1).
pub trait Backend {
    /// User-facing engine name for status copy (plan flow gap G7). The
    /// reconnect supervisor's status strings render through this, keeping
    /// them byte-identical for signal-cli and truthful for other engines.
    fn display_name(&self) -> &'static str;

    /// Capability flag (plan KTD-10): whether group admin operations
    /// (create/rename/invite/kick) work on this engine. The native backend
    /// answers false until presage grows the API; UI gating lands with it.
    #[allow(dead_code)]
    fn supports_group_admin(&self) -> bool {
        true
    }

    /// Route one [`SendRequest`] to the engine. Owns the request's follow-up
    /// app-state effects (pending-send registration, status messages, local
    /// failure marking) exactly as the old `dispatch_send` in `main.rs` did.
    async fn dispatch(&mut self, app: &mut App, req: SendRequest);

    /// One-time startup work after the [`App`] is constructed and loaded from
    /// the DB: mark connected and kick off the initial contact/group/identity
    /// sync (or populate demo fixtures).
    async fn startup(&mut self, app: &mut App);

    /// Drain pending backend events into `app` without blocking. Returns true
    /// if any event was handled. Detects engine disconnect and records it on
    /// `app.connection_error`.
    fn drain_events(&mut self, app: &mut App) -> bool;

    /// Whether the main loop's reconnect supervisor applies to this backend.
    /// Preserves the old `matches!(backend, MessagingBackend::Signal(_))`
    /// gate; the supervisor's cap and backoff come from
    /// [`reconnect_policy`](Backend::reconnect_policy).
    fn supports_reconnect(&self) -> bool;

    /// Retry policy for [`try_reconnect`](Backend::try_reconnect) (plan U5,
    /// flow gap G2). Defaults to signal-cli's 6-attempt exponential-backoff
    /// policy (#497); only consulted when
    /// [`supports_reconnect`](Backend::supports_reconnect) is true.
    fn reconnect_policy(&self) -> ReconnectPolicy {
        ReconnectPolicy::default()
    }

    /// Attempt to re-establish the engine connection. Returns true on
    /// success. See #497 for the signal-cli respawn semantics.
    async fn try_reconnect(&mut self, config: &Config) -> bool;

    /// Best-effort re-run of the startup sync after a successful reconnect.
    async fn resync_after_reconnect(&mut self);
}

#[cfg(test)]
mod tests {
    use super::demo::DemoBackend;
    use super::mock::MockBackend;
    use super::*;
    use crate::db::Database;

    fn test_app() -> App {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        // Leak the tempdir so it lives as long as the test process, matching
        // the app_tests fixture. The OS reclaims temp dirs on exit.
        std::mem::forget(dir);
        let db = Database::open_in_memory().unwrap();
        App::new("+10000000000".to_string(), db, &config_path)
    }

    /// Route through a generic parameter so these tests exercise the trait
    /// seam the main loop uses, not MockBackend's inherent methods.
    async fn route<B: Backend>(backend: &mut B, app: &mut App, req: SendRequest) {
        backend.dispatch(app, req).await;
    }

    fn message_req() -> SendRequest {
        SendRequest::Message {
            recipient: "+15551234567".into(),
            body: "hello".into(),
            is_group: false,
            local_ts_ms: 1_700_000_000_000,
            mentions: vec![],
            text_styles: vec![],
            attachment: None,
            preview: None,
            quote_timestamp: None,
            quote_author: None,
            quote_body: None,
        }
    }

    // --- One routing test per SendRequest variant family (plan U4) ---

    #[tokio::test]
    async fn message_routes_to_dispatch() {
        let mut app = test_app();
        let mut mock = MockBackend::default();
        route(&mut mock, &mut app, message_req()).await;
        assert!(matches!(
            &mock.dispatched[..],
            [SendRequest::Message { recipient, body, .. }]
                if recipient == "+15551234567" && body == "hello"
        ));
    }

    #[tokio::test]
    async fn reaction_routes_to_dispatch() {
        let mut app = test_app();
        let mut mock = MockBackend::default();
        route(
            &mut mock,
            &mut app,
            SendRequest::Reaction {
                conv_id: "+15551234567".into(),
                emoji: "👍".into(),
                is_group: false,
                target_author: "+15559876543".into(),
                target_timestamp: 1,
                remove: false,
            },
        )
        .await;
        assert!(matches!(
            &mock.dispatched[..],
            [SendRequest::Reaction { emoji, remove: false, .. }] if emoji == "👍"
        ));
    }

    #[tokio::test]
    async fn edit_routes_to_dispatch() {
        let mut app = test_app();
        let mut mock = MockBackend::default();
        route(
            &mut mock,
            &mut app,
            SendRequest::Edit {
                recipient: "+15551234567".into(),
                body: "edited".into(),
                is_group: false,
                edit_timestamp: 2,
                local_ts_ms: 3,
                mentions: vec![],
                text_styles: vec![],
                quote_timestamp: None,
                quote_author: None,
                quote_body: None,
            },
        )
        .await;
        assert!(matches!(
            &mock.dispatched[..],
            [SendRequest::Edit { edit_timestamp: 2, body, .. }] if body == "edited"
        ));
    }

    #[tokio::test]
    async fn remote_delete_routes_to_dispatch() {
        let mut app = test_app();
        let mut mock = MockBackend::default();
        route(
            &mut mock,
            &mut app,
            SendRequest::RemoteDelete {
                recipient: "+15551234567".into(),
                is_group: false,
                target_timestamp: 4,
            },
        )
        .await;
        assert!(matches!(
            &mock.dispatched[..],
            [SendRequest::RemoteDelete {
                target_timestamp: 4,
                ..
            }]
        ));
    }

    #[tokio::test]
    async fn typing_routes_to_dispatch() {
        let mut app = test_app();
        let mut mock = MockBackend::default();
        route(
            &mut mock,
            &mut app,
            SendRequest::Typing {
                recipient: "+15551234567".into(),
                is_group: false,
                stop: true,
            },
        )
        .await;
        assert!(matches!(
            &mock.dispatched[..],
            [SendRequest::Typing { stop: true, .. }]
        ));
    }

    #[tokio::test]
    async fn read_receipt_routes_to_dispatch() {
        let mut app = test_app();
        let mut mock = MockBackend::default();
        route(
            &mut mock,
            &mut app,
            SendRequest::ReadReceipt {
                recipient: "+15551234567".into(),
                timestamps: vec![5, 6],
            },
        )
        .await;
        assert!(matches!(
            &mock.dispatched[..],
            [SendRequest::ReadReceipt { timestamps, .. }] if timestamps == &[5, 6]
        ));
    }

    #[tokio::test]
    async fn expiration_routes_to_dispatch() {
        let mut app = test_app();
        let mut mock = MockBackend::default();
        route(
            &mut mock,
            &mut app,
            SendRequest::UpdateExpiration {
                conv_id: "+15551234567".into(),
                is_group: false,
                seconds: 3600,
            },
        )
        .await;
        assert!(matches!(
            &mock.dispatched[..],
            [SendRequest::UpdateExpiration { seconds: 3600, .. }]
        ));
    }

    #[tokio::test]
    async fn group_admin_family_routes_to_dispatch() {
        let mut app = test_app();
        let mut mock = MockBackend::default();
        route(
            &mut mock,
            &mut app,
            SendRequest::CreateGroup { name: "g".into() },
        )
        .await;
        route(
            &mut mock,
            &mut app,
            SendRequest::AddGroupMembers {
                group_id: "gid".into(),
                members: vec!["+15551234567".into()],
            },
        )
        .await;
        route(
            &mut mock,
            &mut app,
            SendRequest::RemoveGroupMembers {
                group_id: "gid".into(),
                members: vec!["+15551234567".into()],
            },
        )
        .await;
        route(
            &mut mock,
            &mut app,
            SendRequest::RenameGroup {
                group_id: "gid".into(),
                name: "g2".into(),
            },
        )
        .await;
        route(
            &mut mock,
            &mut app,
            SendRequest::LeaveGroup {
                group_id: "gid".into(),
            },
        )
        .await;
        assert!(matches!(
            &mock.dispatched[..],
            [
                SendRequest::CreateGroup { .. },
                SendRequest::AddGroupMembers { .. },
                SendRequest::RemoveGroupMembers { .. },
                SendRequest::RenameGroup { .. },
                SendRequest::LeaveGroup { .. },
            ]
        ));
    }

    #[tokio::test]
    async fn message_request_response_routes_to_dispatch() {
        let mut app = test_app();
        let mut mock = MockBackend::default();
        route(
            &mut mock,
            &mut app,
            SendRequest::MessageRequestResponse {
                recipient: "+15551234567".into(),
                is_group: false,
                response_type: "accept".into(),
            },
        )
        .await;
        assert!(matches!(
            &mock.dispatched[..],
            [SendRequest::MessageRequestResponse { response_type, .. }]
                if response_type == "accept"
        ));
    }

    #[tokio::test]
    async fn block_family_routes_to_dispatch() {
        let mut app = test_app();
        let mut mock = MockBackend::default();
        route(
            &mut mock,
            &mut app,
            SendRequest::Block {
                recipient: "+15551234567".into(),
                is_group: false,
            },
        )
        .await;
        route(
            &mut mock,
            &mut app,
            SendRequest::Unblock {
                recipient: "+15551234567".into(),
                is_group: false,
            },
        )
        .await;
        assert!(matches!(
            &mock.dispatched[..],
            [SendRequest::Block { .. }, SendRequest::Unblock { .. }]
        ));
    }

    #[tokio::test]
    async fn pin_family_routes_to_dispatch() {
        let mut app = test_app();
        let mut mock = MockBackend::default();
        route(
            &mut mock,
            &mut app,
            SendRequest::Pin {
                recipient: "+15551234567".into(),
                is_group: false,
                target_author: "+15559876543".into(),
                target_timestamp: 7,
                pin_duration: 0,
            },
        )
        .await;
        route(
            &mut mock,
            &mut app,
            SendRequest::Unpin {
                recipient: "+15551234567".into(),
                is_group: false,
                target_author: "+15559876543".into(),
                target_timestamp: 7,
            },
        )
        .await;
        assert!(matches!(
            &mock.dispatched[..],
            [SendRequest::Pin { .. }, SendRequest::Unpin { .. }]
        ));
    }

    #[tokio::test]
    async fn poll_family_routes_to_dispatch() {
        let mut app = test_app();
        let mut mock = MockBackend::default();
        route(
            &mut mock,
            &mut app,
            SendRequest::PollCreate {
                recipient: "+15551234567".into(),
                is_group: false,
                question: "q?".into(),
                options: vec!["a".into(), "b".into()],
                allow_multiple: false,
                local_ts_ms: 8,
            },
        )
        .await;
        route(
            &mut mock,
            &mut app,
            SendRequest::PollVote {
                recipient: "+15551234567".into(),
                is_group: false,
                poll_author: "+15559876543".into(),
                poll_timestamp: 9,
                option_indexes: vec![0],
                vote_count: 1,
            },
        )
        .await;
        route(
            &mut mock,
            &mut app,
            SendRequest::PollTerminate {
                recipient: "+15551234567".into(),
                is_group: false,
                poll_timestamp: 9,
            },
        )
        .await;
        assert!(matches!(
            &mock.dispatched[..],
            [
                SendRequest::PollCreate { .. },
                SendRequest::PollVote { .. },
                SendRequest::PollTerminate { .. },
            ]
        ));
    }

    #[tokio::test]
    async fn identity_family_routes_to_dispatch() {
        let mut app = test_app();
        let mut mock = MockBackend::default();
        route(&mut mock, &mut app, SendRequest::ListIdentities).await;
        route(
            &mut mock,
            &mut app,
            SendRequest::TrustIdentity {
                recipient: "+15551234567".into(),
                safety_number: "0000".into(),
            },
        )
        .await;
        assert!(matches!(
            &mock.dispatched[..],
            [
                SendRequest::ListIdentities,
                SendRequest::TrustIdentity { .. },
            ]
        ));
    }

    #[tokio::test]
    async fn resolve_username_routes_to_dispatch() {
        let mut app = test_app();
        let mut mock = MockBackend::default();
        route(
            &mut mock,
            &mut app,
            SendRequest::ResolveUsername {
                username: "alice.42".into(),
            },
        )
        .await;
        assert!(matches!(
            &mock.dispatched[..],
            [SendRequest::ResolveUsername { username }] if username == "alice.42"
        ));
    }

    #[tokio::test]
    async fn update_profile_routes_to_dispatch() {
        let mut app = test_app();
        let mut mock = MockBackend::default();
        route(
            &mut mock,
            &mut app,
            SendRequest::UpdateProfile {
                given_name: "A".into(),
                family_name: "B".into(),
                about: "".into(),
                about_emoji: "".into(),
            },
        )
        .await;
        assert!(matches!(
            &mock.dispatched[..],
            [SendRequest::UpdateProfile { given_name, .. }] if given_name == "A"
        ));
    }

    // --- Demo adapter preserves the retired enum's no-op arms ---

    #[tokio::test]
    async fn demo_backend_swallows_sends_and_yields_no_events() {
        let mut app = test_app();
        let mut demo = DemoBackend;
        let status_before = app.status_message.clone();
        route(&mut demo, &mut app, message_req()).await;
        // Nothing dispatched anywhere: no pending send, no status change.
        assert!(app.pending.sends.is_empty());
        assert_eq!(app.status_message, status_before);
        assert!(!demo.drain_events(&mut app));
        assert!(!demo.supports_reconnect());
        assert!(!demo.try_reconnect(&Config::default()).await);
    }

    #[tokio::test]
    async fn demo_startup_matches_legacy_run_app_branch() {
        let mut app = test_app();
        let mut demo = DemoBackend;
        demo.startup(&mut app).await;
        assert!(app.is_demo);
        assert!(app.connected);
        assert!(!app.loading);
        assert_eq!(app.status_message, "connected | demo mode");
        assert!(
            !app.store.conversations.is_empty(),
            "demo fixtures populated"
        );
    }

    // --- Identity and capabilities ---

    #[test]
    fn display_names_identify_engines() {
        assert_eq!(DemoBackend.display_name(), "demo");
        assert_eq!(MockBackend::default().display_name(), "mock");
    }

    #[test]
    fn group_admin_capability_defaults_to_supported() {
        assert!(DemoBackend.supports_group_admin());
        assert!(MockBackend::default().supports_group_admin());
    }

    // --- Reconnect policy (plan U5, flow gap G2) ---

    /// Moved from main.rs's `reconnect_backoff_is_exponential_and_capped`
    /// when the backoff became adapter policy (#640 U5). Same assertions:
    /// signal-cli's schedule must stay byte-identical (#497).
    #[test]
    fn reconnect_policy_backoff_is_exponential_and_capped() {
        let policy = ReconnectPolicy::default();
        assert_eq!(policy.backoff(1), Duration::from_secs(2));
        assert_eq!(policy.backoff(2), Duration::from_secs(4));
        assert_eq!(policy.backoff(3), Duration::from_secs(8));
        assert_eq!(policy.backoff(4), Duration::from_secs(16));
        // Capped at 30s, and never overflows the shift for large attempts.
        assert_eq!(policy.backoff(5), Duration::from_secs(30));
        assert_eq!(policy.backoff(6), Duration::from_secs(30));
        assert_eq!(policy.backoff(100), Duration::from_secs(30));
    }

    #[test]
    fn reconnect_policy_defaults_to_signal_cli_attempt_cap() {
        // The old MAX_RECONNECT_ATTEMPTS constant (#497), now trait-supplied.
        assert_eq!(ReconnectPolicy::default().max_attempts, 6);
        assert_eq!(
            MockBackend::default().reconnect_policy(),
            ReconnectPolicy::default()
        );
    }
}
