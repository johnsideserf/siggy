//! Media-handling sub-state extracted from `App` (#352 pattern).
//!
//! Holds the media-related values copied from `Config` at startup: where
//! attachments are downloaded and how voice messages are played.

use std::path::PathBuf;

/// Media-handling configuration snapshot.
#[derive(Default)]
pub struct MediaState {
    /// Directory attachments are downloaded to. "Open attachment" refuses any
    /// path outside this directory (message bodies are remote-controlled text).
    pub download_dir: PathBuf,
    /// User override for the voice playback command (`audio_player` in
    /// config.toml, e.g. "mpv --no-config"). `None` or empty falls back to
    /// autodetecting a player on PATH (see `crate::audio::detect_player`).
    pub audio_player: Option<String>,
}
