use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

    /// Color theme name (matches a built-in or custom theme)
    #[serde(default = "default_theme")]
    pub theme: String,

    /// Keybinding profile name (matches a built-in or custom profile)
    #[serde(default = "default_keybinding_profile")]
    pub keybinding_profile: String,

    /// Settings profile name (matches a built-in or custom profile)
    #[serde(default = "default_settings_profile")]
    pub settings_profile: String,

    /// Signal TLS proxy URL passed through to signal-cli (e.g., "https://signal-proxy.example.com")
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
                // Auto-migrate from old "signal-tui" config directory
                if !new_path.exists() {
                    let old_path = dirs::config_dir()
                        .unwrap_or_else(|| PathBuf::from(".config"))
                        .join("signal-tui")
                        .join("config.toml");
                    if old_path.exists() {
                        if let Some(parent) = new_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::rename(old_path.parent().unwrap(), new_path.parent().unwrap());
                    }
                }
                new_path
            }
        };

        if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path)
                .with_context(|| format!("Failed to read config from {}", config_path.display()))?;
            let mut config: Config = toml::from_str(&contents)
                .with_context(|| format!("Failed to parse config from {}", config_path.display()))?;
            // Migrate legacy inline_images/native_images to image_mode
            if config.image_mode.is_empty() {
                config.image_mode = if config.native_images {
                    "native".to_string()
                } else if config.inline_images {
                    "halfblock".to_string()
                } else {
                    "none".to_string()
                };
            }
            Ok(config)
        } else {
            Ok(Config::default())
        }
    }

    /// Serialize this config to TOML and write it to the default config path.
    pub fn save(&self) -> Result<()> {
        let config_path = Self::default_config_path();
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create config directory {}", parent.display()))?;
            Self::set_dir_permissions(parent);
        }
        let contents = toml::to_string_pretty(self)
            .context("Failed to serialize config")?;
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
