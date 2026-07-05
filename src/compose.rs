//! Composer text-formatting markup (#609).
//!
//! Signal has no wire-format markup; styles travel as (start, length, STYLE)
//! ranges alongside a plain body. This module gives the composer a
//! WhatsApp-style input syntax and converts it to those ranges at send time:
//! `*bold*`, `_italic_`, `~strikethrough~`, `` `monospace` ``, `||spoiler||`.
//!
//! Rules (to avoid mangling ordinary text like `snake_case` or `2 * 3`):
//! - an opening marker must not follow an alphanumeric char and must be
//!   directly attached to non-whitespace text;
//! - a closing marker must be directly attached to preceding non-whitespace
//!   text and must not be followed by an alphanumeric char;
//! - a span never crosses a newline;
//! - text inside a detected URI (`https://`, `http://`, `file:///`) is
//!   always literal, so underscores in URLs survive;
//! - spans do not nest: content inside a span is copied verbatim.
//!
//! Unmatched or invalid markers are left in the text untouched.

use crate::signal::types::StyleType;

/// The result of parsing composer markup: the stripped body plus the same
/// style ranges in the two coordinate systems callers need. `byte_ranges`
/// feeds local display (`DisplayMessage::style_ranges`); `utf16_ranges` feeds
/// signal-cli's `textStyle` param; `removed_utf16` lets callers shift offsets
/// that were computed on the original text (mention placeholders).
pub struct Markup {
    /// Text with style markers removed.
    pub body: String,
    /// Style ranges as byte (start, end) offsets into `body`.
    pub byte_ranges: Vec<(usize, usize, StyleType)>,
    /// Style ranges as UTF-16 (start, length) into `body`.
    pub utf16_ranges: Vec<(usize, usize, StyleType)>,
    /// UTF-16 offsets, in the original text, of each removed marker char,
    /// ascending. Every marker char is ASCII, so each is one UTF-16 unit.
    pub removed_utf16: Vec<usize>,
}

/// The style marker starting at `chars[i]`, as (marker length, style).
fn delim_at(chars: &[char], i: usize) -> Option<(usize, StyleType)> {
    match chars[i] {
        '|' if chars.get(i + 1) == Some(&'|') => Some((2, StyleType::Spoiler)),
        '*' => Some((1, StyleType::Bold)),
        '_' => Some((1, StyleType::Italic)),
        '~' => Some((1, StyleType::Strikethrough)),
        '`' => Some((1, StyleType::Monospace)),
        _ => None,
    }
}

/// Mark every char that is part of a URI so markers inside URLs stay literal.
/// A URI runs from its scheme to the next whitespace, mirroring `ui::links`.
fn uri_mask(chars: &[char]) -> Vec<bool> {
    const SCHEMES: [&str; 3] = ["https://", "http://", "file:///"];
    let mut mask = vec![false; chars.len()];
    let mut i = 0;
    while i < chars.len() {
        let starts_scheme = SCHEMES.iter().any(|s| {
            s.chars()
                .enumerate()
                .all(|(k, sc)| chars.get(i + k) == Some(&sc))
        });
        if starts_scheme {
            while i < chars.len() && !chars[i].is_whitespace() {
                mask[i] = true;
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    mask
}

/// Find a valid closing marker matching the opener at `chars[open..open+dlen]`,
/// scanning from just past the first content char. Returns the closer index.
fn find_closer(
    chars: &[char],
    in_uri: &[bool],
    content_start: usize,
    dlen: usize,
) -> Option<usize> {
    let marker: Vec<char> = chars[content_start - dlen..content_start].to_vec();
    // Require at least one content char before the closer.
    let mut j = content_start + 1;
    while j + dlen <= chars.len() {
        if chars[j] == '\n' {
            return None;
        }
        let matches = (0..dlen).all(|k| chars[j + k] == marker[k]) && !in_uri[j];
        if matches {
            let prev_ok = !chars[j - 1].is_whitespace();
            let after_ok = chars.get(j + dlen).is_none_or(|c| !c.is_alphanumeric());
            if prev_ok && after_ok {
                return Some(j);
            }
        }
        j += 1;
    }
    None
}

/// Parse WhatsApp-style markup out of `text`, returning the stripped body and
/// the style ranges. Text without markers passes through unchanged.
pub fn parse_markup(text: &str) -> Markup {
    let chars: Vec<char> = text.chars().collect();
    let in_uri = uri_mask(&chars);

    // UTF-16 offset of each original char (prefix sums), plus end sentinel.
    let mut orig_utf16 = Vec::with_capacity(chars.len() + 1);
    let mut acc = 0usize;
    for c in &chars {
        orig_utf16.push(acc);
        acc += c.len_utf16();
    }
    orig_utf16.push(acc);

    let mut body = String::with_capacity(text.len());
    let mut out_utf16 = 0usize;
    let mut byte_ranges = Vec::new();
    let mut utf16_ranges = Vec::new();
    let mut removed_utf16 = Vec::new();

    let mut i = 0;
    while i < chars.len() {
        if !in_uri[i]
            && let Some((dlen, style)) = delim_at(&chars, i)
        {
            let prev_ok = i == 0 || !chars[i - 1].is_alphanumeric();
            let next_ok = chars.get(i + dlen).is_some_and(|c| !c.is_whitespace());
            if prev_ok
                && next_ok
                && let Some(close) = find_closer(&chars, &in_uri, i + dlen, dlen)
            {
                for k in 0..dlen {
                    removed_utf16.push(orig_utf16[i + k]);
                }
                let start_byte = body.len();
                let start_utf16 = out_utf16;
                for &c in &chars[i + dlen..close] {
                    body.push(c);
                    out_utf16 += c.len_utf16();
                }
                byte_ranges.push((start_byte, body.len(), style));
                utf16_ranges.push((start_utf16, out_utf16 - start_utf16, style));
                for k in 0..dlen {
                    removed_utf16.push(orig_utf16[close + k]);
                }
                i = close + dlen;
                continue;
            }
        }
        body.push(chars[i]);
        out_utf16 += chars[i].len_utf16();
        i += 1;
    }

    Markup {
        body,
        byte_ranges,
        utf16_ranges,
        removed_utf16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn parse(text: &str) -> (String, Vec<(usize, usize, StyleType)>) {
        let m = parse_markup(text);
        (m.body, m.byte_ranges)
    }

    #[rstest]
    #[case("*bold*", "bold", StyleType::Bold, 0, 4)]
    #[case("_italic_", "italic", StyleType::Italic, 0, 6)]
    #[case("~gone~", "gone", StyleType::Strikethrough, 0, 4)]
    #[case("`code`", "code", StyleType::Monospace, 0, 4)]
    #[case("||secret||", "secret", StyleType::Spoiler, 0, 6)]
    fn single_span(
        #[case] input: &str,
        #[case] body: &str,
        #[case] style: StyleType,
        #[case] start: usize,
        #[case] end: usize,
    ) {
        assert_eq!(parse(input), (body.to_string(), vec![(start, end, style)]));
    }

    #[test]
    fn span_mid_sentence() {
        let (body, ranges) = parse("this is *very* important");
        assert_eq!(body, "this is very important");
        assert_eq!(ranges, vec![(8, 12, StyleType::Bold)]);
    }

    #[test]
    fn multiple_spans() {
        let (body, ranges) = parse("*a* and _b_");
        assert_eq!(body, "a and b");
        assert_eq!(
            ranges,
            vec![(0, 1, StyleType::Bold), (6, 7, StyleType::Italic)]
        );
    }

    #[rstest]
    #[case("snake_case_name")]
    #[case("2 * 3 = 6")]
    #[case("5*3")]
    #[case("a_b")]
    #[case("*unclosed")]
    #[case("* spaced *")]
    #[case("**")]
    #[case("ends with dash-_x")]
    fn literal_text_untouched(#[case] input: &str) {
        assert_eq!(parse(input), (input.to_string(), vec![]));
    }

    #[test]
    fn no_span_across_newline() {
        assert_eq!(parse("*a\nb*"), ("*a\nb*".to_string(), vec![]));
    }

    #[test]
    fn url_underscores_survive() {
        let url = "see https://ex.com/a_b_c ok";
        assert_eq!(parse(url), (url.to_string(), vec![]));
    }

    #[test]
    fn span_before_url_does_not_close_inside_it() {
        let text = "_note https://ex.com/a_b";
        assert_eq!(parse(text), (text.to_string(), vec![]));
    }

    #[test]
    fn punctuation_boundaries_allowed() {
        let (body, ranges) = parse("(*bold*), _it_.");
        assert_eq!(body, "(bold), it.");
        assert_eq!(
            ranges,
            vec![(1, 5, StyleType::Bold), (8, 10, StyleType::Italic)]
        );
    }

    #[test]
    fn content_is_not_rescanned() {
        // No nesting: inner markers are literal inside a span.
        let (body, ranges) = parse("*a _b_ c*");
        assert_eq!(body, "a _b_ c");
        assert_eq!(ranges, vec![(0, 7, StyleType::Bold)]);
    }

    #[test]
    fn utf16_ranges_and_removed_offsets() {
        // "😀 *bold*": emoji is 2 UTF-16 units / 4 bytes.
        let m = parse_markup("\u{1F600} *bold*");
        assert_eq!(m.body, "\u{1F600} bold");
        assert_eq!(m.byte_ranges, vec![(5, 9, StyleType::Bold)]);
        assert_eq!(m.utf16_ranges, vec![(3, 4, StyleType::Bold)]);
        // Markers sat at UTF-16 offsets 3 and 8 in the original text.
        assert_eq!(m.removed_utf16, vec![3, 8]);
    }

    #[test]
    fn spoiler_removed_offsets_cover_both_chars() {
        let m = parse_markup("||s||");
        assert_eq!(m.body, "s");
        assert_eq!(m.removed_utf16, vec![0, 1, 3, 4]);
    }

    #[test]
    fn mention_placeholder_is_valid_span_content() {
        let m = parse_markup("*\u{FFFC} rocks*");
        assert_eq!(m.body, "\u{FFFC} rocks");
        assert_eq!(m.utf16_ranges, vec![(0, 7, StyleType::Bold)]);
        assert_eq!(m.removed_utf16, vec![0, 8]);
    }
}
