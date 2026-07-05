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

/// Duration of an Ogg Opus file (Signal voice notes), or `None` for other
/// formats or malformed files (#618). No decoder needed: Opus granule
/// positions count 48 kHz samples, so duration = (last page granule -
/// OpusHead pre-skip) / 48000.
pub fn ogg_opus_duration(path: &Path) -> Option<std::time::Duration> {
    // Voice notes are small (a few hundred KB per minute); cap the read so a
    // mislabeled giant file cannot balloon memory.
    const MAX_READ: u64 = 16 * 1024 * 1024;
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_READ {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    ogg_opus_duration_from_bytes(&data)
}

/// Parse the duration out of in-memory Ogg Opus data. Split from the file
/// read so it can be unit-tested on synthetic pages.
fn ogg_opus_duration_from_bytes(data: &[u8]) -> Option<std::time::Duration> {
    const SAMPLE_RATE: u64 = 48_000;
    let mut pos = 0usize;
    let mut pre_skip: Option<u64> = None;
    let mut last_granule: Option<u64> = None;

    while pos + 27 <= data.len() {
        if &data[pos..pos + 4] != b"OggS" {
            break;
        }
        let granule = u64::from_le_bytes(data[pos + 6..pos + 14].try_into().ok()?);
        let n_segments = data[pos + 26] as usize;
        let seg_table_end = pos + 27 + n_segments;
        if seg_table_end > data.len() {
            break;
        }
        let payload_len: usize = data[pos + 27..seg_table_end]
            .iter()
            .map(|&b| b as usize)
            .sum();
        let payload_end = seg_table_end + payload_len;
        if payload_end > data.len() {
            break;
        }

        if pre_skip.is_none() {
            // The first page's payload must be the OpusHead identification
            // header; anything else is not an Ogg Opus stream.
            let payload = &data[seg_table_end..payload_end];
            if !payload.starts_with(b"OpusHead") || payload.len() < 12 {
                return None;
            }
            pre_skip = Some(u64::from(u16::from_le_bytes([payload[10], payload[11]])));
        }

        // Granule of -1 marks a page where no packet ends; skip those.
        if granule != u64::MAX {
            last_granule = Some(granule);
        }
        pos = payload_end;
    }

    let samples = last_granule?.saturating_sub(pre_skip?);
    Some(std::time::Duration::from_millis(
        samples * 1000 / SAMPLE_RATE,
    ))
}

/// Format a duration as m:ss for the voice label / progress line.
pub fn format_mmss(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
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

    /// Build one Ogg page: header + segment table + payload.
    fn ogg_page(granule: u64, payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() < 255 * 255);
        let mut segments: Vec<u8> = Vec::new();
        let mut remaining = payload.len();
        loop {
            if remaining >= 255 {
                segments.push(255);
                remaining -= 255;
            } else {
                segments.push(remaining as u8);
                break;
            }
        }
        let mut page = Vec::new();
        page.extend_from_slice(b"OggS");
        page.extend_from_slice(&[0, 0]); // version, header type
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&[0; 12]); // serial, sequence, checksum
        page.push(segments.len() as u8);
        page.extend_from_slice(&segments);
        page.extend_from_slice(payload);
        page
    }

    /// A minimal OpusHead payload with the given pre-skip.
    fn opus_head(pre_skip: u16) -> Vec<u8> {
        let mut head = b"OpusHead".to_vec();
        head.push(1); // version
        head.push(1); // channels
        head.extend_from_slice(&pre_skip.to_le_bytes());
        head.extend_from_slice(&[0; 7]); // rate + gain + mapping
        head
    }

    #[test]
    fn ogg_opus_duration_reads_last_granule_minus_preskip() {
        // 48_312 samples - 312 pre-skip = 48_000 samples = exactly 1s.
        let mut data = ogg_page(0, &opus_head(312));
        data.extend(ogg_page(24_000, &[0u8; 10]));
        data.extend(ogg_page(48_312, &[0u8; 10]));
        assert_eq!(
            ogg_opus_duration_from_bytes(&data),
            Some(std::time::Duration::from_secs(1))
        );
    }

    #[test]
    fn ogg_opus_duration_skips_no_packet_pages_and_rejects_non_opus() {
        // A granule of u64::MAX means "no packet ends here" and is skipped.
        let mut data = ogg_page(0, &opus_head(0));
        data.extend(ogg_page(96_000, &[0u8; 4]));
        data.extend(ogg_page(u64::MAX, &[0u8; 4]));
        assert_eq!(
            ogg_opus_duration_from_bytes(&data),
            Some(std::time::Duration::from_secs(2))
        );

        // Not opus: first payload is not OpusHead.
        let vorbis = ogg_page(48_000, b"vorbis-ish");
        assert_eq!(ogg_opus_duration_from_bytes(&vorbis), None);
        // Garbage and empty input.
        assert_eq!(ogg_opus_duration_from_bytes(b"not an ogg"), None);
        assert_eq!(ogg_opus_duration_from_bytes(&[]), None);
    }

    #[test]
    fn format_mmss_formats() {
        use std::time::Duration;
        assert_eq!(format_mmss(Duration::from_secs(0)), "0:00");
        assert_eq!(format_mmss(Duration::from_secs(5)), "0:05");
        assert_eq!(format_mmss(Duration::from_secs(72)), "1:12");
        assert_eq!(format_mmss(Duration::from_secs(3600)), "60:00");
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
