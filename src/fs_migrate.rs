//! Filesystem path migration helper used by the
//! `signal-tui` -> `siggy` rename (#350).
//!
//! Lives at the top level (rather than inside `db.rs` or `config.rs`)
//! so both the binary and the lib-surface (which `config.rs` is part
//! of, for fuzz harnesses) can call into the same implementation.

use std::path::Path;

/// Move `old` to `new` if and only if `new` does not exist and `old` does.
/// Used by the `signal-tui` -> `siggy` filesystem migrations for the config
/// dir, data dir, and database filename (#350). Silently no-ops on rename
/// errors -- callers fall back to creating fresh state at `new`.
///
/// Creates `new`'s *parent* (not `new` itself) before the rename so the
/// operation can succeed on a first launch where the destination's parent
/// directory may not exist yet. Pre-creating `new` would break Windows,
/// where `std::fs::rename` rejects an existing target dir even when empty;
/// POSIX accepts it. By leaving `new` absent we get atomic rename on both
/// platforms.
pub fn migrate_path(old: &Path, new: &Path) {
    if new.exists() || !old.exists() {
        return;
    }
    if let Some(parent) = new.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::rename(old, new);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_path_renames_directory_with_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("signal-tui");
        let new = tmp.path().join("siggy");

        std::fs::create_dir(&old).unwrap();
        std::fs::write(old.join("siggy.db"), b"payload").unwrap();

        migrate_path(&old, &new);

        assert!(!old.exists(), "old dir should be gone after rename");
        assert!(new.exists(), "new dir should exist after rename");
        assert_eq!(
            std::fs::read(new.join("siggy.db")).unwrap(),
            b"payload",
            "directory contents should survive the rename"
        );
    }

    #[test]
    fn migrate_path_renames_single_file() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("signal-tui.db");
        let new = tmp.path().join("siggy.db");
        std::fs::write(&old, b"sqlite-bytes").unwrap();

        migrate_path(&old, &new);

        assert!(!old.exists());
        assert!(new.exists());
        assert_eq!(std::fs::read(&new).unwrap(), b"sqlite-bytes");
    }

    #[test]
    fn migrate_path_noops_when_new_dir_already_exists() {
        // Reproduces the Windows-rename-over-existing-dir failure mode:
        // we should not even attempt the rename when the new dir is present.
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("signal-tui");
        let new = tmp.path().join("siggy");
        std::fs::create_dir(&old).unwrap();
        std::fs::write(old.join("marker"), b"old").unwrap();
        std::fs::create_dir(&new).unwrap();
        std::fs::write(new.join("marker"), b"new").unwrap();

        migrate_path(&old, &new);

        assert!(old.exists(), "old dir should be left alone");
        assert!(new.exists(), "new dir should be left alone");
        assert_eq!(std::fs::read(new.join("marker")).unwrap(), b"new");
    }

    #[test]
    fn migrate_path_noops_when_new_file_already_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("signal-tui.db");
        let new = tmp.path().join("siggy.db");
        std::fs::write(&old, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();

        migrate_path(&old, &new);

        assert_eq!(std::fs::read(&old).unwrap(), b"old");
        assert_eq!(std::fs::read(&new).unwrap(), b"new");
    }

    #[test]
    fn migrate_path_noops_when_old_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("signal-tui");
        let new = tmp.path().join("siggy");

        migrate_path(&old, &new);

        assert!(!old.exists());
        assert!(!new.exists());
    }

    #[test]
    fn migrate_path_creates_missing_parent_directory() {
        // dirs::config_dir() should always exist on a real machine, but
        // verify we don't choke when the destination's parent is missing
        // (a fresh-machine edge case). The parent gets created so rename
        // can succeed.
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("signal-tui");
        let new = tmp.path().join("nested").join("subdir").join("siggy");
        std::fs::create_dir(&old).unwrap();

        migrate_path(&old, &new);

        assert!(!old.exists());
        assert!(new.exists());
    }
}
