//! Outgoing link-preview generation (#267).
//!
//! Signal previews are sender-generated: the sending client fetches the
//! page, extracts Open Graph metadata, and attaches it to the message.
//! siggy only does this on the explicit `/preview <url>` command (never
//! automatically while typing), because fetching a URL reveals your IP to
//! that site before you even send the message.
//!
//! The fetch runs on a background thread ([`spawn_fetch`]) and reports back
//! over a channel drained by the main loop. Parsing (`parse_og`) is pure and
//! unit-tested; only the network calls live in `fetch_preview`.

use std::sync::mpsc;
use std::time::Duration;

use crate::signal::types::LinkPreview;

/// Cap on fetched HTML (bytes). Pages bigger than this are truncated; og:
/// tags live in `<head>`, so truncation loses nothing in practice.
const MAX_HTML_BYTES: u64 = 512 * 1024;
/// Cap on the downloaded og:image (bytes).
const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;
/// Whole-fetch timeout per request.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Spawn a background thread fetching a preview for `url`. The result
/// arrives on the returned receiver; poll it with `try_recv` from the main
/// loop (`App::poll_preview_fetch`).
pub fn spawn_fetch(url: String) -> mpsc::Receiver<Result<LinkPreview, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(fetch_preview(&url));
    });
    rx
}

/// Fetch `url`, extract Open Graph metadata, and download the og:image (if
/// any) to a temp file signal-cli can attach. Blocking; run off the main
/// thread.
fn fetch_preview(url: &str) -> Result<LinkPreview, String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("only http(s) URLs can be previewed".to_string());
    }

    let agent = agent();
    let mut response = agent
        .get(url)
        .header("User-Agent", concat!("siggy/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "text/html")
        .call()
        .map_err(|e| format!("fetch failed: {e}"))?;
    let html = response
        .body_mut()
        .with_config()
        .limit(MAX_HTML_BYTES)
        .read_to_string()
        .map_err(|e| format!("read failed: {e}"))?;

    let (title, description, image_url) = parse_og(&html);
    if title.is_none() && description.is_none() {
        return Err("page has no title or description to preview".to_string());
    }

    let image_path = image_url
        .and_then(|img| resolve_url(url, &img))
        .and_then(|img| fetch_image(&agent, &img));

    Ok(LinkPreview {
        url: url.to_string(),
        title,
        description,
        image_path,
    })
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .build()
        .into()
}

/// Download the og:image to a temp file, returning its path. Any failure
/// (bad content type, too large, IO) degrades to a text-only preview.
fn fetch_image(agent: &ureq::Agent, image_url: &str) -> Option<String> {
    let mut response = agent
        .get(image_url)
        .header("User-Agent", concat!("siggy/", env!("CARGO_PKG_VERSION")))
        .call()
        .ok()?;
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let ext = match content_type.split(';').next().unwrap_or("").trim() {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => return None,
    };
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_IMAGE_BYTES)
        .read_to_vec()
        .ok()?;
    if bytes.is_empty() {
        return None;
    }
    let path = std::env::temp_dir().join(format!(
        "siggy-preview-{}.{ext}",
        chrono::Utc::now().timestamp_millis()
    ));
    std::fs::write(&path, &bytes).ok()?;
    Some(path.to_string_lossy().into_owned())
}

/// Extract (og:title | <title>, og:description | meta description, og:image)
/// from an HTML document. Pure string scanning: tolerant of attribute order
/// and quoting style, no HTML-parser dependency.
pub fn parse_og(html: &str) -> (Option<String>, Option<String>, Option<String>) {
    let mut og_title = None;
    let mut og_description = None;
    let mut og_image = None;
    let mut meta_description = None;

    for tag in html.split('<').skip(1) {
        let Some(end) = tag.find('>') else { continue };
        let tag = &tag[..end];
        if !tag.get(..4).is_some_and(|t| t.eq_ignore_ascii_case("meta")) {
            continue;
        }
        let key = attr_value(tag, "property").or_else(|| attr_value(tag, "name"));
        let Some(key) = key else { continue };
        let Some(content) = attr_value(tag, "content") else {
            continue;
        };
        let content = decode_entities(&content);
        match key.to_ascii_lowercase().as_str() {
            "og:title" if og_title.is_none() => og_title = Some(content),
            "og:description" if og_description.is_none() => og_description = Some(content),
            "og:image" | "og:image:url" if og_image.is_none() => og_image = Some(content),
            "description" if meta_description.is_none() => meta_description = Some(content),
            _ => {}
        }
    }

    let title = og_title.or_else(|| html_title(html));
    let description = og_description.or(meta_description);
    (
        title.filter(|t| !t.trim().is_empty()),
        description.filter(|d| !d.trim().is_empty()),
        og_image.filter(|i| !i.trim().is_empty()),
    )
}

/// The text of the document's `<title>` element, if any.
fn html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = html[start..].find('>')? + start + 1;
    let close = lower[open_end..].find("</title")? + open_end;
    Some(decode_entities(html[open_end..close].trim()))
}

/// The value of `name="..."` / `name='...'` inside an HTML tag body.
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut search = 0;
    loop {
        let rel = lower[search..].find(name)?;
        let at = search + rel;
        // Must be a standalone attribute name (preceded by whitespace or the
        // tag start, so "content" does not match inside "data-content")
        // followed by '='.
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let after = at + name.len();
        let rest = tag.get(after..)?;
        let rest_trim = rest.trim_start();
        if before_ok && rest_trim.starts_with('=') {
            let value = rest_trim[1..].trim_start();
            let quote = value.chars().next()?;
            if quote == '"' || quote == '\'' {
                let inner = &value[1..];
                let close = inner.find(quote)?;
                return Some(inner[..close].to_string());
            }
            // Unquoted value: read to whitespace.
            let end = value.find(char::is_whitespace).unwrap_or(value.len());
            return Some(value[..end].to_string());
        }
        search = at + name.len();
    }
}

/// Decode the handful of HTML entities that commonly appear in titles.
fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
}

/// Resolve a possibly-relative og:image URL against the page URL.
/// Returns None for schemes we will not fetch (data:, ftp:, ...).
pub fn resolve_url(base: &str, target: &str) -> Option<String> {
    if target.starts_with("https://") || target.starts_with("http://") {
        return Some(target.to_string());
    }
    let scheme_end = base.find("://")?;
    let scheme = &base[..scheme_end];
    let after_scheme = &base[scheme_end + 3..];
    let host_end = after_scheme.find('/').unwrap_or(after_scheme.len());
    let host = &after_scheme[..host_end];
    if target.starts_with("//") {
        return Some(format!("{scheme}:{target}"));
    }
    if target.starts_with('/') {
        return Some(format!("{scheme}://{host}{target}"));
    }
    if target.contains(':') {
        return None; // data:, mailto:, and friends
    }
    // Relative path: resolve against the base URL's directory.
    let dir_end = base.rfind('/').filter(|&i| i > scheme_end + 2);
    match dir_end {
        Some(i) => Some(format!("{}/{target}", &base[..i])),
        None => Some(format!("{base}/{target}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_og_prefers_og_tags() {
        let html = r#"<html><head>
            <title>Fallback Title</title>
            <meta property="og:title" content="OG Title" />
            <meta property="og:description" content="OG Desc"/>
            <meta property="og:image" content="https://ex.com/img.png">
            <meta name="description" content="Meta Desc">
        </head></html>"#;
        let (title, desc, image) = parse_og(html);
        assert_eq!(title.as_deref(), Some("OG Title"));
        assert_eq!(desc.as_deref(), Some("OG Desc"));
        assert_eq!(image.as_deref(), Some("https://ex.com/img.png"));
    }

    #[test]
    fn parse_og_falls_back_to_title_and_meta_description() {
        let html = r#"<head><TITLE>Plain &amp; Simple</TITLE>
            <meta name="description" content="A page."></head>"#;
        let (title, desc, image) = parse_og(html);
        assert_eq!(title.as_deref(), Some("Plain & Simple"));
        assert_eq!(desc.as_deref(), Some("A page."));
        assert_eq!(image, None);
    }

    #[test]
    fn parse_og_handles_attribute_order_and_single_quotes() {
        let html = r#"<meta content='Reversed' property='og:title'>"#;
        let (title, _, _) = parse_og(html);
        assert_eq!(title.as_deref(), Some("Reversed"));
    }

    #[test]
    fn parse_og_empty_document() {
        let (title, desc, image) = parse_og("<html><body>hi</body></html>");
        assert_eq!(title, None);
        assert_eq!(desc, None);
        assert_eq!(image, None);
    }

    #[test]
    fn attr_value_does_not_match_suffix_names() {
        // "property" must not match inside "data-property"; og:image:url
        // handling relies on exact key matching too.
        let tag = r#"meta data-content="wrong" content="right""#;
        assert_eq!(attr_value(tag, "content").as_deref(), Some("right"));
    }

    #[test]
    fn resolve_url_variants() {
        let base = "https://ex.com/a/b/page.html";
        assert_eq!(
            resolve_url(base, "https://cdn.com/i.png").as_deref(),
            Some("https://cdn.com/i.png")
        );
        assert_eq!(
            resolve_url(base, "//cdn.com/i.png").as_deref(),
            Some("https://cdn.com/i.png")
        );
        assert_eq!(
            resolve_url(base, "/img/i.png").as_deref(),
            Some("https://ex.com/img/i.png")
        );
        assert_eq!(
            resolve_url(base, "i.png").as_deref(),
            Some("https://ex.com/a/b/i.png")
        );
        assert_eq!(resolve_url(base, "data:image/png;base64,xx"), None);
    }
}
