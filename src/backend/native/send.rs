//! Native send path (#642 U12, plan KTD-4).
//!
//! Sends run on the engine thread beside the receive supervisor: the
//! adapter's `dispatch` pushes a [`SendCommand`] over the engine's command
//! channel, the loop here resolves the recipient against the store (the
//! reverse of KTD-6's identity mapping) and drives
//! `Manager::send_message`. Each send runs as its own `spawn_local` task
//! with a cloned Manager, so a hung websocket never blocks later sends.
//!
//! Status semantics (KTD-4): the caller-chosen wire timestamp doubles as
//! the [`SendToken`], `Ok` synthesizes `SendTimestamp` (spike finding:
//! `Ok` means server-accepted), `Err` or a 30s timeout synthesizes
//! `SendFailed` - and a timed-out future is deliberately NOT dropped: a
//! late `Ok` still emits `SendTimestamp`, which upgrades the message
//! Failed → Sent through the dedicated status route (the #486 lesson).

use std::path::{Path, PathBuf};
use std::time::Duration;

use presage::libsignal_service::content::DataMessage;
use presage::libsignal_service::prelude::{phonenumber, Uuid};
use presage::libsignal_service::protocol::{Aci, ServiceId};
use presage::manager::{Manager, Registered};
use presage::model::identity::OnNewIdentity;
use presage::store::ContentsStore;
use presage_store_sqlite::SqliteStore;
use tokio::sync::mpsc;

use crate::debug_log;
use crate::signal::types::{SendToken, SignalEvent, UserStatus};

use super::supervisor::EngineEvent;

/// KTD-4: native has no eventually-answering RPC peer; a hung websocket
/// must not leave messages in Sending forever.
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// The adapter → engine send vocabulary (U12: 1:1 messages and username
/// resolution; groups are U13, attachments U14, rich bodies U15).
pub enum SendCommand {
    Message {
        token: SendToken,
        /// Conversation key: E.164 or ACI uuid (KTD-6 formats).
        recipient: String,
        body: String,
        /// Wire timestamp, already uniqueness-adjusted by the adapter.
        timestamp_ms: u64,
    },
    ResolveUsername {
        username: String,
    },
}

/// How a conversation key converts back to a wire identity (the reverse of
/// the KTD-6 selection rule). Pure so it unit-tests without a store.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum RecipientKind {
    Aci(Uuid),
    Number(String),
}

pub(super) fn classify_recipient(key: &str) -> RecipientKind {
    match key.parse::<Uuid>() {
        Ok(uuid) => RecipientKind::Aci(uuid),
        Err(_) => RecipientKind::Number(key.to_string()),
    }
}

/// Long-lived command loop, spawned on the supervisor's LocalSet. Loads
/// its own Manager lazily on the first send so a send-free session never
/// pays for it.
pub(super) async fn run_send_loop(
    store_file: PathBuf,
    mut commands: mpsc::UnboundedReceiver<SendCommand>,
    event_tx: mpsc::UnboundedSender<EngineEvent>,
) {
    let mut manager: Option<Manager<SqliteStore, Registered>> = None;
    while let Some(command) = commands.recv().await {
        let Some(manager) = ensure_manager(&mut manager, &store_file, &command, &event_tx).await
        else {
            continue;
        };
        match command {
            SendCommand::Message {
                token,
                recipient,
                body,
                timestamp_ms,
            } => {
                // Per-send Manager clone: sends are independent tasks, and
                // one stuck 30s timeout must not serialize the rest.
                let manager = manager.clone();
                let event_tx = event_tx.clone();
                tokio::task::spawn_local(send_one(
                    manager,
                    token,
                    recipient,
                    body,
                    timestamp_ms,
                    event_tx,
                ));
            }
            SendCommand::ResolveUsername { username } => {
                resolve_username(manager, &username, &event_tx).await;
            }
        }
    }
}

/// Load the send-side Manager on first use; on failure, fail the command
/// honestly (a Message gets its SendFailed, a lookup gets an unregistered
/// answer) instead of wedging.
async fn ensure_manager<'a>(
    slot: &'a mut Option<Manager<SqliteStore, Registered>>,
    store_file: &Path,
    command: &SendCommand,
    event_tx: &mpsc::UnboundedSender<EngineEvent>,
) -> Option<&'a mut Manager<SqliteStore, Registered>> {
    if slot.is_none() {
        match load_manager(store_file).await {
            Ok(manager) => *slot = Some(manager),
            Err(e) => {
                debug_log::logf(format_args!("native send: manager load failed: {e}"));
                match command {
                    SendCommand::Message { token, .. } => {
                        emit(
                            event_tx,
                            SignalEvent::SendFailed {
                                token: token.clone(),
                            },
                        );
                    }
                    SendCommand::ResolveUsername { .. } => {
                        emit(
                            event_tx,
                            SignalEvent::Error(format!("Username lookup failed: {e}")),
                        );
                    }
                }
                return None;
            }
        }
    }
    slot.as_mut()
}

async fn load_manager(store_file: &Path) -> anyhow::Result<Manager<SqliteStore, Registered>> {
    let path = store_file
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("native store path is not valid UTF-8"))?;
    let store = SqliteStore::open(path, OnNewIdentity::Trust).await?;
    Ok(Manager::load_registered(store).await?)
}

/// One send, one task: resolve → send with the KTD-4 timeout contract.
async fn send_one(
    mut manager: Manager<SqliteStore, Registered>,
    token: SendToken,
    recipient_key: String,
    body: String,
    timestamp_ms: u64,
    event_tx: mpsc::UnboundedSender<EngineEvent>,
) {
    let Some(recipient) = resolve_recipient(&manager, &recipient_key).await else {
        debug_log::logf(format_args!(
            "native send: no wire identity for {}",
            debug_log::mask_phone(&recipient_key)
        ));
        emit(&event_tx, SignalEvent::SendFailed { token });
        return;
    };
    let message = DataMessage {
        body: Some(body),
        timestamp: Some(timestamp_ms),
        ..Default::default()
    };

    let send = manager.send_message(recipient, message, timestamp_ms);
    tokio::pin!(send);
    tokio::select! {
        result = &mut send => match result {
            // Spike finding (#639 question 2): Ok means server-accepted;
            // the local timestamp IS the wire timestamp, so the #480
            // rewrite dance no-ops.
            Ok(()) => emit(&event_tx, SignalEvent::SendTimestamp {
                token,
                server_ts: timestamp_ms as i64,
            }),
            Err(e) => {
                debug_log::logf(format_args!("native send failed: {e}"));
                emit(&event_tx, SignalEvent::SendFailed { token });
            }
        },
        _ = tokio::time::sleep(SEND_TIMEOUT) => {
            // KTD-4: report the timeout, but keep driving the future - a
            // late Ok upgrades Failed → Sent exactly once via the
            // dedicated status route (#486).
            emit(&event_tx, SignalEvent::SendFailed { token: token.clone() });
            match send.await {
                Ok(()) => emit(&event_tx, SignalEvent::SendTimestamp {
                    token,
                    server_ts: timestamp_ms as i64,
                }),
                Err(e) => debug_log::logf(format_args!(
                    "native send failed after timeout: {e}"
                )),
            }
        }
    }
}

/// Conversation key → wire ServiceId (reverse KTD-6): uuid keys convert
/// directly; E.164 keys resolve through the store's contact sync. An
/// unresolvable number means contact sync has not supplied it yet - the
/// send fails rather than guessing.
/// Note to Self (Tier-3 finding, #642): the account's own number never
/// appears in the synced contact list, so it must resolve from the
/// registration data before the contacts walk gets a chance to fail it.
fn own_aci_for_number(number: &str, own_e164: &str, own_aci: Uuid) -> Option<ServiceId> {
    (number == own_e164).then(|| Aci::from(own_aci).into())
}

async fn resolve_recipient(
    manager: &Manager<SqliteStore, Registered>,
    key: &str,
) -> Option<ServiceId> {
    match classify_recipient(key) {
        RecipientKind::Aci(uuid) => Some(Aci::from(uuid).into()),
        RecipientKind::Number(number) => {
            let data = manager.registration_data();
            let own_e164 = data
                .phone_number
                .format()
                .mode(phonenumber::Mode::E164)
                .to_string();
            if let Some(own) = own_aci_for_number(&number, &own_e164, data.service_ids.aci) {
                return Some(own);
            }
            let contacts = match manager.store().contacts().await {
                Ok(contacts) => contacts,
                Err(e) => {
                    debug_log::logf(format_args!("native send: contacts read failed: {e}"));
                    return None;
                }
            };
            for contact in contacts.flatten() {
                if super::supervisor::contact_e164(&contact).as_deref() == Some(number.as_str()) {
                    return Some(Aci::from(contact.uuid).into());
                }
            }
            None
        }
    }
}

/// `/join @handle` resolution (#612): the native twin of signal-cli's
/// getUserStatus round-trip, answered from `lookup_username`. The reply
/// mirrors signal-cli's `u:`-prefixed recipient echo so
/// `handle_user_status_list` matches it unchanged.
async fn resolve_username(
    manager: &mut Manager<SqliteStore, Registered>,
    username: &str,
    event_tx: &mpsc::UnboundedSender<EngineEvent>,
) {
    let status = match manager.lookup_username(username).await {
        Ok(found) => UserStatus {
            recipient: format!("u:{username}"),
            username: Some(username.to_string()),
            uuid: found.map(|aci| aci.service_id_string()),
            registered: found.is_some(),
        },
        Err(e) => {
            emit(
                event_tx,
                SignalEvent::Error(format!("Username lookup failed: {e}")),
            );
            return;
        }
    };
    emit(event_tx, SignalEvent::UserStatusList(vec![status]));
}

fn emit(event_tx: &mpsc::UnboundedSender<EngineEvent>, event: SignalEvent) {
    let _ = event_tx.send(EngineEvent {
        journal_id: None,
        event,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Note to Self (Tier-3 finding, #642): the account's own number never
    /// appears in the synced contact list, so the contacts walk alone fails
    /// every send-to-self. The own-number check must answer first.
    #[test]
    fn own_number_resolves_to_own_aci_without_contacts() {
        let own_aci: Uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".parse().unwrap();
        assert_eq!(
            own_aci_for_number("+15551234567", "+15551234567", own_aci),
            Some(Aci::from(own_aci).into())
        );
    }

    #[test]
    fn other_numbers_fall_through_to_the_contacts_walk() {
        let own_aci: Uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".parse().unwrap();
        assert_eq!(
            own_aci_for_number("+15559999999", "+15551234567", own_aci),
            None
        );
    }

    #[test]
    fn uuid_keys_classify_as_aci() {
        let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        assert_eq!(
            classify_recipient(uuid),
            RecipientKind::Aci(uuid.parse().unwrap())
        );
    }

    #[test]
    fn e164_keys_classify_as_number() {
        assert_eq!(
            classify_recipient("+15551234567"),
            RecipientKind::Number("+15551234567".into())
        );
    }

    #[test]
    fn group_ids_and_garbage_fall_through_as_numbers() {
        // A base64 group id is not a uuid; classification never panics on
        // arbitrary keys (the dispatch layer gates groups before this).
        assert!(matches!(
            classify_recipient("Z3JvdXBpZGdyb3VwaWRncm91cGlkZ3JvdXBpZA=="),
            RecipientKind::Number(_)
        ));
    }
}
