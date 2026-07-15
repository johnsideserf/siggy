//! TOML configuration at the platform-specific user config dir.
//!
//! Held as [`Config`]; persists account (E.164 phone), `signal_cli_path`,
//! `download_dir`, and assorted UI preferences. Includes silent migrations:
//! the legacy `~/.config/signal-tui/` -> `~/.config/siggy/` rename and the
//! `inline_images` / `native_images` -> `image_mode` field collapse.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Notification preview detail level. Drives whether desktop notification
/// bodies show the message text, just the sender, or nothing beyond
/// "new message". Persisted in `notification_preview` config field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationPreview {
    /// Show sender + body.
    #[default]
    Full,
    /// Show sender only.
    Sender,
    /// Show "new message" only.
    Minimal,
}

impl NotificationPreview {
    /// Cycle to the next preview level (Full -> Sender -> Minimal -> Full).
    pub fn cycle(self) -> Self {
        match self {
            Self::Full => Self::Sender,
            Self::Sender => Self::Minimal,
            Self::Minimal => Self::Full,
        }
    }

    /// User-facing label for the settings overlay.
    pub fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Sender => "sender",
            Self::Minimal => "minimal",
        }
    }
}

/// Image rendering mode. Selects the protocol used to draw inline image
/// previews in the chat pane. Persisted in `image_mode` config field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageMode {
    /// Native terminal image protocols (Kitty, iTerm2, Sixel).
    Native,
    /// Unicode halfblock approximation. Universal fallback.
    #[default]
    Halfblock,
    /// Do not render images.
    None,
}

impl ImageMode {
    /// Cycle to the next image mode (Native -> Halfblock -> None -> Native).
    pub fn cycle(self) -> Self {
        match self {
            Self::Native => Self::Halfblock,
            Self::Halfblock => Self::None,
            Self::None => Self::Native,
        }
    }

    /// User-facing label for the settings overlay.
    pub fn label(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Halfblock => "halfblock",
            Self::None => "none",
        }
    }
}

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

    /// Notification preview level (full / sender / minimal).
    #[serde(default)]
    pub notification_preview: NotificationPreview,

    /// Seconds before clipboard is auto-cleared after copying (0 = disabled)
    #[serde(default = "default_clipboard_clear_seconds")]
    pub clipboard_clear_seconds: u64,

    /// Minutes of keyboard inactivity before the session auto-locks (0 = disabled)
    #[serde(default)]
    pub lock_timeout: u64,

    /// Override the message database path (for running multiple accounts side by
    /// side, each with its own config + db). Absolute paths are used as-is;
    /// relative paths resolve under the data dir. `None` keeps the default
    /// `siggy.db`, so existing single-account setups are unaffected (#260).
    #[serde(default)]
    pub db_path: Option<String>,

    /// Image display mode (native / halfblock / none). `None` here means the
    /// field was absent from the on-disk TOML and migration should fill it in
    /// from the legacy `inline_images` / `native_images` flags.
    #[serde(default)]
    pub image_mode: Option<ImageMode>,

    /// Override cell pixel width for Sixel sizing (0 = auto-detect)
    #[serde(default)]
    pub cell_pixel_width: u16,

    /// Override cell pixel height for Sixel sizing (0 = auto-detect)
    #[serde(default)]
    pub cell_pixel_height: u16,

    /// Maximum inline image attachment width in terminal cells
    #[serde(default = "default_image_max_width")]
    pub image_max_width: u32,

    /// Maximum link preview thumbnail width in terminal cells
    #[serde(default = "default_preview_image_max_width")]
    pub preview_image_max_width: u32,

    /// Maximum inline image height in terminal cell rows
    #[serde(default = "default_image_max_height")]
    pub image_max_height: u32,

    /// Maximum colors for Sixel encoding (2-256)
    #[serde(default = "default_sixel_max_colors")]
    pub sixel_max_colors: u16,

    /// Floyd-Steinberg diffusion strength for Sixel encoding (0.0-1.0)
    #[serde(default = "default_sixel_diffusion")]
    pub sixel_diffusion: f32,

    /// Command used to play voice messages inline, e.g. "mpv --no-config".
    /// Absent or empty autodetects a CLI player on PATH (mpv, ffplay,
    /// afplay, cvlc, paplay, aplay, in that order).
    #[serde(default)]
    pub audio_player: Option<String>,

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

    /// Show Signal usernames (@handle) next to 1:1 conversation names (#612)
    #[serde(default = "default_true")]
    pub show_usernames: bool,

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

    /// REMOVED (#656): this was passed to signal-cli as `--proxy`, a flag
    /// that does not exist in any signal-cli version, so it never worked.
    /// Still deserialized so startup can refuse to run with it set instead
    /// of silently connecting unproxied; never written back to disk.
    #[serde(default, skip_serializing_if = "String::is_empty")]
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

fn default_clipboard_clear_seconds() -> u64 {
    30
}

fn default_sidebar_width() -> u16 {
    22
}

fn default_image_max_width() -> u32 {
    40
}

fn default_preview_image_max_width() -> u32 {
    30
}

fn default_image_max_height() -> u32 {
    30
}

fn default_sixel_max_colors() -> u16 {
    256
}

fn default_sixel_diffusion() -> f32 {
    0.875
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
            notification_preview: NotificationPreview::Full,
            clipboard_clear_seconds: default_clipboard_clear_seconds(),
            lock_timeout: 0,
            db_path: None,
            image_mode: Some(ImageMode::Halfblock),
            cell_pixel_width: 0,
            cell_pixel_height: 0,
            image_max_width: default_image_max_width(),
            preview_image_max_width: default_preview_image_max_width(),
            image_max_height: default_image_max_height(),
            sixel_max_colors: default_sixel_max_colors(),
            sixel_diffusion: default_sixel_diffusion(),
            audio_player: None,
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
            show_usernames: true,
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
                    crate::fs_migrate::migrate_path(&old_dir, new_dir);
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
        if self.image_mode.is_some() {
            return;
        }
        self.image_mode = Some(if self.native_images {
            ImageMode::Native
        } else if self.inline_images {
            ImageMode::Halfblock
        } else {
            ImageMode::None
        });
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
    pub(crate) fn set_dir_permissions(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }

    #[cfg(not(unix))]
    pub(crate) fn set_dir_permissions(_path: &std::path::Path) {}

    /// Returns true if the account is empty and setup is needed.
    pub fn needs_setup(&self) -> bool {
        self.account.is_empty()
    }

    /// Hard startup error when `proxy` is set (#656). The old passthrough
    /// invented a `--proxy` flag signal-cli never had: the main process died
    /// at arg parsing while linking and `--check` probes ran WITHOUT the
    /// proxy, i.e. unproxied. Refusing to start is the only honest behavior;
    /// anything else silently leaks traffic a user asked to route elsewhere.
    pub fn proxy_unsupported_error(&self) -> Option<String> {
        if self.proxy.is_empty() {
            return None;
        }
        Some(format!(
            "the `proxy` config field is set (\"{}\"), but proxying is not supported by the signal-cli backend.\n\
             signal-cli has no proxy option; earlier siggy versions passed it a flag that does not exist, so\n\
             connections were never proxied. Traffic would go out directly, NOT through your proxy.\n\
             Remove the `proxy` line from config.toml to start siggy. To route signal-cli through a proxy,\n\
             use an OS-level mechanism instead (e.g. JAVA_TOOL_OPTIONS JVM proxy properties or proxychains).",
            self.proxy
        ))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_config(inline: bool, native: bool) -> Config {
        // image_mode MUST be explicitly None here -- Config::default()
        // sets it to Some(Halfblock), which would early-return the migration.
        Config {
            image_mode: None,
            inline_images: inline,
            native_images: native,
            ..Config::default()
        }
    }

    #[test]
    fn migrate_legacy_image_mode_native_wins() {
        let mut c = legacy_config(true, true);
        c.migrate_legacy_image_mode();
        assert_eq!(c.image_mode, Some(ImageMode::Native));
    }

    #[test]
    fn migrate_legacy_image_mode_halfblock_when_only_inline() {
        let mut c = legacy_config(true, false);
        c.migrate_legacy_image_mode();
        assert_eq!(c.image_mode, Some(ImageMode::Halfblock));
    }

    #[test]
    fn migrate_legacy_image_mode_none_when_both_disabled() {
        let mut c = legacy_config(false, false);
        c.migrate_legacy_image_mode();
        assert_eq!(c.image_mode, Some(ImageMode::None));
    }

    #[test]
    fn migrate_legacy_image_mode_preserves_existing() {
        let mut c = Config {
            image_mode: Some(ImageMode::Native),
            inline_images: false,
            native_images: false,
            ..Config::default()
        };
        c.migrate_legacy_image_mode();
        assert_eq!(c.image_mode, Some(ImageMode::Native));
    }

    // Filesystem migrations are tested in db::tests::migrate_path_*.
    // The Config::load wiring is exercised by integration; the rename
    // semantics live in `db::migrate_path`.

    // --- proxy guard (#656: the old `--proxy` passthrough never existed in
    // signal-cli, so a set proxy must be a hard startup error, not a silent
    // unproxied connection) ---

    #[test]
    fn proxy_unset_passes_guard() {
        assert_eq!(Config::default().proxy_unsupported_error(), None);
    }

    #[test]
    fn proxy_set_is_a_hard_error_mentioning_the_field() {
        let c = Config {
            proxy: "https://signal-proxy.example.com".to_string(),
            ..Config::default()
        };
        let msg = c.proxy_unsupported_error().expect("set proxy must error");
        assert!(msg.contains("proxy"), "error must name the config field");
        assert!(
            msg.contains("not supported"),
            "error must say proxying is unsupported"
        );
        assert!(
            msg.to_lowercase().contains("not proxied") || msg.to_lowercase().contains("directly"),
            "error must warn that traffic would go out unproxied: {msg}"
        );
    }

    #[test]
    fn empty_proxy_is_not_serialized_into_saved_configs() {
        let toml = toml::to_string_pretty(&Config::default()).unwrap();
        assert!(
            !toml.contains("proxy"),
            "fresh configs must not advertise the removed proxy field:\n{toml}"
        );
    }

    #[test]
    fn set_proxy_still_deserializes_so_the_guard_can_fire() {
        let c: Config = toml::from_str("proxy = \"https://p.example.com\"").unwrap();
        assert_eq!(c.proxy, "https://p.example.com");
    }
}
