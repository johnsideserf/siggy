//! Inline voice-message playback (#199).
//!
//! Signal voice notes arrive as audio attachments (usually `audio/ogg`/opus).
//! Rather than take on a Rust audio-decoding dependency, siggy shells out to
//! whatever command-line player is already installed, detected the same way it
//! detects signal-cli. Playback is fire-and-forget: the player runs detached
//! and siggy does not track progress (a live progress indicator is a follow-up).
//! When no player is found, callers fall back to opening the file in the OS
//! default app.

use std::path::Path;
use std::process::{Child, Command, Stdio};

/// Known headless CLI players, in preference order, with the args that make
/// them play one file and exit without opening a window. `mpv`/`ffplay` (and
/// `cvlc`) decode Signal's opus/ogg notes; `afplay` is macOS-native; `paplay`/
/// `aplay` are last because they only reliably handle PCM/WAV.
const PLAYERS: &[(&str, &[&str])] = &[
    ("mpv", &["--no-video", "--really-quiet"]),
    ("ffplay", &["-nodisp", "-autoexit", "-loglevel", "quiet"]),
    ("afplay", &[]),
    ("cvlc", &["--play-and-exit", "--intf", "dummy"]),
    ("paplay", &[]),
    ("aplay", &["-q"]),
];

/// The fixed arguments for a known player name, or `None` if unknown. Pure, so
/// the player table can be unit-tested without touching the filesystem.
fn player_args(name: &str) -> Option<&'static [&'static str]> {
    PLAYERS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, args)| *args)
}

/// Whether `exe` is found on `PATH` (honouring Windows executable extensions).
fn on_path(exe: &str) -> bool {
    let exts: &[&str] = if cfg!(windows) {
        &["", ".exe", ".bat", ".cmd", ".com"]
    } else {
        &[""]
    };
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        exts.iter()
            .any(|ext| dir.join(format!("{exe}{ext}")).is_file())
    })
}

/// Resolve the player command to use as `[executable, args...]`.
///
/// A non-empty `override_cmd` (the `audio_player` config option) wins and is
/// used as-is, so a user can point at any binary. Otherwise the first
/// [`PLAYERS`] entry found on `PATH` is chosen. Returns `None` when nothing is
/// available, so the caller can fall back to opening the file externally.
pub fn detect_player(override_cmd: Option<&str>) -> Option<Vec<String>> {
    if let Some(cmd) = override_cmd.map(str::trim).filter(|c| !c.is_empty()) {
        let mut parts = cmd.split_whitespace().map(str::to_string);
        let exe = parts.next()?;
        let mut command = vec![exe];
        command.extend(parts);
        return Some(command);
    }
    for (name, args) in PLAYERS {
        if on_path(name) {
            let mut command = vec![(*name).to_string()];
            command.extend(args.iter().map(|s| (*s).to_string()));
            return Some(command);
        }
    }
    None
}

/// Spawn `player` (from [`detect_player`]) on `path`, detached, with stdio
/// silenced so it never disturbs the TUI. Returns the child handle; the caller
/// may drop it (fire-and-forget) or keep it to stop playback later.
pub fn play(path: &Path, player: &[String]) -> std::io::Result<Child> {
    let (exe, args) = player.split_first().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty player command")
    })?;
    Command::new(exe)
        .args(args)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// Whether a content type is an audio attachment (a voice note or audio file).
pub fn is_audio(content_type: &str) -> bool {
    content_type.starts_with("audio/")
}

/// Whether a file path looks like a playable audio file by extension. Used to
/// decide, at open time, whether to play inline rather than hand off to the OS.
pub fn is_audio_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("mp3" | "ogg" | "oga" | "m4a" | "wav" | "flac" | "opus" | "aac")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_audio_matches_audio_types_only() {
        assert!(is_audio("audio/ogg"));
        assert!(is_audio("audio/aac"));
        assert!(!is_audio("image/png"));
        assert!(!is_audio("application/octet-stream"));
        assert!(!is_audio(""));
    }

    #[test]
    fn player_args_known_and_unknown() {
        assert_eq!(
            player_args("ffplay"),
            Some(["-nodisp", "-autoexit", "-loglevel", "quiet"].as_slice())
        );
        assert_eq!(player_args("afplay"), Some([].as_slice()));
        assert_eq!(player_args("not-a-player"), None);
    }

    #[test]
    fn detect_player_honours_override_verbatim() {
        // Single binary.
        assert_eq!(
            detect_player(Some("/usr/bin/myplayer")),
            Some(vec!["/usr/bin/myplayer".to_string()])
        );
        // Override with args is split on whitespace.
        assert_eq!(
            detect_player(Some("mpv --no-config")),
            Some(vec!["mpv".to_string(), "--no-config".to_string()])
        );
        // Blank / whitespace override is ignored: behaves identically to no
        // override (both fall through to PATH autodetect).
        assert_eq!(detect_player(Some("   ")), detect_player(None));
    }
}
