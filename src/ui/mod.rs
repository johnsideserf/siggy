//! Stateless rendering layer.
//!
//! [`draw`] takes the current [`App`] and renders sidebar + chat + status
//! bar each frame. Sender colors are hash-based across an 8-color palette;
//! groups are prefixed with `#`. OSC 8 hyperlinks are injected post-render
//! to dodge ratatui width calculation bugs (see [`LinkRegion`]).

mod composer;
mod overlays;
mod sidebar;
mod status_bar;

use composer::draw_input;
use overlays::about::draw_about;
use overlays::action_menu::draw_action_menu;
use overlays::contacts::draw_contacts;
use overlays::customize::draw_customize;
use overlays::delete_confirm::draw_delete_confirm;
use overlays::emoji_picker::draw_emoji_picker;
use overlays::file_browser::draw_file_browser;
use overlays::forward::draw_forward;
use overlays::group_menu::draw_group_menu;
use overlays::help::draw_help;
use overlays::keybindings::draw_keybindings;
use overlays::message_request::draw_message_request;
use overlays::pin_duration::draw_pin_duration_picker;
use overlays::poll_vote::draw_poll_vote_overlay;
use overlays::profile::draw_profile;
use overlays::reaction_picker::draw_reaction_picker;
use overlays::search::draw_search;
use overlays::settings::draw_settings;
use overlays::settings_profile::draw_settings_profile_manager;
use overlays::theme_picker::draw_theme_picker;
use overlays::verify::draw_verify;
use sidebar::draw_sidebar;
use status_bar::draw_status_bar;

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
};

use crate::app::{App, AutocompleteMode, InputMode, OverlayKind, VisibleImage};
use crate::image_render::{self, ImageProtocol};
use crate::input::{COMMANDS, format_compact_duration};
use crate::signal::types::{MessageStatus, PollData, PollVote, Reaction, StyleType, TrustLevel};
use crate::theme::Theme;

// Layout constants
const SIDEBAR_AUTO_HIDE_WIDTH: u16 = 60;
const MIN_CHAT_WIDTH: u16 = 30;
const MSG_WINDOW_MULTIPLIER: usize = 10;

// Popup dimensions
pub(super) const SETTINGS_POPUP_WIDTH: u16 = 50;
pub(super) const SETTINGS_POPUP_HEIGHT: u16 = 25;
pub(super) const CONTACTS_POPUP_WIDTH: u16 = 50;
pub(super) const CONTACTS_MAX_VISIBLE: usize = 20;
pub(super) const FILE_BROWSER_POPUP_WIDTH: u16 = 60;
pub(super) const FILE_BROWSER_MAX_VISIBLE: usize = 20;
pub(super) const SEARCH_POPUP_WIDTH: u16 = 60;
pub(super) const SEARCH_MAX_VISIBLE: usize = 15;
pub(super) const GROUP_MENU_POPUP_WIDTH: u16 = 40;
pub(super) const GROUP_MEMBER_MAX_VISIBLE: usize = 15;
pub(super) const ABOUT_POPUP_WIDTH: u16 = 50;
pub(super) const PROFILE_POPUP_WIDTH: u16 = 50;
pub(super) const EMOJI_POPUP_WIDTH: u16 = 52;
pub(super) const EMOJI_POPUP_HEIGHT: u16 = 20;

/// Convert emoji in a string to text emoticons or :shortcodes:.
/// Common emoji get classic emoticons (e.g. :) <3), others get :shortcode: format.
fn emoji_to_text(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        // Try to match emoji starting at this character
        // Build a candidate string (emoji can be multi-char with ZWJ sequences)
        let mut candidate = String::new();
        candidate.push(c);
        // Consume variation selectors and ZWJ sequences
        while let Some(&next) = chars.peek() {
            if next == '\u{fe0f}'
                || next == '\u{200d}'
                || next == '\u{20e3}'
                || ('\u{1f3fb}'..='\u{1f3ff}').contains(&next)
            {
                candidate.push(chars.next().unwrap());
            } else if next.is_ascii() {
                break;
            } else if emojis::get(&format!("{candidate}{next}")).is_some() {
                candidate.push(chars.next().unwrap());
            } else {
                break;
            }
        }
        if let Some(emoji) = emojis::get(&candidate) {
            // Check for common emoticon mapping first
            let text = match emoji.as_str() {
                "\u{1f642}" | "\u{1f60a}" | "\u{263a}\u{fe0f}" => ":)",
                "\u{1f600}" | "\u{1f603}" | "\u{1f604}" => ":D",
                "\u{1f601}" => ":D",
                "\u{1f606}" => "XD",
                "\u{1f609}" => ";)",
                "\u{1f61e}" | "\u{2639}\u{fe0f}" | "\u{1f641}" => ":(",
                "\u{1f622}" => ":'(",
                "\u{1f62d}" => ":'(",
                "\u{1f602}" => "XD",
                "\u{1f923}" => "XD",
                "\u{1f60d}" => "<3_<3",
                "\u{2764}\u{fe0f}" | "\u{2764}" => "<3",
                "\u{1f495}" | "\u{1f496}" | "\u{1f497}" | "\u{1f498}" => "<3",
                "\u{1f44d}" | "\u{1f44d}\u{1f3fb}" | "\u{1f44d}\u{1f3fc}"
                | "\u{1f44d}\u{1f3fd}" | "\u{1f44d}\u{1f3fe}" | "\u{1f44d}\u{1f3ff}" => "+1",
                "\u{1f44e}" => "-1",
                "\u{1f61b}" | "\u{1f61c}" | "\u{1f61d}" => ":P",
                "\u{1f610}" | "\u{1f611}" => ":|",
                "\u{1f914}" => ":?",
                "\u{1f62e}" | "\u{1f632}" => ":O",
                "\u{1f615}" => ":/",
                _ => {
                    // Fall back to :shortcode:
                    if let Some(sc) = emoji.shortcode() {
                        result.push(':');
                        result.push_str(sc);
                        result.push(':');
                    } else {
                        result.push_str(&candidate);
                    }
                    continue;
                }
            };
            result.push_str(text);
        } else {
            result.push_str(&candidate);
        }
    }
    result
}

/// Map a MessageStatus to its display symbol and color.
pub(crate) fn status_symbol(
    status: MessageStatus,
    nerd_fonts: bool,
    color: bool,
    theme: &Theme,
) -> (&'static str, Color) {
    let (unicode_sym, nerd_sym, colored) = match status {
        MessageStatus::Failed => ("\u{2717}", "\u{f055c}", theme.receipt_failed),
        MessageStatus::Sending => ("\u{25cc}", "\u{f0996}", theme.receipt_sending),
        MessageStatus::Sent => ("\u{25cb}", "\u{f0954}", theme.receipt_sent),
        MessageStatus::Delivered => ("\u{2713}", "\u{f012c}", theme.receipt_delivered),
        MessageStatus::Read => ("\u{25cf}", "\u{f012d}", theme.receipt_read),
        MessageStatus::Viewed => ("\u{25c9}", "\u{f0208}", theme.receipt_viewed),
    };
    let sym = if nerd_fonts { nerd_sym } else { unicode_sym };
    let fg = if color { colored } else { theme.fg_muted };
    (sym, fg)
}

/// Hash a sender name to one of ~8 distinct colors. "you" always gets sender_self.
pub(crate) fn sender_color(name: &str, theme: &Theme) -> Color {
    if name == "you" {
        return theme.sender_self;
    }
    let hash: u32 = name
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    theme.sender_palette[(hash as usize) % theme.sender_palette.len()]
}

/// Truncate a string to fit within `max_width`, appending `…` if truncated.
pub(crate) fn truncate(s: &str, max_width: usize) -> String {
    if s.len() <= max_width {
        s.to_string()
    } else if max_width <= 1 {
        "…".to_string()
    } else {
        let mut truncated: String = s.chars().take(max_width - 1).collect();
        truncated.push('…');
        truncated
    }
}

/// Build a centered separator line: `───── label ─────`
pub(crate) fn build_separator(label: &str, width: usize, style: Style) -> Line<'static> {
    let pad_total = width.saturating_sub(label.len());
    let pad_left = pad_total / 2;
    let pad_right = pad_total - pad_left;
    Line::from(Span::styled(
        format!("{}{}{}", "─".repeat(pad_left), label, "─".repeat(pad_right)),
        style,
    ))
}

/// Create a centered popup overlay: clears the area, returns the Rect and a styled Block.
/// Preferred width/height are clamped to fit within the terminal.
pub(super) fn centered_popup(
    frame: &mut Frame,
    area: Rect,
    pref_width: u16,
    pref_height: u16,
    title: &str,
    theme: &Theme,
) -> (Rect, Block<'static>) {
    let w = pref_width.min(area.width.saturating_sub(4));
    let h = pref_height.min(area.height.saturating_sub(2));
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let popup_area = Rect::new(x, y, w, h);
    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent))
        .title(title.to_string())
        .title_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .style(Style::default().bg(theme.bg));
    (popup_area, block)
}

/// A clickable link region detected in the rendered buffer.
pub struct LinkRegion {
    pub x: u16,
    pub y: u16,
    pub url: String,
    pub text: String,
    /// Display width in terminal columns (may differ from text.len() for Unicode).
    pub width: u16,
    /// Background color from the buffer cell, if non-default (e.g. highlight).
    pub bg: Option<Color>,
}

/// Extract a URL from link-styled text.
pub(crate) fn extract_url(text: &str) -> String {
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
fn collect_link_regions(buf: &Buffer, area: Rect, link_color: Color) -> Vec<LinkRegion> {
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
fn split_spans_by_newline(spans: Vec<Span<'static>>) -> Vec<Vec<Span<'static>>> {
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
fn styled_uri_spans(
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

    // Attachment/image patterns: extract bracket text as display, URI as hidden metadata
    if body.starts_with("[image:") || body.starts_with("[attachment:") {
        // Extract the bracket portion: [image: label] or [attachment: label]
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

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.image.link_url_map.clear();
    app.image.visible_images.clear();
    let size = frame.area();
    let terminal_width = size.width;

    // Main vertical layout: body + status bar
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // body
            Constraint::Length(1), // status bar
        ])
        .split(size);

    let body_area = outer[0];
    let status_area = outer[1];

    // Narrow terminal adaptation: auto-hide sidebar below threshold
    let sidebar_auto_hidden = terminal_width < SIDEBAR_AUTO_HIDE_WIDTH;
    let show_sidebar = app.sidebar_visible && !sidebar_auto_hidden;

    let input_area = if show_sidebar {
        let (sidebar_idx, chat_idx, constraints) = if app.sidebar_on_right {
            (
                1,
                0,
                [
                    Constraint::Min(MIN_CHAT_WIDTH),
                    Constraint::Length(app.sidebar_width),
                ],
            )
        } else {
            (
                0,
                1,
                [
                    Constraint::Length(app.sidebar_width),
                    Constraint::Min(MIN_CHAT_WIDTH),
                ],
            )
        };
        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(constraints)
            .split(body_area);

        draw_sidebar(frame, app, horizontal[sidebar_idx]);
        draw_chat_area(frame, app, horizontal[chat_idx])
    } else {
        app.mouse.sidebar_inner = None;
        draw_chat_area(frame, app, body_area)
    };

    draw_status_bar(frame, app, status_area, sidebar_auto_hidden);

    // Autocomplete popup (overlays everything)
    if app.is_overlay(OverlayKind::Autocomplete) {
        let has_items = !app.autocomplete.is_empty();
        if has_items {
            draw_autocomplete(frame, app, input_area);
        }
    }

    // Settings overlay (overlays everything)
    if app.is_overlay(OverlayKind::Settings) {
        draw_settings(frame, app, size);
    }

    // Customize sub-menu overlay (Theme, Keybindings, Profile)
    if app.is_overlay(OverlayKind::Customize) {
        draw_customize(frame, app, size);
    }

    // Help overlay (overlays everything)
    if app.is_overlay(OverlayKind::Help) {
        draw_help(frame, app, size);
    }

    // Contacts overlay (overlays everything)
    if app.is_overlay(OverlayKind::Contacts) {
        draw_contacts(frame, app, size);
    }

    // Verify identity overlay
    if app.is_overlay(OverlayKind::Verify) {
        draw_verify(frame, app, size);
    }

    // Search overlay
    if app.is_overlay(OverlayKind::Search) {
        draw_search(frame, app, size);
    }

    // File browser overlay
    if app.is_overlay(OverlayKind::FilePicker) {
        draw_file_browser(frame, app, size);
    }

    // Group management menu overlay
    if app.is_overlay(OverlayKind::GroupMenu) {
        draw_group_menu(frame, app, size);
    }

    // Message request overlay
    if app.is_overlay(OverlayKind::MessageRequest) {
        draw_message_request(frame, app, size);
    }

    // Action menu overlay
    if app.is_overlay(OverlayKind::ActionMenu) {
        draw_action_menu(frame, app, size);
    }

    // Reaction picker overlay
    if app.is_overlay(OverlayKind::ReactionPicker) {
        draw_reaction_picker(frame, app, size);
    }

    // Emoji picker overlay
    if app.is_overlay(OverlayKind::EmojiPicker) {
        draw_emoji_picker(frame, app, size);
    }

    // Delete confirmation overlay
    if app.is_overlay(OverlayKind::DeleteConfirm) {
        draw_delete_confirm(frame, app, size);
    }

    // Theme picker overlay
    if app.is_overlay(OverlayKind::ThemePicker) {
        draw_theme_picker(frame, app, size);
    }

    // Keybindings overlay
    if app.is_overlay(OverlayKind::Keybindings) {
        draw_keybindings(frame, app, size);
    }

    // Settings profile manager overlay
    if app.is_overlay(OverlayKind::SettingsProfiles) {
        draw_settings_profile_manager(frame, app, size);
    }

    // Pin duration picker overlay
    if app.is_overlay(OverlayKind::PinDuration) {
        draw_pin_duration_picker(frame, app, size);
    }

    // Poll vote overlay
    if app.is_overlay(OverlayKind::PollVote) {
        draw_poll_vote_overlay(frame, app, size);
    }

    // About overlay
    if app.is_overlay(OverlayKind::About) {
        draw_about(frame, app, size);
    }

    // Profile editor overlay
    if app.is_overlay(OverlayKind::Profile) {
        draw_profile(frame, app, size);
    }

    // Forward message picker overlay
    if app.is_overlay(OverlayKind::Forward) {
        draw_forward(frame, app, size);
    }

    // Collect link regions from the rendered buffer for OSC 8 injection
    let area = frame.area();
    app.image.link_regions = collect_link_regions(frame.buffer_mut(), area, app.theme.link);

    // Resolve hidden URLs for attachment links (display text has no URI scheme)
    for link in &mut app.image.link_regions {
        if !link.url.contains("://")
            && let Some(url) = app.image.link_url_map.get(&link.text)
        {
            link.url = url.clone();
        }
    }
}

fn draw_chat_area(frame: &mut Frame, app: &mut App, area: Rect) -> Rect {
    let max_input_height = (area.height / 2).max(3);
    let input_height = (app.input_line_count() as u16 + 2).clamp(3, max_input_height);
    let chat_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),               // messages (typing indicator rendered inside)
            Constraint::Length(input_height), // input
        ])
        .split(area);

    let messages_area = chat_layout[0];
    let input_area = chat_layout[1];

    app.mouse.input_area = input_area;
    draw_messages(frame, app, messages_area);
    draw_input(frame, app, input_area);
    input_area
}

fn draw_messages(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = &app.theme;
    let (title_spans, title_right) = match &app.active_conversation {
        Some(id) => {
            let conv = &app.store.conversations[id];
            let prefix = if conv.is_group { " #" } else { " " };
            let mut spans = vec![Span::styled(
                format!("{prefix}{} ", conv.name),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )];

            // Timer indicator when disappearing messages are enabled
            if conv.expiration_timer > 0 {
                let timer_label = format_compact_duration(conv.expiration_timer);
                let icon = if app.nerd_fonts {
                    "\u{F0150}"
                } else {
                    "\u{23F1}"
                };
                spans.push(Span::styled(
                    format!("{icon} {timer_label} "),
                    Style::default().fg(theme.fg_muted),
                ));
            }

            // Trust level indicator (1:1 only)
            if !conv.is_group
                && let Some(trust) = app.identity_trust.get(id)
            {
                match trust {
                    TrustLevel::TrustedVerified => {
                        spans.push(Span::styled(
                            "\u{2713} verified ",
                            Style::default().fg(theme.accent),
                        ));
                    }
                    TrustLevel::Untrusted => {
                        spans.push(Span::styled(
                            "\u{26A0} untrusted ",
                            Style::default().fg(theme.warning),
                        ));
                    }
                    TrustLevel::TrustedUnverified => {} // normal state, no indicator
                }
            }

            // Mute indicator
            let now = chrono::Utc::now();
            if let Some(indicator) = app
                .active_mute(id, now)
                .and_then(|m| m.sidebar_indicator(now))
            {
                spans.push(Span::styled(
                    format!("{} ", indicator.trim_start()),
                    Style::default().fg(theme.fg_muted),
                ));
            }

            // Scroll indicator in title
            let right = if app.scroll.offset > 0 {
                format!(" \u{2191} {} more ", app.scroll.offset)
            } else {
                String::new()
            };
            (spans, right)
        }
        None => (
            vec![Span::styled(
                " siggy ".to_string(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )],
            String::new(),
        ),
    };

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Line::from(title_spans));

    if !title_right.is_empty() {
        block = block
            .title_bottom(Line::from(title_right).alignment(Alignment::Right))
            .title_style(Style::default().fg(theme.accent));
    }

    let full_inner = block.inner(area);
    frame.render_widget(block, area);

    let messages_ref = match &app.active_conversation {
        Some(id) => app.store.conversations.get(id).map(|c| &c.messages),
        None => None,
    };

    // Build pinned message banner text
    let pinned_banner_text: Option<String> = messages_ref.and_then(|msgs| {
        let pinned: Vec<_> = msgs
            .iter()
            .filter(|m| m.is_pinned && !m.is_deleted)
            .collect();
        match pinned.len() {
            0 => None,
            1 => {
                let m = pinned[0];
                // Collapse newlines to spaces for the single-line banner.
                let body: String = m.body.replace('\n', " ").chars().take(80).collect();
                Some(format!("\u{1f4cc} {}: {body}", m.sender))
            }
            n => Some(format!("\u{1f4cc} {n} pinned messages")),
        }
    });

    let (banner_area, inner) = if pinned_banner_text.is_some() && full_inner.height > 2 {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(full_inner);
        (Some(chunks[0]), chunks[1])
    } else {
        (None, full_inner)
    };

    if let Some(ref pin_text) = pinned_banner_text
        && let Some(banner) = banner_area
    {
        let pin_line = Line::from(Span::styled(
            truncate(pin_text, banner.width as usize),
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(Paragraph::new(pin_line), banner);
    }

    app.mouse.messages_area = inner;

    let messages = match &app.active_conversation {
        Some(id) => {
            if let Some(conv) = app.store.conversations.get(id) {
                &conv.messages
            } else {
                app.scroll.focused_time = None;
                app.scroll.focused_index = None;
                return;
            }
        }
        None => {
            draw_welcome(frame, app, inner);
            app.scroll.focused_time = None;
            app.scroll.focused_index = None;
            return;
        }
    };

    let available_height = inner.height as usize;
    let total = messages.len();

    // Build lines from a fixed window of recent messages.
    // app.scroll.offset is NOT included here; it controls the Paragraph scroll position instead.
    // Including it would expand the window by 1 message per scroll increment, growing
    // content_height and base_scroll in lockstep, keeping scroll_y constant (viewport stuck).
    let start = total.saturating_sub(available_height * MSG_WINDOW_MULTIPLIER);
    let visible = &messages[start..total];

    // Get last_read_index for unread marker
    let conv_id = app.active_conversation.as_ref().unwrap();
    let last_read = app.store.last_read_index.get(conv_id).copied().unwrap_or(0);

    let inner_width = inner.width as usize;

    let mut lines: Vec<Line> = Vec::new();
    let mut prev_date: Option<String> = None;

    // Map each line to its source message index (None for separators/markers)
    let mut line_msg_idx: Vec<Option<usize>> = Vec::new();

    // Track images for native protocol overlay: (first_line_index, line_count, path)
    let use_native =
        app.image.image_mode == "native" && app.image.image_protocol != ImageProtocol::Halfblock;
    let mut image_records: Vec<(usize, usize, String)> = Vec::new();

    for (i, msg) in visible.iter().enumerate() {
        let msg_index = start + i;

        // Date separator: detect day boundary
        if app.date_separators {
            let local = msg.timestamp.with_timezone(&chrono::Local);
            let date_str = local.format("%Y-%m-%d").to_string();
            if prev_date.as_ref() != Some(&date_str) {
                if prev_date.is_some() {
                    let today = chrono::Local::now().date_naive();
                    let msg_date = local.date_naive();
                    let friendly = if msg_date == today {
                        "Today".to_string()
                    } else if msg_date == today.pred_opt().unwrap_or(today) {
                        "Yesterday".to_string()
                    } else {
                        local.format("%b %-d, %Y").to_string()
                    };
                    let label = format!(" {friendly} ");
                    lines.push(build_separator(
                        &label,
                        inner_width,
                        Style::default().fg(theme.fg_muted),
                    ));
                    line_msg_idx.push(None);
                }
                prev_date = Some(date_str);
            }
        }

        // Unread marker: between last_read - 1 and last_read
        if msg_index == last_read && last_read > 0 && last_read < total {
            lines.push(build_separator(
                " new messages ",
                inner_width,
                Style::default()
                    .fg(theme.error)
                    .add_modifier(Modifier::BOLD),
            ));
            line_msg_idx.push(None);
        }

        if msg.is_system {
            let body = if app.reactions.emoji_to_text {
                emoji_to_text(&msg.body)
            } else {
                msg.body.clone()
            };
            lines.push(Line::from(Span::styled(
                format!("  {body}"),
                Style::default().fg(theme.system_msg),
            )));
            line_msg_idx.push(Some(msg_index));
        } else {
            // Render quoted reply line above message
            if let Some(ref quote) = msg.quote {
                let raw_body = if app.reactions.emoji_to_text {
                    emoji_to_text(&quote.body)
                } else {
                    quote.body.clone()
                };
                // Quotes render on a single line; collapse any newlines to spaces.
                let raw_body = raw_body.replace('\n', " ");
                let quote_body = truncate(&raw_body, 50);
                lines.push(Line::from(vec![
                    Span::styled("  \u{256D} ", Style::default().fg(theme.quote)),
                    Span::styled(
                        format!("<{}>", quote.author),
                        Style::default()
                            .fg(sender_color(&quote.author, theme))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" {quote_body}"), Style::default().fg(theme.quote)),
                ]));
                line_msg_idx.push(Some(msg_index));
            }

            let time = msg.format_time();
            let mut spans = Vec::new();

            // Status symbol for outgoing messages (before timestamp)
            if app.show_receipts
                && let Some(status) = msg.status
            {
                let (sym, color) = status_symbol(status, app.nerd_fonts, app.color_receipts, theme);
                spans.push(Span::styled(format!("{sym} "), Style::default().fg(color)));
            }

            if msg.expires_in_seconds > 0 {
                let icon = if app.nerd_fonts {
                    "\u{F0150}"
                } else {
                    "\u{23F1}"
                };
                spans.push(Span::styled(
                    format!("{icon} [{}] ", time),
                    Style::default().fg(theme.fg_muted),
                ));
            } else {
                spans.push(Span::styled(
                    format!("[{}] ", time),
                    Style::default().fg(theme.fg_muted),
                ));
            }
            spans.push(Span::styled(
                format!("<{}>", msg.sender),
                Style::default()
                    .fg(sender_color(&msg.sender, theme))
                    .add_modifier(Modifier::BOLD),
            ));

            // "(edited)" label
            if msg.is_edited {
                spans.push(Span::styled(
                    " (edited)",
                    Style::default()
                        .fg(theme.fg_muted)
                        .add_modifier(Modifier::ITALIC),
                ));
            }

            // "(pinned)" label
            if msg.is_pinned {
                spans.push(Span::styled(
                    " (pinned)",
                    Style::default()
                        .fg(theme.warning)
                        .add_modifier(Modifier::ITALIC),
                ));
            }

            if msg.is_deleted {
                // Deleted message body
                spans.push(Span::styled(
                    " [deleted]",
                    Style::default()
                        .fg(theme.fg_muted)
                        .add_modifier(Modifier::ITALIC),
                ));
                lines.push(Line::from(spans));
                line_msg_idx.push(Some(msg_index));
            } else {
                // Style URIs and @mentions
                let (body_spans, hidden_url) =
                    styled_uri_spans(&msg.body, &msg.mention_ranges, &msg.style_ranges, theme);
                if let Some(url) = hidden_url {
                    // Collect display text for link_url_map lookup
                    let display_text: String =
                        body_spans.iter().map(|s| s.content.as_ref()).collect();
                    app.image.link_url_map.insert(display_text, url);
                }
                let body_spans: Vec<Span<'static>> = if app.reactions.emoji_to_text {
                    body_spans
                        .into_iter()
                        .map(|s| Span::styled(emoji_to_text(&s.content), s.style))
                        .collect()
                } else {
                    body_spans
                };
                // Multi-line bodies: first line joins the header, each subsequent
                // line gets a continuation indent.
                let body_lines = split_spans_by_newline(body_spans);
                spans.push(Span::raw(" ".to_string()));
                if let Some(first) = body_lines.first() {
                    spans.extend(first.iter().cloned());
                }
                lines.push(Line::from(spans));
                line_msg_idx.push(Some(msg_index));
                const CONT_INDENT: &str = "  ";
                for body_line in body_lines.iter().skip(1) {
                    let mut cont_spans: Vec<Span<'static>> =
                        vec![Span::raw(CONT_INDENT.to_string())];
                    cont_spans.extend(body_line.iter().cloned());
                    lines.push(Line::from(cont_spans));
                    line_msg_idx.push(Some(msg_index));
                }
            }

            // Render inline image preview if available (skip for deleted, skip if images disabled)
            if !msg.is_deleted
                && app.image.image_mode != "none"
                && let Some(ref image_lines) = msg.image_lines
            {
                let first_idx = lines.len();
                let count = image_lines.len();
                for line in image_lines {
                    lines.push(line.clone());
                    line_msg_idx.push(Some(msg_index));
                }
                // Record for native protocol overlay
                if use_native && let Some(ref path) = msg.image_path {
                    image_records.push((first_idx, count, path.clone()));
                }
            }

            // Render link preview block
            if !msg.is_deleted
                && app.image.show_link_previews
                && let Some(ref preview) = msg.preview
            {
                if let Some(ref title) = preview.title {
                    lines.push(Line::from(vec![
                        Span::styled("  \u{251C} ", Style::default().fg(theme.link)),
                        Span::styled(
                            truncate(title, 60),
                            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
                        ),
                    ]));
                    line_msg_idx.push(Some(msg_index));
                }
                if let Some(ref desc) = preview.description {
                    // Description is a middle line; URL always follows
                    lines.push(Line::from(vec![
                        Span::styled("  \u{251C} ", Style::default().fg(theme.link)),
                        Span::styled(truncate(desc, 60), Style::default().fg(theme.fg_muted)),
                    ]));
                    line_msg_idx.push(Some(msg_index));
                }
                lines.push(Line::from(vec![
                    Span::styled("  \u{2570} ", Style::default().fg(theme.link)),
                    Span::styled(
                        truncate(&preview.url, 60),
                        Style::default()
                            .fg(theme.link)
                            .add_modifier(Modifier::UNDERLINED),
                    ),
                ]));
                line_msg_idx.push(Some(msg_index));

                // Render link preview thumbnail (only when images enabled)
                if app.image.image_mode != "none"
                    && let Some(ref img_lines) = msg.preview_image_lines
                {
                    let first_idx = lines.len();
                    let count = img_lines.len();
                    for line in img_lines {
                        lines.push(line.clone());
                        line_msg_idx.push(Some(msg_index));
                    }
                    if use_native && let Some(ref path) = msg.preview_image_path {
                        image_records.push((first_idx, count, path.clone()));
                    }
                }
            }

            // Render inline poll display
            if !msg.is_deleted
                && let Some(ref poll_data) = msg.poll_data
            {
                let poll_lines =
                    build_poll_display(poll_data, &msg.poll_votes, &app.account, theme);
                for line in poll_lines {
                    lines.push(line);
                    line_msg_idx.push(Some(msg_index));
                }
            }

            // Render reaction summary line (skip for deleted or when reactions hidden)
            if app.reactions.show_reactions && !msg.is_deleted && !msg.reactions.is_empty() {
                lines.push(build_reaction_summary(
                    &msg.reactions,
                    app.reactions.verbose,
                    app.reactions.emoji_to_text,
                    theme,
                ));
                line_msg_idx.push(Some(msg_index));
            }
        }
    }

    // Append typing indicator as the last line inside the message area
    if let Some(ref conv_id) = app.active_conversation {
        let typers: Vec<String> = app
            .typing
            .indicators
            .get(conv_id)
            .map(|senders| {
                senders
                    .keys()
                    .map(|sender| {
                        if let Some(name) = app.store.contact_names.get(sender) {
                            name.clone()
                        } else if let Some(conv) = app.store.conversations.get(sender) {
                            conv.name.clone()
                        } else {
                            sender.clone()
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        if !typers.is_empty() {
            let text = if typers.len() == 1 {
                format!("  {} is typing...", typers[0])
            } else {
                format!("  {} are typing...", typers.join(", "))
            };
            lines.push(Line::from(Span::styled(
                text,
                Style::default()
                    .fg(theme.fg_muted)
                    .add_modifier(Modifier::ITALIC),
            )));
            line_msg_idx.push(None);
        }
    }

    // Compute actual content height using ratatui's word-wrap algorithm so that
    // image-position calculations below align with how the Paragraph widget
    // actually renders. A character-based div_ceil approximation diverges from
    // WordWrapper on realistic text and shifts Kitty placeholder cells off their
    // halfblock origins, which caused images to clip into neighboring messages.
    let inner_w_u16 = inner.width.max(1);
    let line_heights: Vec<usize> = lines
        .iter()
        .map(|line| {
            Paragraph::new(line.clone())
                .wrap(Wrap { trim: false })
                .line_count(inner_w_u16)
                .max(1)
        })
        .collect();
    let content_height: usize = line_heights.iter().sum();

    // Bottom-align by default; app.scroll.offset shifts the view upward
    let base_scroll = content_height.saturating_sub(available_height);
    app.scroll.offset = app.scroll.offset.min(base_scroll);
    let mut scroll_y = base_scroll - app.scroll.offset;

    // Signal when user has scrolled to the top of loaded content
    app.scroll.at_top = app.scroll.offset >= base_scroll
        && base_scroll > 0
        && app
            .active_conversation
            .as_ref()
            .is_some_and(|id| app.store.has_more_messages.contains(id));

    // Determine the focused message for highlight and full-timestamp display in Normal mode.
    // Check scroll.focused_index too so J/K navigation works even when content fits the viewport
    // (base_scroll == 0 clamps scroll.offset to 0, but J/K focus should persist).
    //
    // `render_focus` is used for highlighting; it may differ from app.scroll.focused_index when
    // j/k line-scrolling (where we derive focus for display but don't persist it, to avoid
    // the "ensure visible" logic snapping the viewport back on the next frame).
    let render_focus;
    if app.mode == InputMode::Normal
        && (app.scroll.offset > 0 || app.scroll.focused_index.is_some())
    {
        if let Some(fi) = app.scroll.focused_index {
            // J/K already set scroll.focused_index — ensure it's visible by adjusting scroll.
            let mut msg_start: Option<usize> = None;
            let mut msg_end = 0usize;
            let mut cumul = 0usize;
            for (idx, &h) in line_heights.iter().enumerate() {
                if line_msg_idx.get(idx) == Some(&Some(fi)) {
                    if msg_start.is_none() {
                        msg_start = Some(cumul);
                    }
                    msg_end = cumul + h;
                }
                cumul += h;
            }
            if let Some(start) = msg_start {
                if start < scroll_y {
                    // Message is above viewport — scroll up
                    app.scroll.offset = base_scroll.saturating_sub(start);
                    scroll_y = base_scroll - app.scroll.offset;
                } else if msg_end > scroll_y + available_height {
                    // Message is below viewport — scroll down
                    let new_scroll_y = msg_end.saturating_sub(available_height);
                    app.scroll.offset = base_scroll.saturating_sub(new_scroll_y);
                    scroll_y = base_scroll - app.scroll.offset;
                }
            }
            app.scroll.focused_time = messages.get(fi).map(|m| m.timestamp);
            render_focus = Some(fi);
        } else {
            // Viewport-only scroll (Ctrl-E/Y, Ctrl-D/U) — no highlight without explicit focus.
            render_focus = None;
        }
    } else {
        app.scroll.focused_index = None;
        app.scroll.focused_time = None;
        render_focus = None;
    };

    // Compute screen positions for native protocol image overlay (before lines is consumed)
    if !image_records.is_empty() {
        // Build cumulative wrapped-line positions from the pre-computed heights so
        // that image placements line up exactly with Paragraph's rendered rows.
        let mut wrapped_positions: Vec<usize> = Vec::with_capacity(lines.len() + 1);
        let mut cumulative = 0usize;
        for &h in &line_heights {
            wrapped_positions.push(cumulative);
            cumulative += h;
        }

        for (first_idx, count, path) in &image_records {
            let img_start = wrapped_positions[*first_idx];
            let img_end = if first_idx + count < wrapped_positions.len() {
                wrapped_positions[first_idx + count]
            } else {
                cumulative
            };

            let screen_start = img_start as i64 - scroll_y as i64;
            let screen_end = img_end as i64 - scroll_y as i64;

            // Skip if entirely outside visible area
            if screen_end <= 0 || screen_start >= available_height as i64 {
                continue;
            }

            // Clip to visible area
            let vis_start = screen_start.max(0) as u16;
            let vis_end = (screen_end.min(available_height as i64)) as u16;

            if vis_start < vis_end {
                // Image width = first image line width minus 2-char indent
                let img_width = if *first_idx < lines.len() {
                    (lines[*first_idx].width()).saturating_sub(2) as u16
                } else {
                    0
                };

                let full_height = (img_end - img_start) as u16;
                let crop_top = (vis_start as i64 - screen_start) as u16;

                app.image.visible_images.push(VisibleImage {
                    x: inner.x + 2, // account for 2-char indent
                    y: inner.y + vis_start,
                    width: img_width,
                    height: vis_end - vis_start,
                    full_height,
                    crop_top,
                    path: path.clone(),
                });
            }
        }
    }

    // Highlight all lines belonging to the focused message
    if let Some(focused_idx) = render_focus {
        for (i, line) in lines.iter_mut().enumerate() {
            if line_msg_idx.get(i) == Some(&Some(focused_idx)) {
                let patched: Vec<Span> = line
                    .spans
                    .drain(..)
                    .map(|mut s| {
                        s.style = s.style.bg(theme.msg_selected_bg);
                        s
                    })
                    .collect();
                *line = Line::from(patched);
            }
        }
    }

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll_y as u16, 0));
    frame.render_widget(paragraph, inner);

    if use_native && app.image.image_protocol == ImageProtocol::Kitty {
        patch_kitty_placeholders(frame, app);
    }
    // Note: Sixel does NOT use set_skip. ratatui writes halfblock at image cells,
    // which clears stale Sixel pixels from previous positions when images scroll.
    // Sixel is then overlaid outside the synchronized update (see main.rs).

    // Scrollbar on right border, inset to preserve rounded corners
    if content_height > available_height {
        let scrollbar_area = Rect::new(
            area.x + area.width.saturating_sub(1),
            area.y + 1,
            1,
            area.height.saturating_sub(2),
        );
        let mut scrollbar_state = ScrollbarState::new(base_scroll).position(scroll_y);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

/// Patch ratatui buffer cells with Kitty Unicode Placeholder characters.
///
/// Replaces the halfblock cells with U+10EEEE + row/column diacritics so the
/// terminal renders image data at the cell level (instead of GPU overlays).
fn patch_kitty_placeholders(frame: &mut Frame, app: &mut App) {
    for img in &app.image.visible_images {
        let id = if let Some(&existing) = app.image.kitty_image_ids.get(&img.path) {
            existing
        } else {
            let new_id = app.image.next_kitty_image_id;
            app.image.next_kitty_image_id += 1;
            app.image.kitty_image_ids.insert(img.path.clone(), new_id);
            new_id
        };
        let fg = image_render::kitty_id_color(id);

        for row_offset in 0..img.height {
            let image_row = (img.crop_top + row_offset) as usize;
            for col in 0..img.width {
                let symbol = image_render::placeholder_symbol(image_row, col as usize);
                let pos = Position::new(img.x + col, img.y + row_offset);
                if let Some(cell) = frame.buffer_mut().cell_mut(pos) {
                    cell.reset();
                    cell.set_symbol(&symbol);
                    cell.set_fg(fg);
                }
            }
        }

        if !app.image.kitty_transmitted.contains(&id) {
            app.image.kitty_pending_transmits.push((
                id,
                img.path.clone(),
                img.width,
                img.full_height,
            ));
        }
    }
}

/// Build a reaction summary line like "    👍 2  ❤️ 1  😂 1"
pub(crate) fn build_reaction_summary(
    reactions: &[Reaction],
    verbose: bool,
    convert_emoji: bool,
    theme: &Theme,
) -> Line<'static> {
    let display = |emoji: &str| -> String {
        if convert_emoji {
            emoji_to_text(emoji)
        } else {
            emoji.to_string()
        }
    };
    if verbose {
        // Verbose: group by emoji, show sender names
        let mut grouped: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for r in reactions {
            grouped
                .entry(r.emoji.clone())
                .or_default()
                .push(r.sender.clone());
        }
        let mut spans = vec![Span::raw("    ".to_string())];
        for (emoji, senders) in &grouped {
            spans.push(Span::raw(format!("{} ", display(emoji))));
            spans.push(Span::styled(
                senders.join(", "),
                Style::default().fg(theme.fg_muted),
            ));
            spans.push(Span::raw("  ".to_string()));
        }
        Line::from(spans)
    } else {
        // Summary: emoji + count
        let mut counts: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for r in reactions {
            *counts.entry(r.emoji.clone()).or_default() += 1;
        }
        let mut spans = vec![Span::raw("    ".to_string())];
        for (emoji, count) in &counts {
            spans.push(Span::raw(display(emoji)));
            spans.push(Span::styled(
                format!(" {count}  "),
                Style::default().fg(theme.fg_muted),
            ));
        }
        Line::from(spans)
    }
}

/// Render the welcome/empty-state screen when no conversation is active.
fn draw_welcome(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let mut lines = vec![Line::from("")];

    if let Some(ref err) = app.connection_error {
        lines.push(Line::from(Span::styled(
            "  Connection Error",
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            format!("  {err}"),
            Style::default().fg(theme.error),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Run with --setup to reconfigure.",
            Style::default().fg(theme.fg_secondary),
        )));
    } else if app.loading {
        const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let spinner_char = SPINNER[app.spinner_tick % SPINNER.len()];
        lines.push(Line::from(Span::styled(
            "  siggy",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  {spinner_char} {}", app.startup_status),
            Style::default().fg(theme.fg_muted),
        )));
    } else if app.store.conversation_order.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Welcome to siggy",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  No conversations yet",
            Style::default().fg(theme.fg_secondary),
        )));
        lines.push(Line::from(Span::styled(
            "  Messages you send and receive will appear here.",
            Style::default().fg(theme.fg_secondary),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  /join +1234567890  message someone by phone number",
            Style::default().fg(theme.fg_secondary),
        )));
        lines.push(Line::from(Span::styled(
            "  /contacts          browse your synced contacts",
            Style::default().fg(theme.fg_secondary),
        )));
        lines.push(Line::from(Span::styled(
            "  /help              see all commands and keybindings",
            Style::default().fg(theme.fg_secondary),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  Welcome to siggy",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Getting started",
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            "  Tab / Shift+Tab    cycle through conversations",
            Style::default().fg(theme.fg_secondary),
        )));
        lines.push(Line::from(Span::styled(
            "  /join <contact>    open a conversation by name or number",
            Style::default().fg(theme.fg_secondary),
        )));
        lines.push(Line::from(Span::styled(
            "  Esc                switch to Normal mode (vim keys)",
            Style::default().fg(theme.fg_secondary),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Useful commands",
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            "  /contacts          browse synced contacts",
            Style::default().fg(theme.fg_secondary),
        )));
        lines.push(Line::from(Span::styled(
            "  /settings          configure preferences",
            Style::default().fg(theme.fg_secondary),
        )));
        lines.push(Line::from(Span::styled(
            "  /help              all commands and keybindings",
            Style::default().fg(theme.fg_secondary),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Ctrl+\u{2190}/\u{2192} to resize sidebar",
            Style::default().fg(theme.fg_muted),
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Find the message index at the bottom of the visible viewport.
/// Returns the index into the conversation's messages Vec.
fn draw_autocomplete(frame: &mut Frame, app: &App, input_area: Rect) {
    let theme = &app.theme;
    let terminal_width = frame.area().width;
    let mut lines: Vec<Line> = Vec::new();
    let mut max_content_width: usize = 0;

    match app.autocomplete.mode {
        AutocompleteMode::Command => {
            for (i, &cmd_idx) in app.autocomplete.command_candidates.iter().enumerate() {
                let cmd = &COMMANDS[cmd_idx];
                let args_part = if cmd.args.is_empty() {
                    String::new()
                } else {
                    format!(" {}", cmd.args)
                };
                let left = format!("  {}{}", cmd.name, args_part);
                let right = format!("  {}", cmd.description);
                let total_len = left.len() + right.len() + 2;
                if total_len > max_content_width {
                    max_content_width = total_len;
                }

                let is_selected = i == app.autocomplete.index;
                let style = if is_selected {
                    Style::default()
                        .bg(theme.bg_selected)
                        .fg(theme.fg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.fg_secondary)
                };
                let desc_style = if is_selected {
                    Style::default().bg(theme.bg_selected).fg(theme.accent)
                } else {
                    Style::default().fg(theme.fg_muted)
                };

                lines.push(Line::from(vec![
                    Span::styled(left, style),
                    Span::styled(right, desc_style),
                ]));
            }
        }
        AutocompleteMode::Mention => {
            for (i, (phone, name, _uuid)) in app.autocomplete.mention_candidates.iter().enumerate()
            {
                let left = format!("  @{name}");
                let right = format!("  {phone}");
                let total_len = left.len() + right.len() + 2;
                if total_len > max_content_width {
                    max_content_width = total_len;
                }

                let is_selected = i == app.autocomplete.index;
                let style = if is_selected {
                    Style::default()
                        .bg(theme.bg_selected)
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.accent)
                };
                let phone_style = if is_selected {
                    Style::default().bg(theme.bg_selected).fg(theme.fg_muted)
                } else {
                    Style::default().fg(theme.fg_muted)
                };

                lines.push(Line::from(vec![
                    Span::styled(left, style),
                    Span::styled(right, phone_style),
                ]));
            }
        }
        AutocompleteMode::Join => {
            for (i, (display, _value)) in app.autocomplete.join_candidates.iter().enumerate() {
                let left = format!("  {display}");
                let total_len = left.len() + 2;
                if total_len > max_content_width {
                    max_content_width = total_len;
                }

                let is_selected = i == app.autocomplete.index;
                let style = if is_selected {
                    Style::default()
                        .bg(theme.bg_selected)
                        .fg(theme.success)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.success)
                };

                lines.push(Line::from(vec![Span::styled(left, style)]));
            }
        }
    }

    let count = lines.len();

    // Size the popup, clamping to available space
    let terminal_height = frame.area().height;
    let popup_width = (max_content_width as u16 + 2)
        .min(terminal_width.saturating_sub(2))
        .max(20);
    let popup_height = ((count as u16) + 2).min(input_area.y).min(terminal_height); // +2 for border
    if popup_height < 3 {
        return; // not enough space to render anything useful
    }

    // Position above the input box, left-aligned with it
    let x = input_area.x;
    let y = input_area.y.saturating_sub(popup_height);

    let area = Rect::new(
        x,
        y,
        popup_width.min(terminal_width.saturating_sub(x)),
        popup_height,
    );
    lines.truncate((popup_height.saturating_sub(2)) as usize);

    // Clear the area behind the popup so chat text doesn't leak through
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.bg));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, area);
}

/// Format a safety number string as groups of 5 digits, 6 per line.
/// Format a file size in human-readable form (B, K, M, G).
pub(crate) fn build_poll_display(
    poll: &PollData,
    votes: &[PollVote],
    own_account: &str,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    let option_count = poll.options.len();
    let mut counts = vec![0usize; option_count];
    let mut own_selections: Vec<bool> = vec![false; option_count];

    for vote in votes {
        for &idx in &vote.option_indexes {
            if (idx as usize) < option_count {
                counts[idx as usize] += 1;
            }
        }
        if vote.voter == own_account {
            for &idx in &vote.option_indexes {
                if (idx as usize) < option_count {
                    own_selections[idx as usize] = true;
                }
            }
        }
    }
    let total_votes: usize = counts.iter().sum();

    let bar_width = 10;

    for (i, opt) in poll.options.iter().enumerate() {
        let count = counts[i];
        let pct = (count * 100).checked_div(total_votes).unwrap_or(0);
        let filled = (count * bar_width).checked_div(total_votes).unwrap_or(0);
        let empty = bar_width - filled;

        let bar: String = "\u{2588}".repeat(filled) + &"\u{2591}".repeat(empty);

        let voted_marker = if own_selections[i] { "\u{2713} " } else { "  " };
        let text_style = if own_selections[i] {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };

        let label = if opt.text.chars().count() > 12 {
            let truncated: String = opt.text.chars().take(11).collect();
            format!("{truncated}\u{2026}")
        } else {
            opt.text.clone()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {voted_marker}"), text_style),
            Span::styled(format!("{:<12}", label), text_style),
            Span::styled(bar, Style::default().fg(theme.accent)),
            Span::styled(
                format!("  {count} ({pct}%)"),
                Style::default().fg(theme.fg_muted),
            ),
        ]));
    }

    let mode = if poll.allow_multiple {
        "multi-select"
    } else {
        "single choice"
    };
    let status = if poll.closed { " [CLOSED]" } else { "" };
    lines.push(Line::from(Span::styled(
        format!("    {total_votes} votes \u{00b7} {mode}{status}"),
        Style::default().fg(theme.fg_muted),
    )));

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::types::{MessageStatus, PollData, PollOption, PollVote, Reaction};
    use crate::theme::default_theme;
    use rstest::rstest;

    // --- sender_color ---

    #[test]
    fn sender_color_you_returns_self() {
        let theme = default_theme();
        assert_eq!(sender_color("you", &theme), theme.sender_self);
    }

    #[test]
    fn sender_color_deterministic() {
        let theme = default_theme();
        let c1 = sender_color("Alice", &theme);
        let c2 = sender_color("Alice", &theme);
        assert_eq!(c1, c2);
    }

    #[test]
    fn sender_color_in_palette() {
        let theme = default_theme();
        let c = sender_color("Bob", &theme);
        assert!(theme.sender_palette.contains(&c));
    }

    // --- truncate ---

    #[rstest]
    #[case("hi", 10, "hi")]
    #[case("hello", 5, "hello")]
    #[case("hello world", 5, "hell\u{2026}")]
    #[case("abc", 1, "\u{2026}")]
    #[case("abc", 0, "\u{2026}")]
    #[case("", 5, "")]
    fn truncate_cases(#[case] input: &str, #[case] max: usize, #[case] expected: &str) {
        assert_eq!(truncate(input, max), expected);
    }

    // --- status_symbol ---

    #[rstest]
    #[case(MessageStatus::Failed, "\u{2717}")]
    #[case(MessageStatus::Sending, "\u{25cc}")]
    #[case(MessageStatus::Sent, "\u{25cb}")]
    #[case(MessageStatus::Delivered, "\u{2713}")]
    #[case(MessageStatus::Read, "\u{25cf}")]
    #[case(MessageStatus::Viewed, "\u{25c9}")]
    fn status_symbol_variants(#[case] status: MessageStatus, #[case] expected_sym: &str) {
        let theme = default_theme();
        let (sym, _) = status_symbol(status, false, true, &theme);
        assert_eq!(sym, expected_sym);
    }

    #[test]
    fn status_symbol_color_vs_muted() {
        let theme = default_theme();
        let (_, colored) = status_symbol(MessageStatus::Read, false, true, &theme);
        let (_, muted) = status_symbol(MessageStatus::Read, false, false, &theme);
        assert_eq!(colored, theme.receipt_read);
        assert_eq!(muted, theme.fg_muted);
    }

    // --- build_separator ---

    #[test]
    fn build_separator_pads() {
        let theme = default_theme();
        let line = build_separator(" Jan 1 ", 40, Style::default().fg(theme.fg_muted));
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(text.chars().count(), 40);
        assert!(text.contains("Jan 1"));
    }

    // --- extract_url ---

    #[rstest]
    #[case("https://example.com", "https://example.com")]
    #[case("http://foo.bar/baz", "http://foo.bar/baz")]
    #[case("file:///tmp/a.txt", "file:///tmp/a.txt")]
    #[case("check https://x.com/path here", "https://x.com/path")]
    #[case("no-scheme.com", "no-scheme.com")]
    fn extract_url_cases(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(extract_url(input), expected);
    }

    // --- build_reaction_summary ---

    #[test]
    fn reaction_summary_counts() {
        let theme = default_theme();
        let reactions = vec![
            Reaction {
                emoji: "\u{1f44d}".to_string(),
                sender: "Alice".to_string(),
            },
            Reaction {
                emoji: "\u{1f44d}".to_string(),
                sender: "Bob".to_string(),
            },
        ];
        let line = build_reaction_summary(&reactions, false, false, &theme);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("2"), "expected count '2' in: {text}");
    }

    #[test]
    fn reaction_summary_verbose_names() {
        let theme = default_theme();
        let reactions = vec![Reaction {
            emoji: "\u{2764}".to_string(),
            sender: "Alice".to_string(),
        }];
        let line = build_reaction_summary(&reactions, true, false, &theme);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("Alice"), "expected sender name in: {text}");
    }

    #[test]
    fn reaction_summary_empty() {
        let theme = default_theme();
        let line = build_reaction_summary(&[], false, false, &theme);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(text.trim(), "");
    }

    // --- build_poll_display ---

    #[test]
    fn poll_display_basic() {
        let theme = default_theme();
        let poll = PollData {
            question: "Favorite?".to_string(),
            options: vec![
                PollOption {
                    id: 0,
                    text: "A".to_string(),
                },
                PollOption {
                    id: 1,
                    text: "B".to_string(),
                },
            ],
            allow_multiple: false,
            closed: false,
        };
        let votes = vec![
            PollVote {
                voter: "+1".to_string(),
                voter_name: None,
                option_indexes: vec![0],
                vote_count: 1,
            },
            PollVote {
                voter: "+2".to_string(),
                voter_name: None,
                option_indexes: vec![0],
                vote_count: 1,
            },
        ];
        let lines = build_poll_display(&poll, &votes, "+99", &theme);
        assert_eq!(lines.len(), 3);
        let summary: String = lines
            .last()
            .unwrap()
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(summary.contains("votes"), "expected 'votes' in: {summary}");
    }

    #[test]
    fn poll_display_own_vote_marked() {
        let theme = default_theme();
        let poll = PollData {
            question: "Q?".to_string(),
            options: vec![PollOption {
                id: 0,
                text: "Yes".to_string(),
            }],
            allow_multiple: false,
            closed: false,
        };
        let votes = vec![PollVote {
            voter: "+me".to_string(),
            voter_name: None,
            option_indexes: vec![0],
            vote_count: 1,
        }];
        let lines = build_poll_display(&poll, &votes, "+me", &theme);
        let option_text: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(
            option_text.contains("\u{2713}"),
            "expected checkmark in: {option_text}"
        );
    }

    #[test]
    fn poll_display_closed() {
        let theme = default_theme();
        let poll = PollData {
            question: "Q?".to_string(),
            options: vec![PollOption {
                id: 0,
                text: "X".to_string(),
            }],
            allow_multiple: false,
            closed: true,
        };
        let lines = build_poll_display(&poll, &[], "+me", &theme);
        let summary: String = lines
            .last()
            .unwrap()
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(
            summary.contains("[CLOSED]"),
            "expected [CLOSED] in: {summary}"
        );
    }

    #[test]
    fn poll_display_no_votes() {
        let theme = default_theme();
        let poll = PollData {
            question: "Q?".to_string(),
            options: vec![PollOption {
                id: 0,
                text: "A".to_string(),
            }],
            allow_multiple: false,
            closed: false,
        };
        let lines = build_poll_display(&poll, &[], "+me", &theme);
        let option_text: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(
            option_text.contains("0 (0%)"),
            "expected '0 (0%)' in: {option_text}"
        );
        let summary: String = lines
            .last()
            .unwrap()
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert!(
            summary.contains("0 votes"),
            "expected '0 votes' in: {summary}"
        );
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::app::{App, InputMode, PinPending};
    use crate::db::Database;
    use crate::domain::EmojiPickerSource;
    use crate::image_render::ImageProtocol;
    use chrono::NaiveDate;
    use ratatui::{Terminal, backend::TestBackend};

    /// Fixed date for deterministic timestamps in snapshots.
    fn fixed_date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2025, 1, 15).unwrap()
    }

    /// Create a fully-populated demo App with deterministic data.
    fn demo_app() -> App {
        let db = Database::open_in_memory().unwrap();
        let mut app = App::new("+15559999999".to_string(), db);
        app.connected = true;
        app.loading = false;
        app.is_demo = true;
        app.date_separators = false;
        app.image.image_protocol = ImageProtocol::Halfblock;
        app.populate_demo_data(fixed_date());
        app
    }

    /// Render the app into a TestBackend and return the buffer contents as a string.
    fn render_to_string(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut output = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                let cell = &buffer[(x, y)];
                output.push_str(cell.symbol());
            }
            // Trim trailing spaces for cleaner snapshots
            let trimmed = output.trim_end();
            output.truncate(trimmed.len());
            output.push('\n');
        }
        output
    }

    #[test]
    fn test_sidebar_layout() {
        let mut app = demo_app();
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_chat_messages() {
        let mut app = demo_app();
        // Alice is already the active conversation
        assert_eq!(app.active_conversation.as_deref(), Some("+15550001111"));
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn body_newlines_render_as_separate_lines() {
        use crate::conversation_store::DisplayMessage;
        let mut app = demo_app();
        let conv_id = app.active_conversation.clone().unwrap();
        if let Some(conv) = app.store.conversations.get_mut(&conv_id) {
            conv.messages.clear();
            conv.messages.push(DisplayMessage {
                sender: "Alice".to_string(),
                timestamp: chrono::Utc::now(),
                body: "line one\nline two".to_string(),
                is_system: false,
                image_lines: None,
                image_path: None,
                status: None,
                timestamp_ms: 1_700_000_000_000,
                reactions: Vec::new(),
                mention_ranges: Vec::new(),
                style_ranges: Vec::new(),
                body_raw: None,
                mentions: Vec::new(),
                quote: None,
                is_edited: false,
                is_deleted: false,
                is_pinned: false,
                sender_id: "+15550001111".to_string(),
                expires_in_seconds: 0,
                expiration_start_ms: 0,
                poll_data: None,
                poll_votes: Vec::new(),
                preview: None,
                preview_image_lines: None,
                preview_image_path: None,
            });
        }
        let output = render_to_string(&mut app, 100, 30);
        for row in output.lines() {
            assert!(
                !(row.contains("line one") && row.contains("line two")),
                "body text should split across rows; got row: {row:?}\nfull output:\n{output}"
            );
        }
        assert!(
            output.contains("line one") && output.contains("line two"),
            "expected both body lines to appear; got:\n{output}"
        );
    }

    #[test]
    fn test_normal_vs_insert_mode() {
        let mut app = demo_app();

        app.mode = InputMode::Insert;
        let insert_output = render_to_string(&mut app, 100, 30);

        app.mode = InputMode::Normal;
        let normal_output = render_to_string(&mut app, 100, 30);

        // They should differ (mode indicator in status bar)
        assert_ne!(insert_output, normal_output);
        insta::assert_snapshot!("insert_mode", insert_output);
        insta::assert_snapshot!("normal_mode", normal_output);
    }

    #[test]
    fn test_no_active_conversation() {
        let mut app = demo_app();
        app.active_conversation = None;
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_help_overlay() {
        let mut app = demo_app();
        app.open_overlay(OverlayKind::Help);
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_narrow_terminal() {
        let mut app = demo_app();
        // Below SIDEBAR_AUTO_HIDE_WIDTH (60), sidebar should auto-hide
        let output = render_to_string(&mut app, 50, 20);
        insta::assert_snapshot!(output);
    }

    // --- Phase 2: Message features ---
    // Note: quote replies, link previews, edited messages, and reactions are all
    // covered by test_chat_messages (Alice conversation contains all of these).

    #[test]
    fn test_styled_text() {
        // Bob conversation: bold and monospace styled text
        let mut app = demo_app();
        app.active_conversation = Some("+15550002222".to_string());
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_poll() {
        // Rust Devs group: poll rendering with question, options, vote counts
        let mut app = demo_app();
        app.active_conversation = Some("group_rustdevs".to_string());
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_pinned_message() {
        // Rust Devs group: "(pinned)" label on the pinned message
        let mut app = demo_app();
        app.active_conversation = Some("group_rustdevs".to_string());
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_unread_marker() {
        // Family group has 2 unread out of 5 messages, last_read_index = 3
        let mut app = demo_app();
        app.active_conversation = Some("group_family".to_string());
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    // --- Phase 3: Overlays ---

    #[test]
    fn test_settings_overlay() {
        let mut app = demo_app();
        app.open_overlay(OverlayKind::Settings);
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_about_overlay() {
        let mut app = demo_app();
        app.open_overlay(OverlayKind::About);
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    // --- Phase 4: Edge cases ---

    #[test]
    fn test_empty_conversation() {
        use crate::app::Conversation;
        let mut app = demo_app();
        let empty_id = "+15550009999".to_string();
        app.store.conversations.insert(
            empty_id.clone(),
            Conversation {
                name: "Empty".to_string(),
                id: empty_id.clone(),
                messages: Vec::new(),
                unread: 0,
                is_group: false,
                expiration_timer: 0,
                accepted: true,
            },
        );
        app.store.conversation_order.push(empty_id.clone());
        app.active_conversation = Some(empty_id);
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_message_request() {
        // Eve's conversation is unaccepted (message request)
        let mut app = demo_app();
        app.active_conversation = Some("+15550007777".to_string());
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_disappearing_messages() {
        // Dave's conversation has disappearing messages with timer icons
        let mut app = demo_app();
        app.active_conversation = Some("+15550004444".to_string());
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_sidebar_filter() {
        let mut app = demo_app();
        app.open_overlay(OverlayKind::SidebarFilter);
        app.sidebar_filter = "ali".to_string();
        app.refresh_sidebar_filter();
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_theme_picker_overlay() {
        let mut app = demo_app();
        app.open_overlay(OverlayKind::ThemePicker);
        app.theme_picker.index = 1;
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_pin_duration_overlay() {
        let mut app = demo_app();
        app.open_overlay(OverlayKind::PinDuration);
        app.pin_duration.index = 1;
        app.pin_duration.pending = Some(PinPending {
            conv_id: "+15551234567".to_string(),
            is_group: false,
            target_author: "+15551234567".to_string(),
            target_timestamp: 1000,
        });
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_action_menu_overlay() {
        let mut app = demo_app();
        app.open_overlay(OverlayKind::ActionMenu);
        app.action_menu.index = 0;
        app.scroll.focused_index = Some(0);
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_contacts_overlay() {
        let mut app = demo_app();
        app.open_overlay(OverlayKind::Contacts);
        app.contacts_overlay.index = 0;
        app.contacts_overlay.filtered = vec![
            ("+15551234567".to_string(), "Alice".to_string()),
            ("+15559876543".to_string(), "Bob".to_string()),
        ];
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_forward_overlay() {
        let mut app = demo_app();
        app.open_overlay(OverlayKind::Forward);
        app.forward.index = 0;
        app.forward.filtered = vec![
            ("+15551234567".to_string(), "Alice".to_string()),
            ("+15559876543".to_string(), "Bob".to_string()),
        ];
        app.forward.body = "Hello world".to_string();
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_emoji_picker_overlay() {
        let mut app = demo_app();
        app.emoji_picker.open(EmojiPickerSource::Input, None);
        app.open_overlay(OverlayKind::EmojiPicker);
        let output = render_to_string(&mut app, 100, 30);
        insta::assert_snapshot!(output);
    }
}
