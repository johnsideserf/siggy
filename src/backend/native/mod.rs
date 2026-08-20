//! Native in-process Signal engine (#642, plan U9-U12).
//!
//! U9 landed the skeleton (pinned presage tree, per-account store paths in
//! [`store`], the `!Send` runtime shim in [`runtime`]); U10 added device
//! linking and the store-backed link-state probe ([`linking`]); U11 adds
//! the receive path: the pure mapping layer ([`receive`]), the durable
//! journal ([`journal`], KTD-2), and the stream supervisor
//! ([`supervisor`]). Sending remains an honest stub until U12.

pub mod journal;
pub mod linking;
pub mod receive;
pub mod runtime;
pub mod send;
pub mod store;
pub mod supervisor;

use crate::app::{App, SendRequest};
use crate::config::Config;
use crate::debug_log;
use crate::handlers;
use crate::signal::types::LinkState;

use super::Backend;
use supervisor::{EngineStatus, ReceiveEngine};

/// User-facing engine name (flow gap G7).
pub const ENGINE_NAME: &str = "native";

/// The native engine adapter: owns the receive supervisor's engine thread
/// once [`Backend::startup`] spawns it; sends route over the same thread's
/// command channel (U12).
pub struct NativeBackend {
    engine: Option<ReceiveEngine>,
    /// Last wire timestamp handed out (KTD-4): tokens must be strictly
    /// increasing and unique even for two sends inside one millisecond,
    /// both for `pending.sends` correlation and for Signal's own
    /// (sender, timestamp) message identity.
    last_wire_ts: i64,
}

impl NativeBackend {
    pub fn new() -> Self {
        Self {
            engine: None,
            last_wire_ts: 0,
        }
    }

    /// KTD-4 wire timestamp: now, bumped past the previous send when two
    /// land in the same millisecond. The caller-chosen value IS the wire
    /// timestamp natively, so the #480 local→server rewrite no-ops.
    fn next_wire_ts(&mut self) -> i64 {
        let now = chrono::Utc::now().timestamp_millis();
        let ts = now.max(self.last_wire_ts + 1);
        self.last_wire_ts = ts;
        ts
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

/// KTD-2 crash recovery: replay journal rows the previous process
/// persisted but never confirmed, through the same pipeline live events
/// use. The v16 entry_seq dedup index makes rows whose effects already
/// committed silent, so replaying after any crash point is safe.
fn replay_journal(app: &mut App) {
    let rows = match app.db.journal_pending() {
        Ok(rows) => rows,
        Err(e) => {
            debug_log::logf(format_args!("journal replay: read failed: {e}"));
            return;
        }
    };
    if rows.is_empty() {
        return;
    }
    debug_log::logf(format_args!(
        "journal replay: {} event(s) from previous session",
        rows.len()
    ));
    for (id, payload) in rows {
        match serde_json::from_str::<journal::JournalEvent>(&payload) {
            Ok(event) => app.handle_signal_event(event.into_signal()),
            // A row that survived a crash AND a version upgrade whose
            // schema drifted: log and drop (see journal.rs module docs for
            // why this residual window is accepted).
            Err(e) => debug_log::logf(format_args!(
                "journal replay: row {id} unparseable, dropping: {e}"
            )),
        }
        if let Err(e) = app.db.journal_delete(id) {
            debug_log::logf(format_args!("journal replay: delete {id} failed: {e}"));
        }
    }
}

/// Reflect the supervisor's latest connection status into app state.
fn apply_status(app: &mut App, status: &EngineStatus) {
    match status {
        EngineStatus::Connecting => {
            app.startup_status = "Connecting to Signal...".to_string();
        }
        EngineStatus::Connected => {
            app.set_connected();
            app.connection_error = None;
        }
        EngineStatus::Reconnecting { attempt } => {
            app.connected = false;
            app.status_message = format!("native engine: reconnecting (attempt {attempt})...");
        }
        EngineStatus::Terminal(msg) => {
            app.connected = false;
            app.connection_error = Some(msg.clone());
        }
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

    /// U12: 1:1 messages and username resolution route over the engine
    /// thread's command channel; U13: groups now route as well. Everything else
    /// states its unit honestly instead of silently dropping the request (KTD-10 spirit):
    /// attachments U14, rich bodies U15, reactions/edits/deletes U15,
    /// receipt/typing/profile tail U16+.
    async fn dispatch(&mut self, app: &mut App, req: SendRequest) {
        match req {
            SendRequest::Message {
                recipient,
                body,
                is_group,
                local_ts_ms,
                attachment,
                ..
                // mentions/text_styles/quote/preview deferred to U15: the
                // body still carries the display text, so the message
                // sends as plain text until rich bodies land.
            } => {
                if attachment.is_some() {
                    app.status_message =
                        "native engine: attachments not implemented yet (#642 U14)".to_string();
                    handlers::signal::mark_send_failed(app, &recipient, local_ts_ms);
                    return;
                }
                if self.engine.is_none() {
                    app.status_message = "native engine: not connected".to_string();
                    handlers::signal::mark_send_failed(app, &recipient, local_ts_ms);
                    return;
                }
                let wire_ts = self.next_wire_ts();
                let engine = self.engine.as_ref().expect("checked above");
                let token = crate::signal::types::SendToken::new(wire_ts.to_string());
                debug_log::logf(format_args!(
                    "native send: to={} group={is_group} wire_ts={wire_ts} local_ts={local_ts_ms}",
                    debug_log::mask_phone(&recipient)
                ));
                app.pending
                    .sends
                    .insert(token.clone(), (recipient.clone(), local_ts_ms));
                let command = if is_group {
                    send::SendCommand::GroupMessage {
                        token: token.clone(),
                        group_id: recipient.clone(),
                        body,
                        timestamp_ms: wire_ts as u64,
                    }
                } else {
                    send::SendCommand::Message {
                        token: token.clone(),
                        recipient: recipient.clone(),
                        body,
                        timestamp_ms: wire_ts as u64,
                    }
                };
                if engine.commands.send(command).is_err() {
                    // Engine thread gone: no SendFailed event will ever
                    // arrive, mark it here (#486 lesson).
                    app.pending.sends.remove(&token);
                    app.status_message = "native engine: send failed (engine stopped)".to_string();
                    handlers::signal::mark_send_failed(app, &recipient, local_ts_ms);
                }
            }
            SendRequest::ResolveUsername { username } => match &self.engine {
                Some(engine) => {
                    if engine
                        .commands
                        .send(send::SendCommand::ResolveUsername { username })
                        .is_err()
                    {
                        app.pending.username_resolve = None;
                        app.status_message =
                            "Username lookup failed: native engine stopped".to_string();
                    }
                }
                None => {
                    app.pending.username_resolve = None;
                    app.status_message = "Username lookup failed: not connected".to_string();
                }
            },
            other => {
                app.status_message = format!(
                    "native engine: {} not implemented yet (#642)",
                    other.kind_name()
                );
            }
        }
    }

    async fn startup(&mut self, app: &mut App) {
        // Replay before the supervisor spawns so recovered rows cannot
        // interleave with fresh appends.
        replay_journal(app);

        app.sweep_expired_messages();

        // G5: loading ends with the store load - the DB read (before
        // startup) plus the replay above. There is no RPC round-trip to
        // wait for; the notification-suppressing sync window still runs
        // until QueueEmpty maps to SyncComplete (KTD-5).
        app.loading = false;

        if !store::account_data_exists(&app.account) {
            // Unreachable in the normal flow (the linking gate in main.rs
            // runs first); honest copy instead of a phantom engine if a
            // store vanishes between the gate and here.
            app.connected = false;
            app.connection_error =
                Some("native engine: no linked account store; restart siggy to link".to_string());
            return;
        }

        let journal_db = app.db.path();
        if journal_db.is_none() {
            // In-memory DBs exist only in tests; production siggy.db is
            // always file-backed.
            debug_log::logf(format_args!(
                "native startup: in-memory DB, journaling disabled"
            ));
        }
        match supervisor::spawn(store::store_file(&app.account), journal_db) {
            Ok(engine) => {
                self.engine = Some(engine);
                app.startup_status = "Connecting to Signal...".to_string();
            }
            Err(e) => {
                app.connected = false;
                app.connection_error = Some(format!("native engine failed to start: {e}"));
            }
        }
    }

    fn drain_events(&mut self, app: &mut App) -> bool {
        let Some(engine) = self.engine.as_mut() else {
            return false;
        };
        let mut changed = false;

        // Newest connection status wins; the watch channel coalesces.
        // has_changed errors once the supervisor exits - the Terminal
        // status it sent beforehand was already consumed, and the closed
        // event channel below covers unexpected deaths.
        if engine.status.has_changed().unwrap_or(false) {
            let status = engine.status.borrow_and_update().clone();
            apply_status(app, &status);
            changed = true;
        }

        // Deliberately NO begin_batch here (plan U6 topology): the engine
        // thread commits a journal append between every stream pull, and a
        // long-held main-thread batch transaction would stall those appends
        // exactly when traffic is heaviest. Native drains autocommit per
        // event; the batching optimization (#489) stays signal-cli-internal.
        loop {
            match engine.events.try_recv() {
                Ok(engine_event) => {
                    let journal_id = engine_event.journal_id;
                    app.handle_signal_event(engine_event.event);
                    if let Some(id) = journal_id {
                        // Effects are committed (autocommit above); the
                        // journal row has served its purpose. A crash
                        // between the two lines replays one event, which
                        // dedup silences.
                        if let Err(e) = app.db.journal_delete(id) {
                            debug_log::logf(format_args!("journal delete {id} failed: {e}"));
                        }
                    }
                    changed = true;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    // Supervisor gone without a Terminal status (panic):
                    // report honestly. Normal terminations already set
                    // connection_error via apply_status.
                    if app.connection_error.is_none() {
                        app.connected = false;
                        app.connection_error =
                            Some("native engine stopped unexpectedly".to_string());
                    }
                    break;
                }
                Err(_) => break,
            }
        }
        changed
    }

    /// The supervisor self-heals on its own thread (indefinite capped
    /// backoff for network-class errors, plan Q4); the main loop's
    /// reconnect machinery belongs to signal-cli's child-process model.
    fn supports_reconnect(&self) -> bool {
        false
    }

    async fn try_reconnect(&mut self, _config: &Config) -> bool {
        false
    }

    async fn resync_after_reconnect(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::journal::{JournalEvent, JournalWriter};
    use super::supervisor::{EngineEvent, EngineStatus, ReceiveEngine};
    use super::*;
    use crate::db::Database;
    use crate::signal::types::{SignalEvent, SignalMessage};
    use tokio::sync::{mpsc, watch};

    fn file_backed_app(dir: &std::path::Path) -> App {
        let db = Database::open(&dir.join("siggy-test.db")).unwrap();
        App::new("+10000000000".to_string(), db, &dir.join("config.toml"))
    }

    fn incoming_message() -> SignalMessage {
        SignalMessage {
            source: "+15550001111".into(),
            source_name: Some("Ada".into()),
            source_uuid: Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into()),
            timestamp: chrono::DateTime::from_timestamp_millis(1_700_000_000_123).unwrap(),
            body: Some("hello".into()),
            ..Default::default()
        }
    }

    /// Hand-fed channel triple around a test ReceiveEngine: (event sender,
    /// status sender, command receiver, backend).
    fn engine_backend() -> (
        mpsc::UnboundedSender<EngineEvent>,
        watch::Sender<EngineStatus>,
        mpsc::UnboundedReceiver<send::SendCommand>,
        NativeBackend,
    ) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (status_tx, status_rx) = watch::channel(EngineStatus::Connecting);
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let mut backend = NativeBackend::new();
        backend.engine = Some(ReceiveEngine::for_test(event_rx, status_rx, command_tx));
        (event_tx, status_tx, command_rx, backend)
    }

    fn message_req(recipient: &str, body: &str, local_ts_ms: i64) -> SendRequest {
        SendRequest::Message {
            recipient: recipient.into(),
            body: body.into(),
            is_group: false,
            local_ts_ms,
            mentions: vec![],
            text_styles: vec![],
            attachment: None,
            preview: None,
            quote_timestamp: None,
            quote_author: None,
            quote_body: None,
        }
    }

    fn message_count(app: &App, conv_id: &str) -> usize {
        app.store
            .conversations
            .get(conv_id)
            .map(|c| c.messages.len())
            .unwrap_or(0)
    }

    /// The KTD-2 recovery leg: journaled rows replay through the normal
    /// pipeline on startup, and a replayed row whose effects already
    /// committed is silenced by the v16 dedup index.
    #[tokio::test]
    async fn startup_replays_journal_and_dedups_double_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = file_backed_app(dir.path());
        let db_path = app.db.path().unwrap();

        // The same message journaled twice = the crash-between-commit-and-
        // delete window plus the redelivered envelope, worst case.
        let writer = JournalWriter::open(&db_path).unwrap();
        let event = SignalEvent::MessageReceived(incoming_message());
        let journaled = JournalEvent::from_signal(&event).unwrap();
        writer.append(&journaled).unwrap();
        writer.append(&journaled).unwrap();

        let mut backend = NativeBackend::new();
        backend.startup(&mut app).await;

        assert_eq!(
            message_count(&app, "+15550001111"),
            1,
            "duplicate journal rows must collapse to one message"
        );
        assert!(
            app.db.journal_pending().unwrap().is_empty(),
            "replayed rows are deleted"
        );
        assert!(!app.loading, "G5: loading clears on store load");
        // No native store for this account: honest terminal copy, no engine.
        assert!(app.connection_error.is_some());
        assert!(backend.engine.is_none());
    }

    #[tokio::test]
    async fn startup_with_unparseable_journal_row_drops_it_and_continues() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = file_backed_app(dir.path());
        let db_path = app.db.path().unwrap();

        let writer = JournalWriter::open(&db_path).unwrap();
        writer
            .append(
                &JournalEvent::from_signal(&SignalEvent::MessageReceived(incoming_message()))
                    .unwrap(),
            )
            .unwrap();
        // Simulate version skew: a second row whose payload no current
        // variant matches (as if written by a different build).
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO native_journal (payload) VALUES ('{\"FutureVariant\":{}}')",
            [],
        )
        .unwrap();
        drop(conn);

        let mut backend = NativeBackend::new();
        backend.startup(&mut app).await;

        assert_eq!(message_count(&app, "+15550001111"), 1);
        assert!(
            app.db.journal_pending().unwrap().is_empty(),
            "unparseable rows are dropped, not wedged"
        );
    }

    /// Live drain: handle the event, then delete its journal row - and no
    /// batching wraps the loop (autocommit visibility is what the engine
    /// thread's busy_timeout relies on).
    #[tokio::test]
    async fn drain_handles_event_then_deletes_journal_row() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = file_backed_app(dir.path());
        let db_path = app.db.path().unwrap();

        let writer = JournalWriter::open(&db_path).unwrap();
        let event = SignalEvent::MessageReceived(incoming_message());
        let id = writer
            .append(&JournalEvent::from_signal(&event).unwrap())
            .unwrap();

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (_status_tx, status_rx) = watch::channel(EngineStatus::Connecting);
        let (_command_tx, _command_rx) = mpsc::unbounded_channel();
        let mut backend = NativeBackend::new();
        backend.engine = Some(ReceiveEngine::for_test(event_rx, status_rx, _command_tx));

        event_tx
            .send(EngineEvent {
                journal_id: Some(id),
                event,
            })
            .unwrap();

        assert!(backend.drain_events(&mut app));
        assert_eq!(message_count(&app, "+15550001111"), 1);
        assert!(
            app.db.journal_pending().unwrap().is_empty(),
            "journal row deleted after the event was handled"
        );
    }

    #[tokio::test]
    async fn drain_reflects_engine_status_transitions() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = file_backed_app(dir.path());

        let (_event_tx, event_rx) = mpsc::unbounded_channel();
        let (status_tx, status_rx) = watch::channel(EngineStatus::Connecting);
        let (_command_tx, _command_rx) = mpsc::unbounded_channel();
        let mut backend = NativeBackend::new();
        backend.engine = Some(ReceiveEngine::for_test(event_rx, status_rx, _command_tx));

        status_tx.send(EngineStatus::Connected).unwrap();
        assert!(backend.drain_events(&mut app));
        assert!(app.connected);
        assert!(app.connection_error.is_none());

        status_tx
            .send(EngineStatus::Reconnecting { attempt: 3 })
            .unwrap();
        backend.drain_events(&mut app);
        assert!(!app.connected);
        assert!(app.status_message.contains("attempt 3"));

        status_tx
            .send(EngineStatus::Terminal("relink needed".into()))
            .unwrap();
        backend.drain_events(&mut app);
        assert!(!app.connected);
        assert_eq!(app.connection_error.as_deref(), Some("relink needed"));
    }

    #[tokio::test]
    async fn drain_reports_supervisor_death_without_terminal_status() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = file_backed_app(dir.path());
        app.connected = true;

        let (event_tx, event_rx) = mpsc::unbounded_channel::<EngineEvent>();
        let (_status_tx, status_rx) = watch::channel(EngineStatus::Connected);
        let (_command_tx, _command_rx) = mpsc::unbounded_channel();
        let mut backend = NativeBackend::new();
        backend.engine = Some(ReceiveEngine::for_test(event_rx, status_rx, _command_tx));
        drop(event_tx); // supervisor panicked: channel closes with no Terminal

        backend.drain_events(&mut app);
        assert!(!app.connected);
        assert_eq!(
            app.connection_error.as_deref(),
            Some("native engine stopped unexpectedly")
        );
    }

    #[tokio::test]
    async fn drain_without_engine_is_a_quiet_noop() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = file_backed_app(dir.path());
        let mut backend = NativeBackend::new();
        assert!(!backend.drain_events(&mut app));
    }

    // --- U12 dispatch: send routing (KTD-4) ---

    #[tokio::test]
    async fn dispatch_routes_sends_with_strictly_increasing_wire_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = file_backed_app(dir.path());
        let (_event_tx, _status_tx, mut command_rx, mut backend) = engine_backend();

        // Two sends in (almost certainly) the same millisecond: KTD-4
        // demands distinct, strictly increasing wire timestamps anyway.
        backend
            .dispatch(&mut app, message_req("+15550001111", "one", 111))
            .await;
        backend
            .dispatch(&mut app, message_req("+15550001111", "two", 222))
            .await;

        let first = match command_rx.try_recv().unwrap() {
            send::SendCommand::Message {
                token,
                body,
                timestamp_ms,
                ..
            } => {
                assert_eq!(body, "one");
                assert_eq!(token.to_string(), timestamp_ms.to_string());
                timestamp_ms
            }
            _ => panic!("expected Message command"),
        };
        let second = match command_rx.try_recv().unwrap() {
            send::SendCommand::Message { timestamp_ms, .. } => timestamp_ms,
            _ => panic!("expected Message command"),
        };
        assert!(
            second > first,
            "wire timestamps must be strictly increasing: {first} then {second}"
        );
        assert_eq!(
            app.pending.sends.len(),
            2,
            "both sends registered for confirmation correlation"
        );
    }

    #[tokio::test]
    async fn dispatch_routes_group_messages_to_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = file_backed_app(dir.path());
        let (_event_tx, _status_tx, mut command_rx, mut backend) = engine_backend();

        let mut group = message_req("Z3JvdXBpZA==", "hi group", 1);
        if let SendRequest::Message { is_group, .. } = &mut group {
            *is_group = true;
        }
        backend.dispatch(&mut app, group).await;

        let command = command_rx
            .try_recv()
            .expect("group send reaches the engine");
        let send::SendCommand::GroupMessage { group_id, body, .. } = command else {
            panic!("expected GroupMessage command");
        };
        assert_eq!(group_id, "Z3JvdXBpZA==");
        assert_eq!(body, "hi group");
        assert_eq!(
            app.pending.sends.len(),
            1,
            "group send registered for confirmation correlation"
        );
    }

    #[tokio::test]
    async fn dispatch_gates_attachments_honestly() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = file_backed_app(dir.path());
        let (_event_tx, _status_tx, mut command_rx, mut backend) = engine_backend();

        // 1:1 attachment: still U14.
        let mut with_attachment = message_req("+15550001111", "hi", 1);
        if let SendRequest::Message { attachment, .. } = &mut with_attachment {
            *attachment = Some(std::path::PathBuf::from("x.png"));
        }
        backend.dispatch(&mut app, with_attachment).await;
        assert!(app.status_message.contains("U14"));

        // Group attachment: the attachment gate wins over group routing.
        let mut group_attachment = message_req("Z3JvdXBpZA==", "hi", 2);
        if let SendRequest::Message {
            is_group,
            attachment,
            ..
        } = &mut group_attachment
        {
            *is_group = true;
            *attachment = Some(std::path::PathBuf::from("x.png"));
        }
        backend.dispatch(&mut app, group_attachment).await;
        assert!(app.status_message.contains("U14"));

        assert!(
            command_rx.try_recv().is_err(),
            "gated requests never reach the engine"
        );
        assert!(app.pending.sends.is_empty());
    }

    #[tokio::test]
    async fn dispatch_with_dead_engine_thread_fails_the_send_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = file_backed_app(dir.path());
        let (_event_tx, _status_tx, command_rx, mut backend) = engine_backend();
        drop(command_rx); // engine thread died

        backend
            .dispatch(&mut app, message_req("+15550001111", "hi", 1))
            .await;
        assert!(
            app.pending.sends.is_empty(),
            "no confirmation will ever arrive; the pending entry must not leak"
        );
        assert!(app.status_message.contains("engine stopped"));
    }

    #[tokio::test]
    async fn dispatch_without_engine_reports_not_connected() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = file_backed_app(dir.path());
        let mut backend = NativeBackend::new();
        backend
            .dispatch(&mut app, message_req("+15550001111", "hi", 1))
            .await;
        assert_eq!(app.status_message, "native engine: not connected");
    }

    #[tokio::test]
    async fn dispatch_routes_username_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = file_backed_app(dir.path());
        let (_event_tx, _status_tx, mut command_rx, mut backend) = engine_backend();
        backend
            .dispatch(
                &mut app,
                SendRequest::ResolveUsername {
                    username: "ada.42".into(),
                },
            )
            .await;
        assert!(matches!(
            command_rx.try_recv().unwrap(),
            send::SendCommand::ResolveUsername { username } if username == "ada.42"
        ));
    }

    #[tokio::test]
    async fn dispatch_names_unrouted_operations() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = file_backed_app(dir.path());
        let (_event_tx, _status_tx, _command_rx, mut backend) = engine_backend();
        backend
            .dispatch(
                &mut app,
                SendRequest::Reaction {
                    conv_id: "+15550001111".into(),
                    emoji: "x".into(),
                    is_group: false,
                    target_author: "+15550001111".into(),
                    target_timestamp: 1,
                    remove: false,
                },
            )
            .await;
        assert!(
            app.status_message.contains("reactions"),
            "capability copy names the operation: {}",
            app.status_message
        );
    }
}
