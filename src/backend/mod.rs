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
//! Linking and connection lifecycle (`link_state`, `link`, `--check`) are not
//! part of the trait yet; U5 routes those bypass paths through the boundary.

pub mod demo;
#[cfg(test)]
pub mod mock;
#[cfg(feature = "signal-cli-backend")]
pub mod signal_cli;

use crate::app::{App, SendRequest};
use crate::config::Config;

pub use demo::DemoBackend;

/// The concrete backend the binary is built with, selected at compile time
/// by the mutually exclusive `signal-cli-backend` / `native-backend`
/// features (#640 U7, plan KTD-1).
#[cfg(feature = "signal-cli-backend")]
pub type ActiveBackend<'a> = signal_cli::SignalCliBackend<'a>;

// The native engine arrives with #642 (U9): pinned presage, store, and the
// LocalSet runtime shim. The feature exists ahead of it so the CI matrix,
// mutual-exclusion guard, and lockfile canary are in place before any
// presage code merges; until U9 lands, a native build stops here with a
// clear message instead of a wall of missing-type errors. U9 replaces this
// with `pub mod native;` and the corresponding ActiveBackend alias.
#[cfg(feature = "native-backend")]
compile_error!(
    "the `native-backend` engine is not implemented yet (it lands with #642); build with the default `signal-cli-backend` feature until then"
);

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
///   [`try_reconnect`](Backend::try_reconnect),
///   [`resync_after_reconnect`](Backend::resync_after_reconnect). U5 reshapes
///   this group when linking/connection lifecycle enters the boundary.
pub trait Backend {
    /// User-facing engine name for status copy (plan flow gap G7).
    ///
    /// Not yet threaded into the reconnect/status strings; U5 does that (the
    /// existing strings stay byte-identical until then).
    #[allow(dead_code)]
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
    /// gate; U5 replaces the supervisor with trait-level connection lifecycle.
    fn supports_reconnect(&self) -> bool;

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
}
