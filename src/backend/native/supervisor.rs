//! Receive supervisor (#642 U11 stage 2, plan KTD-2/KTD-5/KTD-6).
//!
//! Owns the long-lived engine thread: opens the account store, loads the
//! registered [`Manager`], emits the store's contact/group directory, then
//! consumes `receive_messages()` item by item - map, journal (KTD-2),
//! emit, and only then pull the next item. `QueueEmpty` maps to
//! [`SignalEvent::SyncComplete`] (KTD-5); the wall-clock sync heuristic
//! never runs natively.
//!
//! Reconnect policy (plan flow question Q4 default): network-class errors
//! retry indefinitely with capped exponential backoff - a laptop that
//! slept through the night reconnects on wake without ceremony. Auth and
//! store errors terminate with actionable copy instead; retrying those
//! would loop forever against a server that already said no.
//!
//! Everything here runs on the [`EngineThread`]'s current-thread runtime
//! because the store futures are partially `!Send` (KTD-8); the main
//! thread sees only the mpsc event stream and the watch status.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use futures::StreamExt;
use presage::manager::Manager;
use presage::model::identity::OnNewIdentity;
use presage::model::messages::Received;
use presage::store::ContentsStore;
use presage_store_sqlite::SqliteStore;
use tokio::sync::{mpsc, watch};

use crate::debug_log;
use crate::signal::types::{Contact, Group, SignalEvent};

use super::journal::{JournalEvent, JournalWriter};
use super::receive::{self, IdentityResolver};
use super::runtime::EngineThread;

/// Connection lifecycle as the main loop sees it, published over a watch
/// channel so `drain_events` reflects the latest state without queueing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineStatus {
    /// Opening the store / websocket (initial state).
    Connecting,
    /// Receive stream is live.
    Connected,
    /// Stream lost; retrying with capped backoff (1-based attempt count).
    Reconnecting { attempt: u32 },
    /// Unrecoverable: auth revoked or store unusable. The copy is
    /// user-facing (rendered via `app.connection_error`).
    Terminal(String),
}

/// One boundary event plus the journal row that guarantees it survives a
/// crash. `journal_id` is `None` for transient events and when journaling
/// is unavailable (in-memory DB in tests).
pub struct EngineEvent {
    pub journal_id: Option<i64>,
    pub event: SignalEvent,
}

/// Handle the adapter holds: the event queue, the status watch, and the
/// engine thread keeping both alive.
pub struct ReceiveEngine {
    pub events: mpsc::UnboundedReceiver<EngineEvent>,
    pub status: watch::Receiver<EngineStatus>,
    /// `None` only in tests that fake the channels; production always
    /// carries the thread handle so the supervisor outlives frames.
    #[allow(dead_code)]
    thread: Option<EngineThread<()>>,
}

impl ReceiveEngine {
    /// Test seam: a ReceiveEngine around hand-fed channels, no thread.
    #[cfg(test)]
    pub fn for_test(
        events: mpsc::UnboundedReceiver<EngineEvent>,
        status: watch::Receiver<EngineStatus>,
    ) -> Self {
        Self {
            events,
            status,
            thread: None,
        }
    }
}

/// Spawn the supervisor on its engine thread. `journal_db` is siggy.db's
/// path (None disables journaling - in-memory test DBs only; production
/// callers always pass it, this is not a soft-degrade switch).
pub fn spawn(store_file: PathBuf, journal_db: Option<PathBuf>) -> Result<ReceiveEngine> {
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (status_tx, status_rx) = watch::channel(EngineStatus::Connecting);
    let thread = EngineThread::spawn("siggy-native-receive", move || async move {
        run_supervisor(store_file, journal_db, event_tx, status_tx).await;
    })?;
    Ok(ReceiveEngine {
        events: event_rx,
        status: status_rx,
        thread: Some(thread),
    })
}

/// Same schedule as `ReconnectPolicy::backoff` (2s, 4s, ... capped 30s)
/// but with no attempt cap: the native engine self-heals indefinitely for
/// network-class failures instead of using the main loop's supervisor.
fn backoff(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.min(6)).min(30))
}

enum SessionEnd {
    /// Retry after backoff (network-class, or the stream simply ended).
    Retry(String),
    /// Stop the engine and surface the message (auth/store class).
    Terminal(String),
}

async fn run_supervisor(
    store_file: PathBuf,
    journal_db: Option<PathBuf>,
    event_tx: mpsc::UnboundedSender<EngineEvent>,
    status_tx: watch::Sender<EngineStatus>,
) {
    let journal = match &journal_db {
        Some(path) => match JournalWriter::open(path) {
            Ok(writer) => Some(writer),
            Err(e) => {
                // Without a journal the KTD-2 contract is void; running
                // anyway would silently downgrade durability.
                let _ = status_tx.send(EngineStatus::Terminal(format!(
                    "native engine: cannot open receive journal: {e}"
                )));
                return;
            }
        },
        None => None,
    };

    let mut attempt: u32 = 0;
    loop {
        let end = run_session(
            &store_file,
            journal.as_ref(),
            &event_tx,
            &status_tx,
            &mut attempt,
        )
        .await;
        if event_tx.is_closed() {
            // The adapter (and with it the app) is gone; nobody is
            // listening. Exit quietly.
            return;
        }
        match end {
            SessionEnd::Terminal(msg) => {
                debug_log::logf(format_args!("native receive terminal: {msg}"));
                let _ = status_tx.send(EngineStatus::Terminal(msg));
                return;
            }
            SessionEnd::Retry(detail) => {
                attempt += 1;
                debug_log::logf(format_args!(
                    "native receive retry (attempt {attempt}): {detail}"
                ));
                let _ = status_tx.send(EngineStatus::Reconnecting { attempt });
                tokio::time::sleep(backoff(attempt)).await;
            }
        }
    }
}

/// One stream lifetime: store open → manager → directory emission →
/// receive loop. Returns how it ended so the supervisor can decide
/// between backoff and termination.
async fn run_session(
    store_file: &std::path::Path,
    journal: Option<&JournalWriter>,
    event_tx: &mpsc::UnboundedSender<EngineEvent>,
    status_tx: &watch::Sender<EngineStatus>,
    attempt: &mut u32,
) -> SessionEnd {
    let Some(path) = store_file.to_str() else {
        return SessionEnd::Terminal("native store path is not valid UTF-8".into());
    };
    // Store-open failures are terminal: the file is local, retrying cannot
    // fix corruption, and `--check` / `--reset-account` own the recovery
    // copy (same classification as linking.rs's Corrupt).
    let store = match SqliteStore::open(path, OnNewIdentity::Trust).await {
        Ok(store) => store,
        Err(e) => {
            return SessionEnd::Terminal(format!(
                "native engine: cannot open account store (run --check; --reset-account recovers): {e}"
            ));
        }
    };
    let mut manager = match Manager::load_registered(store).await {
        Ok(manager) => manager,
        Err(e) => return classify_manager_error(e),
    };
    let own_aci = manager.registration_data().service_ids.aci.to_string();

    // A cloned store handle for directory reads after `receive_messages`
    // mutably borrows the manager.
    let store_handle = manager.store().clone();
    let mut resolver = load_resolver(&store_handle).await;

    // Spike finding: contact sync is NOT automatic on a linked device. On
    // a fresh store, ask the primary for it (best-effort; the payload
    // arrives as Received::Contacts below).
    if resolver.by_aci.is_empty() {
        if let Err(e) = manager.request_contacts().await {
            debug_log::logf(format_args!("request_contacts failed: {e}"));
        }
    }

    // Directory snapshot so conversations render with names immediately
    // (native twin of the startup listContacts/listGroups round-trip).
    emit_directory(&store_handle, &resolver, event_tx).await;

    let stream = match manager.receive_messages().await {
        Ok(stream) => stream,
        Err(e) => return classify_manager_error(e),
    };
    tokio::pin!(stream);
    *attempt = 0;
    let _ = status_tx.send(EngineStatus::Connected);

    while let Some(item) = stream.next().await {
        if matches!(item, Received::Contacts) {
            // Payload landed in the store; refresh the resolver and
            // re-emit the directory so late contact sync names existing
            // conversations (KTD-6; the uuid→E164 re-key is stage 3).
            resolver = load_resolver(&store_handle).await;
            emit_directory(&store_handle, &resolver, event_tx).await;
            continue;
        }
        for event in receive::map_received(&item, &own_aci, &resolver) {
            // KTD-2 ordering: the journal append commits before this
            // iteration ends, i.e. before the next stream pull can ack
            // anything further.
            let journal_id = match (journal, JournalEvent::from_signal(&event)) {
                (Some(writer), Some(journaled)) => match writer.append(&journaled) {
                    Ok(id) => Some(id),
                    Err(e) => {
                        // A failed append re-opens the ack window for this
                        // one event; log loudly and still deliver rather
                        // than dropping it on the floor.
                        debug_log::logf(format_args!("journal append failed: {e}"));
                        None
                    }
                },
                _ => None,
            };
            if event_tx.send(EngineEvent { journal_id, event }).is_err() {
                return SessionEnd::Terminal("event channel closed".into());
            }
        }
    }
    SessionEnd::Retry("receive stream ended".into())
}

/// Q4 default: auth/registration problems terminate with actionable copy;
/// everything else (websocket, IO, timeouts) is network-class and retries
/// forever.
fn classify_manager_error(e: presage::Error<presage_store_sqlite::SqliteStoreError>) -> SessionEnd {
    use presage::Error;
    match &e {
        Error::NotYetRegisteredError | Error::RelinkNecessary => SessionEnd::Terminal(format!(
            "native engine: this device is no longer linked ({e}); relink from the phone"
        )),
        Error::MissingKeyError(_) | Error::Store(_) => SessionEnd::Terminal(format!(
            "native engine: account store is unusable ({e}); --reset-account recovers"
        )),
        _ => SessionEnd::Retry(e.to_string()),
    }
}

/// Store-backed KTD-6 identity data: ACI uuid → (E.164, display name).
#[derive(Default)]
pub(super) struct StoreResolver {
    by_aci: HashMap<String, (Option<String>, Option<String>)>,
}

impl IdentityResolver for StoreResolver {
    fn number_for_aci(&self, aci: &str) -> Option<String> {
        self.by_aci.get(aci).and_then(|(number, _)| number.clone())
    }

    fn name_for_aci(&self, aci: &str) -> Option<String> {
        self.by_aci.get(aci).and_then(|(_, name)| name.clone())
    }
}

async fn load_resolver(store: &SqliteStore) -> StoreResolver {
    let mut by_aci = HashMap::new();
    match store.contacts().await {
        Ok(contacts) => {
            for contact in contacts {
                let Ok(contact) = contact else { continue };
                by_aci.insert(
                    contact.uuid.to_string(),
                    (e164(&contact), non_empty(&contact.name)),
                );
            }
        }
        Err(e) => debug_log::logf(format_args!("store contacts read failed: {e}")),
    }
    StoreResolver { by_aci }
}

/// KTD-6 format lock: numbers cross the boundary in E.164 exactly as
/// signal-cli renders them, never phonenumber's display formats.
fn e164(contact: &presage::model::contacts::Contact) -> Option<String> {
    use presage::libsignal_service::prelude::phonenumber::Mode;
    contact
        .phone_number
        .as_ref()
        .map(|p| p.format().mode(Mode::E164).to_string())
}

fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// Emit ContactList + GroupList from the store so `App` names
/// conversations and populates mention maps. Not journaled: re-emitted at
/// every session start and on every Contacts sync.
async fn emit_directory(
    store: &SqliteStore,
    resolver: &StoreResolver,
    event_tx: &mpsc::UnboundedSender<EngineEvent>,
) {
    let mut contacts_out = Vec::new();
    if let Ok(contacts) = store.contacts().await {
        for contact in contacts.flatten() {
            contacts_out.push(Contact {
                number: e164(&contact),
                name: non_empty(&contact.name),
                uuid: Some(contact.uuid.to_string()),
                // presage's contact model has no username field at the
                // pinned rev; username resolution stays a U12 send-path
                // concern (#612 lookup_username).
                username: None,
            });
        }
    }
    if !contacts_out.is_empty() {
        let _ = event_tx.send(EngineEvent {
            journal_id: None,
            event: SignalEvent::ContactList(contacts_out),
        });
    }

    let mut groups_out = Vec::new();
    match store.groups().await {
        Ok(groups) => {
            for entry in groups {
                let Ok((master_key, group)) = entry else {
                    continue;
                };
                let Some(id) = receive::derive_group_id(&master_key) else {
                    continue;
                };
                let mut members = Vec::new();
                let mut member_uuids = Vec::new();
                for member in &group.members {
                    let aci = member.aci.service_id_string();
                    // KTD-6 selection rule for the member roster too:
                    // E.164 when known, else the uuid string.
                    let key = resolver.number_for_aci(&aci).unwrap_or_else(|| aci.clone());
                    members.push(key.clone());
                    member_uuids.push((key, aci));
                }
                groups_out.push(Group {
                    id,
                    name: group.title.clone(),
                    members,
                    member_uuids,
                });
            }
        }
        Err(e) => debug_log::logf(format_args!("store groups read failed: {e}")),
    }
    if !groups_out.is_empty() {
        let _ = event_tx.send(EngineEvent {
            journal_id: None,
            event: SignalEvent::GroupList(groups_out),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_matches_reconnect_policy_schedule_without_a_cap() {
        assert_eq!(backoff(1), Duration::from_secs(2));
        assert_eq!(backoff(2), Duration::from_secs(4));
        assert_eq!(backoff(4), Duration::from_secs(16));
        assert_eq!(backoff(5), Duration::from_secs(30));
        // No attempt cap: hour-1000 still retries at the 30s ceiling.
        assert_eq!(backoff(100_000), Duration::from_secs(30));
    }

    #[test]
    fn resolver_applies_the_ktd6_selection_rule() {
        let mut by_aci = HashMap::new();
        by_aci.insert(
            "aaaa".to_string(),
            (Some("+15550001111".to_string()), Some("Ada".to_string())),
        );
        by_aci.insert("bbbb".to_string(), (None, None));
        let resolver = StoreResolver { by_aci };
        assert_eq!(
            resolver.number_for_aci("aaaa").as_deref(),
            Some("+15550001111")
        );
        assert_eq!(resolver.name_for_aci("aaaa").as_deref(), Some("Ada"));
        assert_eq!(resolver.number_for_aci("bbbb"), None);
        assert_eq!(resolver.number_for_aci("unknown"), None);
    }

    #[tokio::test]
    async fn terminal_manager_errors_are_classified_as_terminal() {
        let end = classify_manager_error(presage::Error::NotYetRegisteredError);
        assert!(matches!(end, SessionEnd::Terminal(msg) if msg.contains("relink")));
        let end = classify_manager_error(presage::Error::RelinkNecessary);
        assert!(matches!(end, SessionEnd::Terminal(_)));
    }

    #[tokio::test]
    async fn other_manager_errors_retry() {
        let io = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "socket died");
        let end = classify_manager_error(presage::Error::IoError(io));
        assert!(matches!(end, SessionEnd::Retry(_)));
    }
}
