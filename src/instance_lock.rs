//! Single-instance guard for on-disk engine state (#685).
//!
//! signal-cli enforces one process per account; the native engine's sqlite
//! store had no equivalent, so two siggy processes could open it at once and
//! fight over the websocket session. This module provides the guard: an
//! exclusive OS-level advisory lock (`flock` on Unix, `LockFileEx` on
//! Windows, via std's `File::try_lock`) on a lockfile beside the store.
//!
//! Stale-lock safety comes from the OS: the lock belongs to the open file
//! description, so it is released when the process exits for any reason
//! (including a crash or SIGKILL). The lockfile itself may linger, but an
//! unlocked lockfile never blocks the next start, unlike a PID file.
//!
//! Engine-agnostic and compiled in every build so it unit-tests without the
//! `native-backend` feature (whose presage tree needs `protoc`).

// Only the native engine consumes the guard today; the default build still
// compiles (and tests) it.
#![cfg_attr(not(feature = "native-backend"), allow(dead_code))]

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Actionable copy shown when another process holds the account lock. Same
/// shape as the signal-cli path's "is in use by another instance" hint
/// (`cli_stderr_hint` in main.rs).
pub const IN_USE_HINT: &str =
    "another siggy instance is using this account - close it and try again";

/// Why [`InstanceLock::try_acquire`] failed.
#[derive(Debug)]
pub enum LockError {
    /// Another process (or another open handle) holds the lock.
    Held,
    /// The lockfile could not be opened or locked for another reason.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Held => f.write_str(IN_USE_HINT),
            LockError::Io { path, source } => {
                write!(f, "failed to lock {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for LockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LockError::Held => None,
            LockError::Io { source, .. } => Some(source),
        }
    }
}

/// An exclusive lock on a lockfile, released when dropped (or when the
/// process exits).
#[derive(Debug)]
pub struct InstanceLock {
    path: PathBuf,
    // Held only for its lock; closing the handle releases it.
    _file: File,
}

impl InstanceLock {
    /// Take the exclusive lock on `path`, creating the file (0600 on Unix)
    /// if needed. Non-blocking: fails with [`LockError::Held`] immediately
    /// when someone else holds it. The parent directory must exist.
    pub fn try_acquire(path: &Path) -> Result<Self, LockError> {
        let io_err = |source| LockError::Io {
            path: path.to_path_buf(),
            source,
        };
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(path).map_err(io_err)?;
        match file.try_lock() {
            Ok(()) => Ok(Self {
                path: path.to_path_buf(),
                _file: file,
            }),
            Err(TryLockError::WouldBlock) => Err(LockError::Held),
            Err(TryLockError::Error(e)) => Err(io_err(e)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Locks held for the remainder of the process, keyed by lockfile path.
static PROCESS_LOCKS: Mutex<Vec<InstanceLock>> = Mutex::new(Vec::new());

/// Take the lock on `path` and keep it until the process exits. Idempotent
/// within one process: the engine opens its store from several places (link
/// probe, receive supervisor, send loop), and a second `flock` on a fresh
/// handle would otherwise conflict with our own first one.
pub fn hold_for_process(path: &Path) -> Result<(), LockError> {
    let mut held = PROCESS_LOCKS.lock().unwrap_or_else(|p| p.into_inner());
    if held.iter().any(|l| l.path() == path) {
        return Ok(());
    }
    held.push(InstanceLock::try_acquire(path)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_while_first_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.lock");
        let first = InstanceLock::try_acquire(&path).unwrap();
        let second = InstanceLock::try_acquire(&path);
        assert!(matches!(second, Err(LockError::Held)), "got {second:?}");
        drop(first);
    }

    #[test]
    fn acquire_succeeds_again_after_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.lock");
        let first = InstanceLock::try_acquire(&path).unwrap();
        drop(first);
        // The lockfile lingers on disk, but an unlocked file never blocks
        // (the crashed-process case).
        assert!(path.exists());
        let again = InstanceLock::try_acquire(&path);
        assert!(again.is_ok(), "got {again:?}");
    }

    #[test]
    fn held_error_carries_actionable_copy() {
        assert_eq!(LockError::Held.to_string(), IN_USE_HINT);
        assert!(IN_USE_HINT.contains("close it and try again"));
    }

    #[test]
    fn missing_parent_dir_is_io_error_not_held() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-such-dir").join("store.lock");
        assert!(matches!(
            InstanceLock::try_acquire(&path),
            Err(LockError::Io { .. })
        ));
    }

    #[test]
    fn hold_for_process_is_idempotent_and_excludes_others() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.lock");
        hold_for_process(&path).unwrap();
        // Same process, same path: a no-op, not a self-conflict.
        hold_for_process(&path).unwrap();
        // Any other handle (standing in for a second process) is refused.
        assert!(matches!(
            InstanceLock::try_acquire(&path),
            Err(LockError::Held)
        ));
    }
}
