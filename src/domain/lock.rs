//! Session lock state: passphrase hashing, hash-file persistence, and the
//! state machine for the lock screen.
//!
//! The argon2 hash is stored in a separate file (`<config_dir>/lock_hash`)
//! outside `config.toml` so it never round-trips through serde and never
//! appears in TOML the user might share. The file is created on first
//! `/lock` (or `/lock-reset`) and is the only persistent state for the
//! lock feature.

use anyhow::{Context, Result};
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use rand_core::OsRng;
use std::path::{Path, PathBuf};

/// State machine for the lock screen.
#[allow(dead_code)]
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockPhase {
    #[default]
    Unlocked,
    SetPassphrase,
    LockEntry,
    ChangePassphraseOld,
    ChangePassphraseNew,
}

/// Session-lock state owned by `App`.
#[allow(dead_code)]
#[derive(Default)]
pub struct LockState {
    pub phase: LockPhase,
    /// In-progress passphrase being typed on the lock screen. Cleared on
    /// phase transition.
    pub input_buffer: String,
    /// Transient error/status text shown on the lock screen ("Incorrect
    /// passphrase", "Passphrase too short", etc.). Cleared on next keypress.
    pub error: Option<String>,
    /// Set to true when `ChangePassphraseOld` accepted the user's current
    /// passphrase, so the subsequent `ChangePassphraseNew` step knows the
    /// flow is authorised without re-prompting or re-reading the hash file.
    /// Cleared when phase returns to Unlocked.
    pub old_passphrase_verified: bool,
    /// Filesystem path to the on-disk lock hash file. Set once at App::new.
    pub hash_path: std::path::PathBuf,
}

impl LockState {
    #[allow(dead_code)]
    pub fn is_locked(&self) -> bool {
        self.phase != LockPhase::Unlocked
    }
}

/// Path to the passphrase hash file, alongside the TOML config.
#[allow(dead_code)]
pub fn lock_hash_path(config_path: &Path) -> PathBuf {
    let dir = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    dir.join("lock_hash")
}

/// Hash a passphrase with argon2 defaults. Returns the PHC string format
/// (`$argon2id$...`) suitable for `verify_passphrase`.
pub fn hash_passphrase(passphrase: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(passphrase.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("argon2 hash failed: {e}"))?;
    Ok(hash.to_string())
}

/// Verify a passphrase against a stored PHC-format hash.
pub fn verify_passphrase(passphrase: &str, stored_hash: &str) -> bool {
    let parsed = match PasswordHash::new(stored_hash) {
        Ok(p) => p,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(passphrase.as_bytes(), &parsed)
        .is_ok()
}

/// Read the stored hash from disk, if any.
pub fn load_hash(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s.trim().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("reading lock hash file"),
    }
}

/// Write a hash to disk, overwriting any existing value. Creates parent
/// directories if needed. Sets 0600 perms on Unix.
///
/// The write is atomic: contents go to `<path>.tmp` first, then rename onto
/// `<path>`. On Unix `rename` is atomic, so a crash mid-write leaves either
/// the old hash or the new one intact -- never a truncated/empty file that
/// would lock the user out forever.
pub fn save_hash(path: &Path, hash: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("creating lock hash dir")?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, hash).context("writing lock hash tmp")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).context("renaming lock hash file")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify_succeeds() {
        let h = hash_passphrase("correct horse battery staple").expect("hash");
        assert!(verify_passphrase("correct horse battery staple", &h));
    }

    #[test]
    fn hash_then_verify_rejects_wrong_passphrase() {
        let h = hash_passphrase("correct horse battery staple").expect("hash");
        assert!(!verify_passphrase("Tr0ub4dor&3", &h));
    }

    #[test]
    fn verify_rejects_malformed_hash() {
        assert!(!verify_passphrase("anything", "not-a-phc-string"));
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("lock_hash");
        let h = hash_passphrase("hello").expect("hash");
        save_hash(&path, &h).expect("save");
        let loaded = load_hash(&path).expect("load");
        assert_eq!(loaded.as_deref(), Some(h.as_str()));
    }

    #[test]
    fn load_missing_file_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("does_not_exist");
        let loaded = load_hash(&path).expect("load");
        assert!(loaded.is_none());
    }
}
