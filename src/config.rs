//! TOML configuration at the platform-specific user config dir.
//!
//! Held as [`Config`]; persists account (E.164 phone), `signal_cli_path`,
//! `download_dir`, and assorted UI preferences. Includes silent migrations:
//! the legacy `~/.config/signal-tui/` -> `~/.config/siggy/` rename and the
//! `inline_images` / `native_images` -> `image_mode` field collapse.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Phone number in E.164 format (e.g., +15551234567)
    #[serde(default)]
    pub account: String,

    /// Path to signal-cli binary
    #[serde(default = "default_signal_cli_path")]
    pub signal_cli_path: String,

    /// Directory for downloaded attachments
    #[serde(default = "default_download_dir")]
    pub download_dir: PathBuf,

    /// Terminal bell for 1:1 messages in background conversations
    #[serde(default = "default_true")]
    pub notify_direct: bool,

    /// Terminal bell for group messages in background conversations
    #[serde(default = "default_true")]
    pub notify_group: bool,

    /// OS-level desktop notifications for incoming messages
    #[serde(default)]
    pub desktop_notifications: bool,

    /// Notification preview level: "full", "sender", or "minimal"
    #[serde(default = "default_notification_preview")]
    pub notification_preview: String,

    /// Seconds before clipboard is auto-cleared after copying (0 = disabled)
    #[serde(default = "default_clipboard_clear_seconds")]
    pub clipboard_clear_seconds: u64,

    /// Image display mode: "native" (terminal protocol), "halfblock", or "none"
    #[serde(default)]
    pub image_mode: String,

    /// Override cell pixel width for Sixel sizing (0 = auto-detect)
    #[serde(default)]
    pub cell_pixel_width: u16,

    /// Override cell pixel height for Sixel sizing (0 = auto-detect)
    #[serde(default)]
    pub cell_pixel_height: u16,

    /// Legacy: show inline halfblock image previews (migrated to image_mode)
    #[serde(default = "default_true", skip_serializing)]
    pub inline_images: bool,

    /// Show link previews (title, description, thumbnail) for URLs in messages
    #[serde(default = "default_true")]
    pub show_link_previews: bool,

    /// Legacy: use native terminal image protocols (migrated to image_mode)
    #[serde(default, skip_serializing)]
    pub native_images: bool,

    /// Show date separator lines between messages from different days
    #[serde(default = "default_true")]
    pub date_separators: bool,

    /// Show delivery/read receipt status symbols on outgoing messages
    #[serde(default = "default_true")]
    pub show_receipts: bool,

    /// Use colored status symbols (vs monochrome DarkGray)
    #[serde(default = "default_true")]
    pub color_receipts: bool,

    /// Use Nerd Font glyphs for status symbols
    #[serde(default)]
    pub nerd_fonts: bool,

    /// Convert emoji to text emoticons/shortcodes in message display
    #[serde(default)]
    pub emoji_to_text: bool,

    /// Show emoji reactions on messages
    #[serde(default = "default_true")]
    pub show_reactions: bool,

    /// Show verbose reaction display (usernames instead of counts)
    #[serde(default)]
    pub reaction_verbose: bool,

    /// Send read receipts to message senders when viewing conversations
    #[serde(default = "default_true")]
    pub send_read_receipts: bool,

    /// Enable mouse support (click sidebar, scroll messages, click links)
    #[serde(default = "default_true")]
    pub mouse_enabled: bool,

    /// Display sidebar on the right side instead of left
    #[serde(default)]
    pub sidebar_on_right: bool,

    /// Sidebar width in columns (14-40, default 22)
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: u16,

    /// Color theme name (matches a built-in or custom theme)
    #[serde(default = "default_theme")]
    pub theme: String,

    /// Keybinding profile name (matches a built-in or custom profile)
    #[serde(default = "default_keybinding_profile")]
    pub keybinding_profile: String,

    /// Settings profile name (matches a built-in or custom profile)
    #[serde(default = "default_settings_profile")]
    pub settings_profile: String,

    /// Signal TLS proxy URL passed through to signal-cli (e.g., `<https://signal-proxy.example.com>`)
    #[serde(default)]
    pub proxy: String,
}

fn default_true() -> bool {
    true
}

fn default_theme() -> String {
    "Default".to_string()
}

fn default_keybinding_profile() -> String {
    "Default".to_string()
}

fn default_settings_profile() -> String {
    "Default".to_string()
}

fn default_notification_preview() -> String {
    "full".to_string()
}

fn default_clipboard_clear_seconds() -> u64 {
    30
}

fn default_sidebar_width() -> u16 {
    22
}

fn default_signal_cli_path() -> String {
    "signal-cli".to_string()
}

fn default_download_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("signal-downloads")
}

impl Default for Config {
    fn default() -> Self {
        Self {
            account: String::new(),
            signal_cli_path: default_signal_cli_path(),
            download_dir: default_download_dir(),
            notify_direct: true,
            notify_group: true,
            desktop_notifications: false,
            notification_preview: default_notification_preview(),
            clipboard_clear_seconds: default_clipboard_clear_seconds(),
            image_mode: "halfblock".to_string(),
            cell_pixel_width: 0,
            cell_pixel_height: 0,
            inline_images: true,
            show_link_previews: true,
            native_images: false,
            date_separators: true,
            show_receipts: true,
            color_receipts: true,
            nerd_fonts: false,
            emoji_to_text: false,
            show_reactions: true,
            reaction_verbose: false,
            send_read_receipts: true,
            mouse_enabled: true,
            sidebar_on_right: false,
            sidebar_width: default_sidebar_width(),
            theme: default_theme(),
            keybinding_profile: default_keybinding_profile(),
            settings_profile: default_settings_profile(),
            proxy: String::new(),
        }
    }
}

impl Config {
    pub fn load(path: Option<&str>) -> Result<Self> {
        let config_path = match path {
            Some(p) => PathBuf::from(p),
            None => {
                let new_path = Self::default_config_path();
                if let (Some(new_dir), Some(old_dir)) = (new_path.parent(), legacy_config_dir()) {
                    migrate_config_dir(&old_dir, new_dir);
                }
                new_path
            }
        };

        if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path)
                .with_context(|| format!("Failed to read config from {}", config_path.display()))?;
            let mut config: Config = toml::from_str(&contents).with_context(|| {
                format!("Failed to parse config from {}", config_path.display())
            })?;
            config.migrate_legacy_image_mode();
            Ok(config)
        } else {
            Ok(Config::default())
        }
    }

    /// Backfill `image_mode` from the legacy `inline_images` / `native_images`
    /// flags when upgrading from a config written by siggy < v1.6.0. No-op
    /// once `image_mode` is set.
    fn migrate_legacy_image_mode(&mut self) {
        if !self.image_mode.is_empty() {
            return;
        }
        self.image_mode = if self.native_images {
            "native".to_string()
        } else if self.inline_images {
            "halfblock".to_string()
        } else {
            "none".to_string()
        };
    }

    /// Serialize this config to TOML and write it to the default config path.
    pub fn save(&self) -> Result<()> {
        let config_path = Self::default_config_path();
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory {}", parent.display())
            })?;
            Self::set_dir_permissions(parent);
        }
        let contents = toml::to_string_pretty(self).context("Failed to serialize config")?;
        std::fs::write(&config_path, contents)
            .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
        Self::set_file_permissions(&config_path);
        Ok(())
    }

    /// Set restrictive permissions (0600) on a sensitive file (Unix only).
    #[cfg(unix)]
    fn set_file_permissions(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }

    #[cfg(not(unix))]
    fn set_file_permissions(_path: &std::path::Path) {}

    /// Set restrictive permissions (0700) on a sensitive directory (Unix only).
    #[cfg(unix)]
    fn set_dir_permissions(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }

    #[cfg(not(unix))]
    fn set_dir_permissions(_path: &std::path::Path) {}

    /// Returns true if the account is empty and setup is needed.
    pub fn needs_setup(&self) -> bool {
        self.account.is_empty()
    }

    pub fn default_config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from(".config"))
            .join("siggy")
            .join("config.toml")
    }
}

/// Path to the legacy `signal-tui` config directory, if `dirs::config_dir()`
/// is resolvable on this platform.
fn legacy_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("signal-tui"))
}

/// Move the legacy config directory into place at `new_dir` if and only if
/// `new_dir` does not already exist and `old_dir` does. Silently no-ops on
/// rename errors -- the user can always fall back to running the setup wizard.
///
/// The previous implementation pre-created `new_dir` before calling rename,
/// which works on POSIX (rename onto an empty dir is allowed) but fails on
/// Windows (`std::fs::rename` returns an error if the target exists). The
/// fix is to only ensure `new_dir`'s parent exists, leaving `new_dir` itself
/// absent so rename can create it atomically.
pub(crate) fn migrate_config_dir(old_dir: &Path, new_dir: &Path) {
    if new_dir.exists() || !old_dir.exists() {
        return;
    }
    if let Some(parent) = new_dir.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::rename(old_dir, new_dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_config(inline: bool, native: bool) -> Config {
        // image_mode MUST be explicitly empty here — Config::default()
        // sets it to "halfblock", which would early-return the migration.
        Config {
            image_mode: String::new(),
            inline_images: inline,
            native_images: native,
            ..Config::default()
        }
    }

    #[test]
    fn migrate_legacy_image_mode_native_wins() {
        let mut c = legacy_config(true, true);
        c.migrate_legacy_image_mode();
        assert_eq!(c.image_mode, "native");
    }

    #[test]
    fn migrate_legacy_image_mode_halfblock_when_only_inline() {
        let mut c = legacy_config(true, false);
        c.migrate_legacy_image_mode();
        assert_eq!(c.image_mode, "halfblock");
    }

    #[test]
    fn migrate_legacy_image_mode_none_when_both_disabled() {
        let mut c = legacy_config(false, false);
        c.migrate_legacy_image_mode();
        assert_eq!(c.image_mode, "none");
    }

    #[test]
    fn migrate_legacy_image_mode_preserves_existing() {
        let mut c = Config {
            image_mode: "native".to_string(),
            inline_images: false,
            native_images: false,
            ..Config::default()
        };
        c.migrate_legacy_image_mode();
        assert_eq!(c.image_mode, "native");
    }

    // --- migrate_config_dir (filesystem) ---

    #[test]
    fn migrate_config_dir_renames_when_only_old_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("signal-tui");
        let new = tmp.path().join("siggy");
        std::fs::create_dir(&old).unwrap();
        std::fs::write(old.join("config.toml"), b"account = \"+1\"").unwrap();

        migrate_config_dir(&old, &new);

        assert!(!old.exists());
        assert!(new.exists());
        assert_eq!(
            std::fs::read_to_string(new.join("config.toml")).unwrap(),
            "account = \"+1\""
        );
    }

    #[test]
    fn migrate_config_dir_noops_when_new_already_exists() {
        // Pre-fix: the previous implementation pre-created `new_dir` then
        // tried to rename onto it, which fails on Windows. The fix removed
        // the pre-creation and the no-op guard ensures nothing happens
        // when the user already has a fresh siggy config dir.
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("signal-tui");
        let new = tmp.path().join("siggy");
        std::fs::create_dir(&old).unwrap();
        std::fs::write(old.join("config.toml"), b"old").unwrap();
        std::fs::create_dir(&new).unwrap();
        std::fs::write(new.join("config.toml"), b"new").unwrap();

        migrate_config_dir(&old, &new);

        assert_eq!(std::fs::read(old.join("config.toml")).unwrap(), b"old");
        assert_eq!(std::fs::read(new.join("config.toml")).unwrap(), b"new");
    }

    #[test]
    fn migrate_config_dir_noops_when_old_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("signal-tui");
        let new = tmp.path().join("siggy");

        migrate_config_dir(&old, &new);

        assert!(!old.exists());
        assert!(!new.exists());
    }

    #[test]
    fn migrate_config_dir_creates_missing_parent() {
        // dirs::config_dir() should always exist on a real machine, but
        // verify we don't choke when it doesn't (e.g., a fresh tempdir
        // pretending to be `~/.config/`). The new path's parent gets
        // created so rename can succeed.
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("signal-tui");
        let new = tmp.path().join("nested").join("subdir").join("siggy");
        std::fs::create_dir(&old).unwrap();

        migrate_config_dir(&old, &new);

        assert!(!old.exists());
        assert!(new.exists());
    }
}
