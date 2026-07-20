//! Native device linking + link-state probe (#642 U10, plan R4/R5/R8).
//!
//! The provisioning flow is the spike's link leg (#639 findings) as
//! production code: `Manager::link_secondary_device` runs on a dedicated
//! [`EngineThread`] (its store futures are `!Send`), the provisioning URL
//! crosses back over presage's oneshot, and the caller renders it with the
//! backend-agnostic QR path in `crate::link`. Device name is "siggy",
//! matching the signal-cli convention in `link.rs`.
//!
//! The link-state probe reads the store's registration row directly
//! (`StateStore::is_registered`), replacing U9's always-Unlinked stub: every
//! "am I linked?" surface (`--check`, startup gating, the wizard) now
//! answers from the same store the engine will use.

use std::path::PathBuf;

use anyhow::{Context, Result};
use presage::libsignal_service::configuration::SignalServers;
use presage::manager::Manager;
use presage::model::identity::OnNewIdentity;
use presage::store::StateStore;
use presage_store_sqlite::SqliteStore;

use crate::config::Config;
use crate::signal::types::LinkState;

use super::runtime::EngineThread;
use super::store;

/// Device name shown in the phone's Linked Devices list. Same value the
/// signal-cli flow passes via `link -n` (see `link.rs`).
const DEVICE_NAME: &str = "siggy";

/// An in-flight native provisioning session: owns the engine thread running
/// `link_secondary_device` between "URI obtained" and "handshake complete".
/// The `LinkSession` twin for the native engine (plan U5 seam, step 2).
pub struct NativeLinkSession {
    engine: Option<EngineThread<Result<(), String>>>,
}

/// What one non-blocking [`NativeLinkSession::poll`] observed. Mirrors the
/// signal-cli session's poll vocabulary in `link.rs`.
pub enum NativeLinkPoll {
    /// Handshake still in progress; keep showing the QR.
    Pending,
    /// The device was linked successfully.
    Completed,
}

impl NativeLinkSession {
    /// Non-blocking completion check. Errors carry presage's failure detail
    /// (timeout, rate limit, rejected handshake) the way the signal-cli
    /// session surfaces stderr.
    pub async fn poll(&mut self) -> Result<NativeLinkPoll> {
        let finished = self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.is_finished());
        if !finished {
            return Ok(NativeLinkPoll::Pending);
        }
        let engine = self
            .engine
            .take()
            .context("link session polled after completion")?;
        // is_finished() was true, so this join returns without blocking.
        match engine.join() {
            Ok(Ok(())) => Ok(NativeLinkPoll::Completed),
            Ok(Err(e)) => anyhow::bail!("device linking failed: {e}"),
            Err(_) => anyhow::bail!("device linking thread panicked"),
        }
    }

    /// Abort the session (user cancelled). presage's provisioning future has
    /// no cancellation handle, so the engine thread is detached: it exits on
    /// its own when the provisioning window times out (~1 min). Safe because
    /// an interrupted handshake leaves the store unregistered (spike finding:
    /// a failed attempt does not corrupt the store, and `clear_registration`
    /// runs at the start of the next attempt).
    pub async fn cancel(&mut self) {
        self.engine = None;
    }
}

/// Obtain a provisioning URI by starting `link_secondary_device` on an
/// engine thread (plan U5 seam, step 1). Returns the URI for the
/// backend-agnostic QR renderer plus the session to poll for completion.
pub async fn start_link_session(config: &Config) -> Result<(String, NativeLinkSession)> {
    store::ensure_store_dir(&config.account)?;
    let store_file = store::store_file(&config.account);
    let (tx, rx) = futures::channel::oneshot::channel();

    let engine = EngineThread::spawn("siggy-native-link", move || async move {
        let store = match open_store(&store_file).await {
            Ok(store) => store,
            Err(e) => return Err(format!("{e:#}")),
        };
        match Manager::link_secondary_device(
            store,
            SignalServers::Production,
            DEVICE_NAME.to_string(),
            tx,
        )
        .await
        {
            // The manager drops here by design: U10 only proves the link and
            // persists registration to the store. U11's receive supervisor
            // owns a long-lived Manager.
            Ok(_manager) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    })
    .context("Failed to spawn native link thread")?;

    match rx.await {
        Ok(url) => Ok((
            url.to_string(),
            NativeLinkSession {
                engine: Some(engine),
            },
        )),
        Err(_) => {
            // The sender dropped without a URL: the engine future failed
            // before provisioning started. Join for the real error.
            let detail = match engine.join() {
                Ok(Err(e)) => e,
                Ok(Ok(())) => "engine exited before producing a provisioning URL".to_string(),
                Err(_) => "engine thread panicked".to_string(),
            };
            anyhow::bail!("Failed to start device linking: {detail}")
        }
    }
}

/// Real link-state probe (replaces U9's always-Unlinked stub): open the
/// account's store and read its registration row. Runs on an engine thread
/// because the store futures are `!Send`; the join is wrapped in
/// `spawn_blocking` so the probe never blocks the caller's runtime.
pub async fn link_state(config: &Config) -> LinkState {
    link_state_of_store(store::store_file(&config.account)).await
}

/// Testable core of [`link_state`], keyed by store file path.
///
/// Mapping: no store file → Unlinked (nothing was ever linked); store opens
/// and `is_registered` → Linked / Unlinked accordingly; store file exists
/// but cannot be opened → Corrupt (the `--reset-account` recovery path).
async fn link_state_of_store(store_file: PathBuf) -> LinkState {
    if !store_file.exists() {
        return LinkState::Unlinked;
    }
    let probe = EngineThread::spawn("siggy-native-linkstate", move || async move {
        match open_store(&store_file).await {
            Ok(store) => {
                if store.is_registered().await {
                    LinkState::Linked
                } else {
                    LinkState::Unlinked
                }
            }
            Err(_) => LinkState::Corrupt,
        }
    });
    let Ok(probe) = probe else {
        // Cannot even spawn a thread; report the unusable-store state so
        // `--check` fails loudly rather than pretending to be unlinked.
        return LinkState::Corrupt;
    };
    tokio::task::spawn_blocking(move || probe.join())
        .await
        .ok()
        .and_then(|joined| joined.ok())
        .unwrap_or(LinkState::Corrupt)
}

async fn open_store(store_file: &std::path::Path) -> Result<SqliteStore> {
    let path = store_file
        .to_str()
        .context("native store path is not valid UTF-8")?;
    SqliteStore::open(path, OnNewIdentity::Trust)
        .await
        .with_context(|| format!("Failed to open native store at {}", store_file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn link_state_missing_store_is_unlinked() {
        let dir = tempfile::tempdir().unwrap();
        let state = link_state_of_store(dir.path().join("nope.sqlite")).await;
        assert_eq!(state, LinkState::Unlinked);
    }

    #[tokio::test]
    async fn link_state_fresh_store_is_unlinked() {
        // A store presage created but never linked (e.g. cancelled QR):
        // opens fine, no registration row.
        let dir = tempfile::tempdir().unwrap();
        let store_file = dir.path().join("store.sqlite");
        open_store(&store_file).await.expect("create fresh store");
        let state = link_state_of_store(store_file).await;
        assert_eq!(state, LinkState::Unlinked);
    }

    #[tokio::test]
    async fn link_state_garbage_store_is_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let store_file = dir.path().join("store.sqlite");
        std::fs::write(&store_file, b"this is not a sqlite database").unwrap();
        let state = link_state_of_store(store_file).await;
        assert_eq!(state, LinkState::Corrupt);
    }

    // Linked requires real registration data from a completed provisioning
    // handshake; that is the Tier-3 manual checklist run (plan U10), not a
    // unit test.
}
