//! Per-account native store location (#642 U9, flow question Q1).
//!
//! The presage sqlite store holds live Signal identity private keys and
//! session state: strictly more sensitive than anything siggy stores today
//! (the message DB has content but no keys). It lives under the platform
//! data dir at `siggy/native/<account>/`, one directory per account so
//! multi-account setups (`db_path` overrides) never share identity state,
//! and the directory is created 0700 on Unix before any file lands in it.
//!
//! U10 tightens further if presage-store-sqlite's own file modes turn out
//! looser than 0600 (spike left this open).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Root of all native-engine account stores: `<data_dir>/siggy/native/`.
pub fn native_root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("siggy")
        .join("native")
}

/// The store directory for one account. Pure path math, no filesystem
/// access; use [`ensure_store_dir`] when the directory must exist.
pub fn store_dir(account: &str) -> PathBuf {
    native_root().join(account)
}

/// The sqlite file presage-store-sqlite opens (U10 passes this to
/// `SqliteStore::open`).
pub fn store_file(account: &str) -> PathBuf {
    store_dir(account).join("store.sqlite")
}

/// Create the account's store directory with restrictive permissions
/// (0700 on Unix) and return its path.
pub fn ensure_store_dir(account: &str) -> Result<PathBuf> {
    let dir = store_dir(account);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create native store dir {}", dir.display()))?;
    crate::config::Config::set_dir_permissions(&dir);
    Ok(dir)
}

/// The single-instance lockfile for the store at `store_file` (#685):
/// `store.sqlite.lock` beside it in the account directory.
/// [`delete_account_data`] (relink, `--reset-account`) deliberately leaves
/// this one file in place: unlinking a held lockfile would let a second
/// process create and lock a fresh inode at the same path.
pub fn lock_file_for(store_file: &Path) -> PathBuf {
    let mut name = store_file.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

/// Take this process's exclusive lock on the store at `store_file`, held
/// until exit (#685). Every path that opens the store calls this first;
/// repeat calls within one process are no-ops. Fails with
/// [`LockError::Held`](crate::instance_lock::LockError::Held) (carrying the
/// actionable "close it and try again" copy) when another siggy holds it.
pub fn lock_store(store_file: &Path) -> Result<(), crate::instance_lock::LockError> {
    let lock = lock_file_for(store_file);
    if let Some(parent) = lock.parent().filter(|p| !p.exists()) {
        std::fs::create_dir_all(parent).map_err(|source| crate::instance_lock::LockError::Io {
            path: lock.clone(),
            source,
        })?;
        crate::config::Config::set_dir_permissions(parent);
    }
    crate::instance_lock::hold_for_process(&lock)
}

/// [`lock_store`] for an account's default store location.
pub fn lock_account(account: &str) -> Result<(), crate::instance_lock::LockError> {
    lock_store(&store_file(account))
}

/// Whether any native store data exists for this account. The native twin
/// of `setup::account_exists_locally`'s signal-cli accounts.json probe,
/// consulted by `--reset-account` and the relink guard (#603).
pub fn account_data_exists(account: &str) -> bool {
    store_file(account).exists()
}

/// Delete everything in the account's native store directory except the
/// single-instance lockfile (see [`lock_file_for`]). The native twin of
/// signal-cli's `deleteLocalAccountData`; purely local, never touches the
/// Signal servers. Refuses while another siggy holds the store (#685).
pub fn delete_account_data(account: &str) -> Result<()> {
    lock_account(account)?;
    let dir = store_dir(account);
    let lock = lock_file_for(&store_file(account));
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("Failed to read native store dir {}", dir.display()));
        }
    };
    for entry in entries {
        let path = entry
            .with_context(|| format!("Failed to read native store dir {}", dir.display()))?
            .path();
        if path == lock {
            continue;
        }
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        removed.with_context(|| format!("Failed to delete {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_paths_are_per_account() {
        let a = store_dir("+15551230001");
        let b = store_dir("+15551230002");
        assert_ne!(a, b);
        assert!(a.ends_with("+15551230001"));
        assert!(store_file("+15551230001").starts_with(&a));
    }

    #[test]
    fn lock_file_sits_beside_the_store() {
        let store = store_file("+15551230001");
        assert_eq!(
            lock_file_for(&store),
            store_dir("+15551230001").join("store.sqlite.lock")
        );
    }

    #[test]
    fn lock_store_excludes_a_second_holder() {
        use crate::instance_lock::{InstanceLock, LockError};
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("acct").join("store.sqlite");
        // Creates the missing account dir on the way.
        lock_store(&store).unwrap();
        // Idempotent for this process (several in-process store opens).
        lock_store(&store).unwrap();
        // Any other handle (standing in for a second siggy) is refused.
        assert!(matches!(
            InstanceLock::try_acquire(&lock_file_for(&store)),
            Err(LockError::Held)
        ));
    }

    #[test]
    fn store_dir_lives_under_native_root() {
        assert!(store_dir("+15551230001").starts_with(native_root()));
    }

    #[test]
    fn ensure_store_dir_creates_and_reports() {
        // Route the data dir through a temp HOME-independent location by
        // exercising the real path: create, check existence probes, delete.
        let account = "+19998887777-u9-test";
        assert!(!account_data_exists(account));
        let dir = ensure_store_dir(account).unwrap();
        assert!(dir.exists());
        // No sqlite file yet, so the account still reads as absent...
        assert!(!account_data_exists(account));
        // ...until the store file lands.
        std::fs::write(store_file(account), b"").unwrap();
        assert!(account_data_exists(account));
        std::fs::create_dir(dir.join("subdir")).unwrap();
        delete_account_data(account).unwrap();
        assert!(!account_data_exists(account));
        // Only the held lockfile survives the wipe (#685).
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(left, vec![lock_file_for(&store_file(account))]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn ensure_store_dir_is_0700() {
        use std::os::unix::fs::PermissionsExt;
        let account = "+19998887766-u9-perm-test";
        let dir = ensure_store_dir(account).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "identity-key dir must be 0700");
        delete_account_data(account).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
