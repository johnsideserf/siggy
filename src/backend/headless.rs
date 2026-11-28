//! Engine seam for the headless CLI modes (`--send`, `--receive`,
//! `--watch`; #686).
//!
//! Those modes need raw "send one text" and "next event" access without an
//! [`App`](crate::app::App), which the [`Backend`](super::Backend) trait
//! deliberately does not expose. [`Headless`] is that narrower contract,
//! with one cfg-selected implementation per engine, so a native build is
//! native end to end instead of shelling out to signal-cli for the oneshot
//! commands. [`start`] is the instance-free entry point, the headless twin
//! of [`super::probe_link_state`].

use anyhow::Result;

#[cfg(feature = "signal-cli-backend")]
use crate::config::Config;
use crate::signal::types::{SendToken, SignalEvent};

/// What the headless modes need from an engine.
pub trait Headless {
    /// Next backend event, or `None` once the engine has stopped
    /// (signal-cli exited, native supervisor terminated).
    async fn next_event(&mut self) -> Option<SignalEvent>;

    /// Send one plain-text message. `recipient` is a phone number, ACI
    /// uuid, `u:`-prefixed username, or (with `is_group`) a group id. The
    /// returned token correlates the eventual `SendTimestamp` /
    /// `SendFailed` event.
    async fn send_text(&mut self, recipient: &str, body: &str, is_group: bool)
    -> Result<SendToken>;

    /// Stop the engine. Returns a user-facing reason when it stopped for a
    /// recognised fatal cause (account lock held, not registered, relink
    /// needed), so the caller can report it instead of exiting cleanly.
    async fn finish(&mut self) -> Option<String>;
}

/// Start the compiled-in engine for a headless command.
#[cfg(feature = "signal-cli-backend")]
pub async fn start(config: &Config) -> Result<crate::signal::client::SignalClient> {
    crate::signal::client::SignalClient::spawn(config).await
}

#[cfg(feature = "signal-cli-backend")]
impl Headless for crate::signal::client::SignalClient {
    async fn next_event(&mut self) -> Option<SignalEvent> {
        match self.event_rx.recv().await {
            Some(SignalEvent::Disconnected) | None => None,
            other => other,
        }
    }

    async fn send_text(
        &mut self,
        recipient: &str,
        body: &str,
        is_group: bool,
    ) -> Result<SendToken> {
        self.send_message(recipient, body, is_group, &[], &[], &[], None, None)
            .await
    }

    async fn finish(&mut self) -> Option<String> {
        let _ = self.shutdown().await;
        crate::cli_stderr_hint(&self.stderr_output()).map(str::to_string)
    }
}

#[cfg(feature = "native-backend")]
pub use native_impl::start;

#[cfg(feature = "native-backend")]
mod native_impl {
    use std::time::Duration;

    use anyhow::Result;

    use super::Headless;
    use crate::backend::native::send::SendCommand;
    use crate::backend::native::supervisor::{self, EngineStatus, ReceiveEngine};
    use crate::backend::native::{NativeBackend, store};
    use crate::config::Config;
    use crate::signal::types::{LinkState, SendToken, SignalEvent, UserStatus};

    /// How long `--send @user` waits for the username lookup before giving
    /// up (the send itself then has its own confirmation timeout).
    const USERNAME_LOOKUP_TIMEOUT: Duration = Duration::from_secs(30);

    /// The native engine driven without an `App`: the same receive
    /// supervisor and send loop the TUI uses.
    pub struct NativeHeadless {
        engine: ReceiveEngine,
        /// Last wire timestamp handed out (KTD-4, same rule as
        /// `NativeBackend::next_wire_ts`).
        last_wire_ts: i64,
        /// The supervisor's Terminal copy, once observed.
        terminal: Option<String>,
    }

    /// Gate on the store's link state before spawning anything, so an
    /// unlinked account gets the same recovery copy as `--check` instead of
    /// a supervisor that opens (and creates) an empty store.
    pub async fn start(config: &Config) -> Result<NativeHeadless> {
        match NativeBackend::link_state(config).await {
            LinkState::Linked => {}
            LinkState::Unlinked => anyhow::bail!(
                "this account is not registered - run `siggy --setup` to link a device"
            ),
            LinkState::Corrupt => anyhow::bail!(
                "local account data is unusable - run `siggy --reset-account`, then `siggy --setup`"
            ),
        }
        // Journal into siggy.db when it exists (KTD-2): the receive stream
        // acks envelopes before yielding them, so rows this process pulls
        // are replayed into the TUI's history on its next startup instead
        // of being lost.
        let db_path = crate::resolve_db_path(config);
        let journal_db = db_path.exists().then_some(db_path);
        let engine = supervisor::spawn(store::store_file(&config.account), journal_db)?;
        Ok(NativeHeadless::new(engine))
    }

    impl NativeHeadless {
        pub(crate) fn new(engine: ReceiveEngine) -> Self {
            Self {
                engine,
                last_wire_ts: 0,
                terminal: None,
            }
        }

        fn next_wire_ts(&mut self) -> i64 {
            let now = chrono::Utc::now().timestamp_millis();
            let ts = now.max(self.last_wire_ts + 1);
            self.last_wire_ts = ts;
            ts
        }

        fn command(&self, command: SendCommand) -> Result<()> {
            self.engine
                .commands
                .send(command)
                .map_err(|_| anyhow::anyhow!("native engine stopped"))
        }

        /// Resolve a `u:` username to its ACI uuid over the engine's lookup
        /// command. Unrelated events arriving meanwhile are dropped: only
        /// `--send` targets usernames, and it ignores them anyway.
        async fn resolve_username(&mut self, username: &str) -> Result<String> {
            self.command(SendCommand::ResolveUsername {
                username: username.to_string(),
            })?;
            let recipient = format!("u:{username}");
            let lookup = async {
                loop {
                    match self.next_event().await {
                        Some(SignalEvent::UserStatusList(statuses)) => {
                            if let Some(found) = username_lookup_result(&statuses, &recipient) {
                                return found;
                            }
                        }
                        Some(SignalEvent::Error(e)) => return Err(anyhow::anyhow!("{e}")),
                        Some(_) => {}
                        None => anyhow::bail!("native engine stopped during username lookup"),
                    }
                }
            };
            tokio::time::timeout(USERNAME_LOOKUP_TIMEOUT, lookup)
                .await
                .map_err(|_| anyhow::anyhow!("timed out looking up username {username}"))?
        }
    }

    /// The lookup answer for `recipient` (`u:name`) in a `UserStatusList`,
    /// if present: its ACI uuid, or an error when the name is unregistered.
    pub(super) fn username_lookup_result(
        statuses: &[UserStatus],
        recipient: &str,
    ) -> Option<Result<String>> {
        let status = statuses.iter().find(|s| s.recipient == recipient)?;
        Some(match (&status.uuid, status.registered) {
            (Some(uuid), true) => Ok(uuid.clone()),
            _ => Err(anyhow::anyhow!(
                "no Signal account has username {recipient}"
            )),
        })
    }

    impl Headless for NativeHeadless {
        async fn next_event(&mut self) -> Option<SignalEvent> {
            if self.terminal.is_some() {
                return None;
            }
            loop {
                tokio::select! {
                    // Drain queued events before acting on a status change.
                    biased;
                    event = self.engine.events.recv() => {
                        return event.map(|e| e.event);
                    }
                    changed = self.engine.status.changed() => {
                        let status = self.engine.status.borrow_and_update().clone();
                        if let EngineStatus::Terminal(msg) = status {
                            self.terminal = Some(msg);
                            return None;
                        }
                        if changed.is_err() {
                            // Supervisor gone without a Terminal status
                            // (panic): the event channel stays open while
                            // the send loop lives, so stop here.
                            self.terminal = Some("native engine stopped unexpectedly".into());
                            return None;
                        }
                    }
                }
            }
        }

        async fn send_text(
            &mut self,
            recipient: &str,
            body: &str,
            is_group: bool,
        ) -> Result<SendToken> {
            // The native send path resolves numbers and uuids only; a
            // username needs its lookup first (the TUI's /join does the
            // same round-trip before a conversation exists).
            let recipient = match recipient.strip_prefix("u:") {
                Some(username) if !is_group => self.resolve_username(username).await?,
                _ => recipient.to_string(),
            };
            let wire_ts = self.next_wire_ts();
            let token = SendToken::new(wire_ts.to_string());
            let body = body.to_string();
            let command = if is_group {
                SendCommand::GroupMessage {
                    token: token.clone(),
                    group_id: recipient,
                    body,
                    timestamp_ms: wire_ts as u64,
                }
            } else {
                SendCommand::Message {
                    token: token.clone(),
                    recipient,
                    body,
                    timestamp_ms: wire_ts as u64,
                }
            };
            self.command(command)?;
            Ok(token)
        }

        async fn finish(&mut self) -> Option<String> {
            self.terminal.clone()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::native::supervisor::EngineEvent;
        use tokio::sync::{mpsc, watch};

        fn headless() -> (
            mpsc::UnboundedSender<EngineEvent>,
            watch::Sender<EngineStatus>,
            mpsc::UnboundedReceiver<SendCommand>,
            NativeHeadless,
        ) {
            let (event_tx, event_rx) = mpsc::unbounded_channel();
            let (status_tx, status_rx) = watch::channel(EngineStatus::Connecting);
            let (command_tx, command_rx) = mpsc::unbounded_channel();
            let engine = ReceiveEngine::for_test(event_rx, status_rx, command_tx);
            (event_tx, status_tx, command_rx, NativeHeadless::new(engine))
        }

        #[tokio::test]
        async fn events_pass_through_and_terminal_status_ends_the_stream() {
            let (event_tx, status_tx, _command_rx, mut h) = headless();
            status_tx.send(EngineStatus::Connected).unwrap();
            event_tx
                .send(EngineEvent {
                    journal_id: None,
                    event: SignalEvent::SyncComplete,
                })
                .unwrap();
            assert!(matches!(
                h.next_event().await,
                Some(SignalEvent::SyncComplete)
            ));

            status_tx
                .send(EngineStatus::Terminal("relink needed".into()))
                .unwrap();
            assert!(h.next_event().await.is_none());
            assert!(h.next_event().await.is_none(), "stays stopped");
            assert_eq!(h.finish().await.as_deref(), Some("relink needed"));
        }

        #[tokio::test]
        async fn sends_route_to_the_engine_with_increasing_wire_timestamps() {
            let (_event_tx, _status_tx, mut command_rx, mut h) = headless();
            let first = h.send_text("+15550001111", "one", false).await.unwrap();
            let second = h.send_text("Z3JvdXBpZA==", "two", true).await.unwrap();

            let SendCommand::Message {
                token, recipient, ..
            } = command_rx.try_recv().unwrap()
            else {
                panic!("expected 1:1 Message");
            };
            assert_eq!((token, recipient.as_str()), (first.clone(), "+15550001111"));
            let SendCommand::GroupMessage {
                token, group_id, ..
            } = command_rx.try_recv().unwrap()
            else {
                panic!("expected GroupMessage");
            };
            assert_eq!((token, group_id.as_str()), (second.clone(), "Z3JvdXBpZA=="));
            assert!(
                second.to_string().parse::<i64>().unwrap()
                    > first.to_string().parse::<i64>().unwrap()
            );
        }

        #[tokio::test]
        async fn send_with_dead_engine_errors() {
            let (_event_tx, _status_tx, command_rx, mut h) = headless();
            drop(command_rx);
            assert!(h.send_text("+15550001111", "hi", false).await.is_err());
        }

        #[test]
        fn username_lookup_result_matches_the_requested_name() {
            let found = UserStatus {
                recipient: "u:ada.42".into(),
                username: Some("ada.42".into()),
                uuid: Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into()),
                registered: true,
            };
            let missing = UserStatus {
                recipient: "u:nobody.1".into(),
                username: Some("nobody.1".into()),
                uuid: None,
                registered: false,
            };
            let statuses = [found, missing];
            assert_eq!(
                username_lookup_result(&statuses, "u:ada.42")
                    .unwrap()
                    .unwrap(),
                "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
            );
            assert!(
                username_lookup_result(&statuses, "u:nobody.1")
                    .unwrap()
                    .is_err()
            );
            assert!(username_lookup_result(&statuses, "u:other.2").is_none());
        }
    }
}
