//! Link/URL detection helpers used by `chat_pane` rendering and the
//! post-render OSC 8 hyperlink injection pass.
//!
//! - `LinkRegion` describes a contiguous run of "link-styled" cells
//!   in the rendered buffer (link color fg + UNDERLINED). Used by
//!   `main`'s OSC 8 emitter and by `domain::ImageState::link_regions`.
//! - `extract_url` peels a `https://` / `http://` / `file:///` URL
//!   out of arbitrary text.
//! - `is_link_style` and `collect_link_regions` rescan the buffer
//!   after rendering to find link runs (handles wrapped lines).
//! - `split_spans_by_newline` is a generic "split spans on `\n`"
//!   helper used by chat_pane to wrap multi-line bodies.
//! - `styled_uri_spans` builds the styled `Vec<Span>` for a message
//!   body, layering URI/mention/spoiler/bold/italic styles together.

use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::Span,
};

use crate::domain::LinkRegion;
use crate::signal::types::StyleType;
use crate::theme::Theme;

/// Extract a URL from link-styled text.
fn extract_url(text: &str) -> String {
    for scheme in &["file:///", "https://", "http://"] {
        if let Some(pos) = text.find(scheme) {
            let uri_start = &text[pos..];
            let uri_end = uri_start
                .find(|c: char| c.is_whitespace())
                .unwrap_or(uri_start.len());
            return uri_start[..uri_end].to_string();
        }
    }
    text.to_string()
}

/// Check if a cell's style matches the link style (link color fg + UNDERLINED).
fn is_link_style(style: &Style, link_color: Color) -> bool {
    style.fg == Some(link_color) && style.add_modifier.contains(Modifier::UNDERLINED)
}

/// Scan a rendered buffer area for consecutive cells with the link style,
/// and collect them into LinkRegion structs.
pub(in crate::ui) fn collect_link_regions(
    buf: &Buffer,
    area: Rect,
    link_color: Color,
) -> Vec<LinkRegion> {
    let right_edge = area.x.saturating_add(area.width);
    let mut regions = Vec::new();
    let mut wrap_url: Option<String> = None;

    for y in area.y..area.y.saturating_add(area.height) {
        let mut x = area.x;
        let mut row_last_url: Option<String> = None;
        let mut row_last_reached_edge = false;

        while x < right_edge {
            let cell = match buf.cell(Position::new(x, y)) {
                Some(c) => c,
                None => {
                    x += 1;
                    continue;
                }
            };

            if !is_link_style(&cell.style(), link_color) {
                x += 1;
                continue;
            }

            // Start of a link run
            let start_x = x;
            let mut text = String::new();

            while x < right_edge {
                match buf.cell(Position::new(x, y)) {
                    Some(c) if is_link_style(&c.style(), link_color) => {
                        let sym = c.symbol();
                        if !sym.is_empty() {
                            text.push_str(sym);
                        }
                        x += 1;
                    }
                    _ => break,
                }
            }

            if text.is_empty() {
                continue;
            }

            // Determine URL: use continuation URL if this is a wrapped link
            let url = if start_x == area.x {
                if let Some(ref wu) = wrap_url {
                    wu.clone()
                } else {
                    extract_url(&text)
                }
            } else {
                extract_url(&text)
            };

            let reached_edge = x >= right_edge;
            row_last_url = Some(url.clone());
            row_last_reached_edge = reached_edge;

            // Capture background color from the first cell of the link run so
            // emit_osc8_links can preserve it (e.g. highlight bg on selection).
            let bg = buf
                .cell(Position::new(start_x, y))
                .and_then(|c| c.style().bg);
            regions.push(LinkRegion {
                x: start_x,
                y,
                url,
                text,
                width: x - start_x,
                bg,
            });
        }

        // Propagate URL for wrapped links
        wrap_url = if row_last_reached_edge {
            row_last_url
        } else {
            None
        };
    }

    regions
}

/// Split a list of body spans into sub-lists, one per output line, using `\n`
/// in any span's content as the line break. Styles are preserved when splitting
/// a span. Empty lines (consecutive `\n`) produce an empty sub-list.
pub(in crate::ui) fn split_spans_by_newline(spans: Vec<Span<'static>>) -> Vec<Vec<Span<'static>>> {
    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    for span in spans {
        if !span.content.contains('\n') {
            lines.last_mut().unwrap().push(span);
            continue;
        }
        let style = span.style;
        let content = span.content.into_owned();
        let mut parts = content.split('\n').peekable();
        while let Some(part) = parts.next() {
            if !part.is_empty() {
                lines
                    .last_mut()
                    .unwrap()
                    .push(Span::styled(part.to_string(), style));
            }
            if parts.peek().is_some() {
                lines.push(Vec::new());
            }
        }
    }
    lines
}

/// Split a message body into spans, styling any URI (https://, http://, file:///) as
/// underlined blue text. Non-URI text is rendered as plain spans.
///
/// Returns `(spans, Option<hidden_url>)`. For attachment bodies like
/// `[image: label](file:///path)`, the bracket text is the visible link and
/// the URI inside parens is returned separately (not displayed).
pub(in crate::ui) fn styled_uri_spans(
    body: &str,
    mention_ranges: &[(usize, usize)],
    style_ranges: &[(usize, usize, StyleType)],
    theme: &Theme,
) -> (Vec<Span<'static>>, Option<String>) {
    let link_style = Style::default()
        .fg(theme.link)
        .add_modifier(Modifier::UNDERLINED);
    let mention_style = Style::default()
        .fg(theme.mention)
        .add_modifier(Modifier::BOLD);

    // Attachment/image/voice patterns: extract bracket text as display, URI as hidden metadata
    if body.starts_with("[image:")
        || body.starts_with("[attachment:")
        || body.starts_with("[voice ")
    {
        // Extract the bracket portion: [image: label], [attachment: label], or [voice ▶ label]
        if let Some(bracket_end) = body.find(']') {
            let display_text = &body[..=bracket_end]; // e.g. "[image: photo.jpg]"

            // Extract URI from either new format ](file:///...) or old format ] file:///...
            let hidden_url = if let Some(uri_pos) = body.find("file:///") {
                let uri_start = &body[uri_pos..];
                // End at whitespace, closing paren, or end of string
                let uri_end = uri_start
                    .find(|c: char| c.is_whitespace() || c == ')')
                    .unwrap_or(uri_start.len());
                Some(uri_start[..uri_end].to_string())
            } else {
                None
            };

            if hidden_url.is_some() {
                return (
                    vec![Span::styled(display_text.to_string(), link_style)],
                    hidden_url,
                );
            }
        }
    }

    // Build a sorted list of styled regions: mentions and URIs
    // Each region: (byte_start, byte_end, style)
    let mut regions: Vec<(usize, usize, Style)> = Vec::new();

    // Add mention regions
    for &(start, end) in mention_ranges {
        if start < body.len() && end <= body.len() {
            regions.push((start, end, mention_style));
        }
    }

    // Find URI regions
    let mut search_pos = 0;
    while search_pos < body.len() {
        let rest = &body[search_pos..];
        let next_uri = ["https://", "http://", "file:///"]
            .iter()
            .filter_map(|scheme| rest.find(scheme).map(|pos| (pos, *scheme)))
            .min_by_key(|(pos, _)| *pos);

        match next_uri {
            Some((rel_pos, _scheme)) => {
                let abs_start = search_pos + rel_pos;
                let uri_slice = &body[abs_start..];
                let uri_len = uri_slice
                    .find(|c: char| c.is_whitespace())
                    .unwrap_or(uri_slice.len());
                let abs_end = abs_start + uri_len;
                // Only add if not overlapping a mention region
                let overlaps = regions
                    .iter()
                    .any(|(ms, me, _)| abs_start < *me && abs_end > *ms);
                if !overlaps {
                    regions.push((abs_start, abs_end, link_style));
                }
                search_pos = abs_end;
            }
            None => break,
        }
    }

    // Sort regions by start position
    regions.sort_by_key(|r| r.0);

    // If no text styles, use the simple path
    if style_ranges.is_empty() {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut pos = 0;
        for (start, end, style) in &regions {
            if *start > pos {
                spans.push(Span::raw(body[pos..*start].to_string()));
            }
            spans.push(Span::styled(body[*start..*end].to_string(), *style));
            pos = *end;
        }
        if pos < body.len() {
            spans.push(Span::raw(body[pos..].to_string()));
        }
        return (spans, None);
    }

    // With text styles: collect all boundary points and build segments where
    // the active set of styles is constant
    let mut boundaries: Vec<usize> = Vec::new();
    boundaries.push(0);
    boundaries.push(body.len());
    for &(start, end, _) in &regions {
        boundaries.push(start);
        boundaries.push(end);
    }
    for &(start, end, _) in style_ranges {
        if start <= body.len() {
            boundaries.push(start);
        }
        if end <= body.len() {
            boundaries.push(end);
        }
    }
    boundaries.sort();
    boundaries.dedup();

    let mut spans: Vec<Span<'static>> = Vec::new();
    for window in boundaries.windows(2) {
        let seg_start = window[0];
        let seg_end = window[1];
        if seg_start >= seg_end || seg_start >= body.len() {
            continue;
        }
        let seg_end = seg_end.min(body.len());

        // Determine base style from mention/URI regions
        let mut style = Style::default();
        for &(rs, re, ref_style) in &regions {
            if seg_start >= rs && seg_end <= re {
                style = ref_style;
                break;
            }
        }

        // Check for spoiler first — if any spoiler range covers this segment,
        // replace the text with block characters
        let mut is_spoiler = false;
        for &(ss, se, st) in style_ranges {
            if st == StyleType::Spoiler && seg_start >= ss && seg_end <= se {
                is_spoiler = true;
                break;
            }
        }

        let segment_text = &body[seg_start..seg_end];
        if is_spoiler {
            // Replace each character with a block character
            let block_text: String = segment_text.chars().map(|_| '\u{2588}').collect();
            let spoiler_style = style.fg(theme.fg_muted);
            spans.push(Span::styled(block_text, spoiler_style));
        } else {
            // Apply text style modifiers
            for &(ss, se, st) in style_ranges {
                if seg_start >= ss && seg_end <= se {
                    match st {
                        StyleType::Bold => style = style.add_modifier(Modifier::BOLD),
                        StyleType::Italic => style = style.add_modifier(Modifier::ITALIC),
                        StyleType::Strikethrough => {
                            style = style.add_modifier(Modifier::CROSSED_OUT)
                        }
                        StyleType::Monospace => style = style.fg(theme.fg_muted),
                        StyleType::Spoiler => {} // handled above
                    }
                }
            }

            if style == Style::default() {
                spans.push(Span::raw(segment_text.to_string()));
            } else {
                spans.push(Span::styled(segment_text.to_string(), style));
            }
        }
    }

    (spans, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("https://example.com", "https://example.com")]
    #[case("http://foo.bar/baz", "http://foo.bar/baz")]
    #[case("file:///tmp/a.txt", "file:///tmp/a.txt")]
    #[case("check https://x.com/path here", "https://x.com/path")]
    #[case("no-scheme.com", "no-scheme.com")]
    fn extract_url_cases(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(extract_url(input), expected);
    }
}
