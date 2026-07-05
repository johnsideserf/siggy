//! Media-handling sub-state extracted from `App` (#352 pattern).
//!
//! Holds the media-related values copied from `Config` at startup (where
//! attachments are downloaded, how voice messages are played) plus the
//! outgoing link-preview draft produced by `/preview` (#267).

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::signal::types::LinkPreview;

/// A voice message currently playing through the spawned CLI player (#618).
pub struct PlayingVoice {
    /// The player child process; killed to stop playback early.
    pub child: std::process::Child,
    /// When playback started, for the elapsed side of the progress line.
    pub started: Instant,
    /// Total length when the file was parseable (Ogg Opus only).
    pub duration: Option<Duration>,
    /// Display label (usually the attachment filename).
    pub label: String,
    /// Canonical path being played, so a second `o` on the same message
    /// stops instead of double-playing.
    pub path: PathBuf,
}

/// Media-handling configuration snapshot and preview-draft state.
#[derive(Default)]
pub struct MediaState {
    /// Directory attachments are downloaded to. "Open attachment" refuses any
    /// path outside this directory (message bodies are remote-controlled text).
    pub download_dir: PathBuf,
    /// User override for the voice playback command (`audio_player` in
    /// config.toml, e.g. "mpv --no-config"). `None` or empty falls back to
    /// autodetecting a player on PATH (see `crate::audio::detect_player`).
    pub audio_player: Option<String>,
    /// Fetched link preview waiting to attach to the next outgoing message
    /// in place (#267). Set when a `/preview` fetch completes; consumed by
    /// the next send; cleared by `/preview` with no argument.
    pub pending_preview: Option<LinkPreview>,
    /// Receiver for an in-flight `/preview` fetch (`Some` while fetching).
    pub preview_rx: Option<mpsc::Receiver<Result<LinkPreview, String>>>,
    /// The voice message currently playing, if any (#618).
    pub playing: Option<PlayingVoice>,
}
