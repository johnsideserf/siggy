//! Generic helpers used by other parsers: attachments, link previews,
//! mentions, text styles, expiration formatting, MIME mapping.

use crate::signal::types::*;

/// Relative path of a sticker inside signal-cli's data dir, or `None` when
/// the pack id is not plain lowercase hex. Pack ids come off the wire, so
/// the hex check doubles as a path-traversal guard (#610).
pub(super) fn sticker_relative_path(pack_id: &str, sticker_id: i64) -> Option<std::path::PathBuf> {
    if pack_id.is_empty()
        || sticker_id < 0
        || !pack_id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return None;
    }
    Some(
        std::path::PathBuf::from("signal-cli")
            .join("stickers")
            .join(pack_id)
            .join(sticker_id.to_string()),
    )
}

/// Absolute path of a locally cached sticker file, or `None` when it is not
/// on disk. signal-cli caches sticker packs as WebP files under
/// `{data_dir}/signal-cli/stickers/<packId>/<stickerId>`; like
/// `find_signal_cli_attachment`, both the platform-native data dir and the
/// POSIX-style `~/.local/share` are checked (plus `XDG_DATA_HOME`).
pub(super) fn sticker_local_path(pack_id: &str, sticker_id: i64) -> Option<String> {
    let relative = sticker_relative_path(pack_id, sticker_id)?;
    let mut bases = Vec::new();
    if let Some(x) = std::env::var_os("XDG_DATA_HOME").filter(|x| !x.is_empty()) {
        bases.push(std::path::PathBuf::from(x));
    }
    if let Some(data_dir) = dirs::data_dir() {
        bases.push(data_dir);
    }
    if let Some(home) = dirs::home_dir() {
        bases.push(home.join(".local").join("share"));
    }
    bases
        .into_iter()
        .map(|base| base.join(&relative))
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
}

pub(super) fn parse_attachment(
    value: &serde_json::Value,
    download_dir: &std::path::Path,
) -> Option<Attachment> {
    let id = value.get("id").and_then(|v| v.as_str())?.to_string();
    let content_type = value
        .get("contentType")
        .and_then(|v| v.as_str())
        .unwrap_or("application/octet-stream")
        .to_string();
    let filename = value
        .get("filename")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Generate a filename if signal-cli didn't provide one
    let mut effective_name = filename.clone().unwrap_or_else(|| {
        let ext = mime_to_ext(&content_type);
        // Use last 8 chars of attachment ID for uniqueness
        let short_id = if id.len() > 8 {
            &id[id.len() - 8..]
        } else {
            &id
        };
        format!("{short_id}.{ext}")
    });

    // Strip doubled extension (e.g. "photo.jpg.jpg" → "photo.jpg")
    if let Some(dot_pos) = effective_name.rfind('.') {
        let ext = &effective_name[dot_pos..]; // e.g. ".jpg"
        let base = &effective_name[..dot_pos];
        if base.ends_with(ext) {
            effective_name = base.to_string();
        }
    }

    // Sanitize filename: strip path separators and traversal sequences
    // to prevent writes outside the download directory.
    effective_name = effective_name.replace(['/', '\\'], "_").replace("..", "_");
    if effective_name.is_empty() {
        let short_id = if id.len() > 8 {
            &id[id.len() - 8..]
        } else {
            &id
        };
        effective_name = format!("{short_id}.bin");
    }

    let dest = download_dir.join(&effective_name);

    // Defense-in-depth: verify resolved path stays within download directory.
    let canon_dir = download_dir
        .canonicalize()
        .unwrap_or_else(|_| download_dir.to_path_buf());
    let canon_dest = dest
        .canonicalize()
        .unwrap_or_else(|_| canon_dir.join(&effective_name));
    if !canon_dest.starts_with(&canon_dir) {
        return None;
    }

    // Try to find the source file: explicit "file" field, or signal-cli's attachment dir
    let local_path = if dest.exists() {
        // Already copied previously
        Some(dest.to_string_lossy().to_string())
    } else {
        // Find source: "file" field from JSON, or signal-cli's attachment storage
        let src = value
            .get("file")
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from)
            .or_else(|| find_signal_cli_attachment(&id, &content_type));

        if let Some(src) = src.filter(|p| p.exists()) {
            let _ = std::fs::create_dir_all(download_dir);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ =
                    std::fs::set_permissions(download_dir, std::fs::Permissions::from_mode(0o700));
            }
            match std::fs::copy(&src, &dest) {
                Ok(_) => Some(dest.to_string_lossy().to_string()),
                Err(_) => Some(src.to_string_lossy().to_string()),
            }
        } else {
            None
        }
    };

    Some(Attachment {
        id,
        content_type,
        filename: Some(effective_name),
        local_path,
    })
}

/// Parse link previews from a dataMessage / sentMessage object.
pub(super) fn parse_link_previews(
    data: &serde_json::Value,
    download_dir: &std::path::Path,
) -> Vec<LinkPreview> {
    // signal-cli uses "previews" (plural) in some versions, "preview" in others
    let arr = data
        .get("previews")
        .or_else(|| data.get("preview"))
        .and_then(|v| v.as_array());
    let Some(arr) = arr else { return Vec::new() };
    arr.iter()
        .filter_map(|p| {
            let url = p.get("url").and_then(|v| v.as_str())?.to_string();
            let title = p
                .get("title")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let description = p
                .get("description")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let image_path = p
                .get("image")
                .and_then(|img| parse_attachment(img, download_dir))
                .and_then(|att| att.local_path);
            Some(LinkPreview {
                url,
                title,
                description,
                image_path,
            })
        })
        .collect()
}

/// Look for an attachment file in signal-cli's data directory by attachment ID.
/// signal-cli stores attachments as `{data_dir}/attachments/{id}.{ext}`.
///
/// Checks multiple locations since signal-cli may use platform-native data dirs
/// or POSIX-style ~/.local/share depending on how it was installed.
fn find_signal_cli_attachment(id: &str, content_type: &str) -> Option<std::path::PathBuf> {
    let mut candidates = Vec::new();
    if let Some(data_dir) = dirs::data_dir() {
        candidates.push(data_dir.join("signal-cli").join("attachments"));
    }
    // Also check ~/.local/share (POSIX-style, common on MSYS/WSL)
    if let Some(home) = dirs::home_dir() {
        candidates.push(
            home.join(".local")
                .join("share")
                .join("signal-cli")
                .join("attachments"),
        );
    }

    let ext = mime_to_ext(content_type);

    for attachments_dir in &candidates {
        // Try with MIME-derived extension first
        let with_ext = attachments_dir.join(format!("{id}.{ext}"));
        if with_ext.exists() {
            return Some(with_ext);
        }

        // Scan directory for files matching the attachment ID
        if let Ok(entries) = std::fs::read_dir(attachments_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(id) {
                    return Some(entry.path());
                }
            }
        }
    }

    None
}

/// Map common MIME types to file extensions
fn mime_to_ext(mime: &str) -> &str {
    match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "video/mp4" => "mp4",
        "video/quicktime" => "mov",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "audio/aac" => "aac",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        _ => "bin",
    }
}

/// Format an expiration timer value as a human-readable string.
/// `pub(crate)`: the native receive mapper (#642 U11) emits the same
/// system-message copy so timer changes render identically per engine.
pub(crate) fn format_expiration(seconds: i64) -> String {
    if seconds == 0 {
        return "Disappearing messages disabled".to_string();
    }
    let (n, unit) = if seconds < 60 {
        (seconds, "second")
    } else if seconds < 3600 {
        (seconds / 60, "minute")
    } else if seconds < 86400 {
        (seconds / 3600, "hour")
    } else if seconds < 604800 {
        (seconds / 86400, "day")
    } else {
        (seconds / 604800, "week")
    };
    let plural = if n == 1 { "" } else { "s" };
    format!("Disappearing messages set to {n} {unit}{plural}")
}

/// Parse mentions from a data/sync message.
/// signal-cli uses "mentions" array with "uuid" field; fall back to legacy "bodyRanges" with "mentionUuid".
pub(super) fn parse_mentions(data: &serde_json::Value) -> Vec<Mention> {
    let arr = data
        .get("mentions")
        .and_then(|v| v.as_array())
        .or_else(|| data.get("bodyRanges").and_then(|v| v.as_array()));

    arr.map(|items| {
        items
            .iter()
            .filter_map(|r| {
                let start = r.get("start").and_then(|v| v.as_u64())? as usize;
                let length = r.get("length").and_then(|v| v.as_u64())? as usize;
                let uuid = r
                    .get("uuid")
                    .or_else(|| r.get("mentionUuid"))
                    .and_then(|v| v.as_str())?
                    .to_string();
                Some(Mention {
                    start,
                    length,
                    uuid,
                })
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Parse text styles from a data message's textStyles array (or bodyRanges style entries).
pub(super) fn parse_text_styles(data: &serde_json::Value) -> Vec<TextStyle> {
    // Try textStyles array first, then fall back to bodyRanges entries with "style" field
    let arr = data
        .get("textStyles")
        .and_then(|v| v.as_array())
        .or_else(|| data.get("bodyRanges").and_then(|v| v.as_array()));

    arr.map(|items| {
        items
            .iter()
            .filter_map(|r| {
                let start = r.get("start").and_then(|v| v.as_u64())? as usize;
                let length = r.get("length").and_then(|v| v.as_u64())? as usize;
                let style_str = r.get("style").and_then(|v| v.as_str())?;
                let style = match style_str {
                    "BOLD" => StyleType::Bold,
                    "ITALIC" => StyleType::Italic,
                    "STRIKETHROUGH" => StyleType::Strikethrough,
                    "MONOSPACE" => StyleType::Monospace,
                    "SPOILER" => StyleType::Spoiler,
                    _ => return None,
                };
                Some(TextStyle {
                    start,
                    length,
                    style,
                })
            })
            .collect()
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticker_relative_path_builds_hex_paths_only() {
        assert_eq!(
            sticker_relative_path("3044281a51307306e5442f2e9070953a", 5),
            Some(std::path::PathBuf::from(
                "signal-cli/stickers/3044281a51307306e5442f2e9070953a/5"
            ))
        );
        // Wire-controlled pack ids must not escape the stickers directory.
        assert_eq!(sticker_relative_path("../../../etc", 0), None);
        assert_eq!(sticker_relative_path("abc/def", 0), None);
        assert_eq!(sticker_relative_path("ABCDEF", 0), None); // uppercase
        assert_eq!(sticker_relative_path("", 0), None);
        assert_eq!(sticker_relative_path("abcdef", -1), None);
    }
}
