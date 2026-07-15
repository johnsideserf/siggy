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

use std::path::PathBuf;

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

/// Whether any native store data exists for this account. The native twin
/// of `setup::account_exists_locally`'s signal-cli accounts.json probe,
/// consulted by `--reset-account` and the relink guard (#603).
pub fn account_data_exists(account: &str) -> bool {
    store_file(account).exists()
}

/// Delete the account's entire native store directory. The native twin of
/// signal-cli's `deleteLocalAccountData`; purely local, never touches the
/// Signal servers.
pub fn delete_account_data(account: &str) -> Result<()> {
    let dir = store_dir(account);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("Failed to delete native store dir {}", dir.display()))?;
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
        delete_account_data(account).unwrap();
        assert!(!dir.exists());
        assert!(!account_data_exists(account));
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
    }
}
