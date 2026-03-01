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

    /// Show inline halfblock image previews in chat
    #[serde(default = "default_true")]
    pub inline_images: bool,

    /// Experimental: use native terminal image protocols (Kitty/iTerm2) over halfblock
    #[serde(default)]
    pub native_images: bool,

    /// Show delivery/read receipt status symbols on outgoing messages
    #[serde(default = "default_true")]
    pub show_receipts: bool,

    /// Use colored status symbols (vs monochrome DarkGray)
    #[serde(default = "default_true")]
    pub color_receipts: bool,

    /// Use Nerd Font glyphs for status symbols
    #[serde(default)]
    pub nerd_fonts: bool,

    /// Show verbose reaction display (usernames instead of counts)
    #[serde(default)]
    pub reaction_verbose: bool,
}

fn default_true() -> bool {
    true
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
            inline_images: true,
            native_images: false,
            show_receipts: true,
            color_receipts: true,
            nerd_fonts: false,
            reaction_verbose: false,
        }
    }
}

impl Config {
    pub fn load(path: Option<&str>) -> Result<Self> {
        let config_path = match path {
            Some(p) => PathBuf::from(p),
            None => Self::default_config_path(),
        };

        if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path)
                .with_context(|| format!("Failed to read config from {}", config_path.display()))?;
            let config: Config = toml::from_str(&contents)
                .with_context(|| format!("Failed to parse config from {}", config_path.display()))?;
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
        }
        let contents = toml::to_string_pretty(self)
            .context("Failed to serialize config")?;
        std::fs::write(&config_path, contents)
            .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
        Ok(())
    }

    /// Returns true if the account is empty and setup is needed.
    pub fn needs_setup(&self) -> bool {
        self.account.is_empty()
    }

    pub fn default_config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from(".config"))
            .join("signal-tui")
            .join("config.toml")
    }
}
