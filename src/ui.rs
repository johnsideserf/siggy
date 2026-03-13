use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
    Frame,
};

use crate::app::{App, AutocompleteMode, GroupMenuState, InputMode, VisibleImage, PIN_DURATIONS, QUICK_REACTIONS, SETTINGS};
use crate::keybindings::{self, BindingMode, KeyAction};
use crate::signal::types::{MessageStatus, PollData, PollVote, Reaction, StyleType, TrustLevel};
use crate::image_render::{self, ImageProtocol};
use crate::input::{COMMANDS, format_compact_duration};
use crate::theme::Theme;

// Layout constants
const SIDEBAR_AUTO_HIDE_WIDTH: u16 = 60;
const MIN_CHAT_WIDTH: u16 = 30;
const MSG_WINDOW_MULTIPLIER: usize = 10;

// Popup dimensions
const SETTINGS_POPUP_WIDTH: u16 = 50;
const SETTINGS_POPUP_HEIGHT: u16 = 18;
const CONTACTS_POPUP_WIDTH: u16 = 50;
const CONTACTS_MAX_VISIBLE: usize = 20;
const FILE_BROWSER_POPUP_WIDTH: u16 = 60;
const FILE_BROWSER_MAX_VISIBLE: usize = 20;
const SEARCH_POPUP_WIDTH: u16 = 60;
const SEARCH_MAX_VISIBLE: usize = 15;
const GROUP_MENU_POPUP_WIDTH: u16 = 40;
const GROUP_MEMBER_MAX_VISIBLE: usize = 15;
const ABOUT_POPUP_WIDTH: u16 = 50;
const PROFILE_POPUP_WIDTH: u16 = 50;

/// Map a MessageStatus to its display symbol and color.
pub(crate) fn status_symbol(status: MessageStatus, nerd_fonts: bool, color: bool, theme: &Theme) -> (&'static str, Color) {
    let (unicode_sym, nerd_sym, colored) = match status {
        MessageStatus::Failed   => ("\u{2717}", "\u{f055c}", theme.receipt_failed),
        MessageStatus::Sending  => ("\u{25cc}", "\u{f0996}", theme.receipt_sending),
        MessageStatus::Sent     => ("\u{25cb}", "\u{f0954}", theme.receipt_sent),
        MessageStatus::Delivered=> ("\u{2713}", "\u{f012c}", theme.receipt_delivered),
        MessageStatus::Read     => ("\u{25cf}", "\u{f012d}", theme.receipt_read),
        MessageStatus::Viewed   => ("\u{25c9}", "\u{f0208}", theme.receipt_viewed),
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
    let hash: u32 = name.bytes().fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
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
fn centered_popup(
    frame: &mut Frame, area: Rect, pref_width: u16, pref_height: u16, title: &str,
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
        .title_style(Style::default().fg(theme.accent).add_modifier(Modifier::BOLD))
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
            let bg = buf.cell(Position::new(start_x, y))
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
                let overlaps = regions.iter().any(|(ms, me, _)| abs_start < *me && abs_end > *ms);
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
                        StyleType::Strikethrough => style = style.add_modifier(Modifier::CROSSED_OUT),
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
    app.link_url_map.clear();
    app.visible_images.clear();
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
            (1, 0, [Constraint::Min(MIN_CHAT_WIDTH), Constraint::Length(app.sidebar_width)])
        } else {
            (0, 1, [Constraint::Length(app.sidebar_width), Constraint::Min(MIN_CHAT_WIDTH)])
        };
        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(constraints)
            .split(body_area);

        draw_sidebar(frame, app, horizontal[sidebar_idx]);
        draw_chat_area(frame, app, horizontal[chat_idx])
    } else {
        app.mouse_sidebar_inner = None;
        draw_chat_area(frame, app, body_area)
    };

    draw_status_bar(frame, app, status_area, sidebar_auto_hidden);

    // Autocomplete popup (overlays everything)
    if app.autocomplete_visible {
        let has_items = match app.autocomplete_mode {
            AutocompleteMode::Command => !app.autocomplete_candidates.is_empty(),
            AutocompleteMode::Mention => !app.mention_candidates.is_empty(),
            AutocompleteMode::Join => !app.join_candidates.is_empty(),
        };
        if has_items {
            draw_autocomplete(frame, app, input_area);
        }
    }

    // Settings overlay (overlays everything)
    if app.show_settings {
        draw_settings(frame, app, size);
    }

    // Help overlay (overlays everything)
    if app.show_help {
        draw_help(frame, app, size);
    }

    // Contacts overlay (overlays everything)
    if app.show_contacts {
        draw_contacts(frame, app, size);
    }

    // Verify identity overlay
    if app.show_verify {
        draw_verify(frame, app, size);
    }

    // Search overlay
    if app.search.visible {
        draw_search(frame, app, size);
    }

    // File browser overlay
    if app.file_picker.visible {
        draw_file_browser(frame, app, size);
    }

    // Group management menu overlay
    if app.group_menu_state.is_some() {
        draw_group_menu(frame, app, size);
    }

    // Message request overlay
    if app.show_message_request {
        draw_message_request(frame, app, size);
    }

    // Action menu overlay
    if app.show_action_menu {
        draw_action_menu(frame, app, size);
    }

    // Reaction picker overlay
    if app.show_reaction_picker {
        draw_reaction_picker(frame, app, size);
    }

    // Delete confirmation overlay
    if app.show_delete_confirm {
        draw_delete_confirm(frame, app, size);
    }

    // Theme picker overlay
    if app.show_theme_picker {
        draw_theme_picker(frame, app, size);
    }

    // Keybindings overlay
    if app.show_keybindings {
        draw_keybindings(frame, app, size);
    }

    // Settings profile manager overlay
    if app.show_settings_profile_manager {
        draw_settings_profile_manager(frame, app, size);
    }

    // Pin duration picker overlay
    if app.show_pin_duration {
        draw_pin_duration_picker(frame, app, size);
    }

    // Poll vote overlay
    if app.show_poll_vote {
        draw_poll_vote_overlay(frame, app, size);
    }

    // About overlay
    if app.show_about {
        draw_about(frame, app, size);
    }

    // Profile editor overlay
    if app.show_profile {
        draw_profile(frame, app, size);
    }

    // Forward message picker overlay
    if app.show_forward {
        draw_forward(frame, app, size);
    }

    // Collect link regions from the rendered buffer for OSC 8 injection
    let area = frame.area();
    app.link_regions = collect_link_regions(frame.buffer_mut(), area, app.theme.link);

    // Resolve hidden URLs for attachment links (display text has no URI scheme)
    for link in &mut app.link_regions {
        if !link.url.contains("://") {
            if let Some(url) = app.link_url_map.get(&link.text) {
                link.url = url.clone();
            }
        }
    }
}

fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = &app.theme;
    let max_name_width = (area.width as usize).saturating_sub(5); // "• # " + margin

    let items: Vec<ListItem> = app
        .conversation_order
        .iter()
        .map(|id| {
            let conv = &app.conversations[id];
            let is_active = app
                .active_conversation
                .as_ref()
                .map(|a| a == id)
                .unwrap_or(false);

            let has_unread = conv.unread > 0;
            let name = truncate(&conv.name, max_name_width);

            let mut spans = Vec::new();

            // Active marker or padding
            if is_active {
                spans.push(Span::styled(
                    "▸ ",
                    Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::raw("  "));
            }

            // Unread / message request marker
            if !conv.accepted {
                spans.push(Span::styled("? ", Style::default().fg(theme.mention)));
            } else if has_unread && !is_active {
                spans.push(Span::styled("• ", Style::default().fg(theme.warning)));
            } else {
                spans.push(Span::raw("  "));
            }

            // Group prefix (dimmed #)
            if conv.is_group {
                spans.push(Span::styled(
                    "#",
                    Style::default().fg(theme.fg_muted),
                ));
            }

            // Conversation name
            let is_muted = app.muted_conversations.contains(id);
            let name_style = if is_active {
                Style::default()
                    .fg(theme.fg)
                    .add_modifier(Modifier::BOLD)
            } else if has_unread {
                Style::default().fg(theme.warning)
            } else if is_muted {
                Style::default().fg(theme.fg_muted)
            } else {
                Style::default().fg(theme.fg_secondary)
            };
            spans.push(Span::styled(name, name_style));

            if is_muted {
                spans.push(Span::styled(" ~", Style::default().fg(theme.fg_muted)));
            }
            if app.blocked_conversations.contains(id) {
                spans.push(Span::styled(" x", Style::default().fg(theme.error)));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    let border_side = if app.sidebar_on_right { Borders::LEFT } else { Borders::RIGHT };
    let block = Block::default()
        .borders(border_side)
        .border_type(BorderType::Rounded)
        .title(" Chats ")
        .title_style(Style::default().fg(theme.accent).add_modifier(Modifier::BOLD));
    app.mouse_sidebar_inner = Some(block.inner(area));

    let sidebar = List::new(items).block(block);
    frame.render_widget(sidebar, area);
}

fn draw_chat_area(frame: &mut Frame, app: &mut App, area: Rect) -> Rect {
    let max_input_height = (area.height / 2).max(3);
    let input_height = (app.input_line_count() as u16 + 2).clamp(3, max_input_height);
    let chat_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),   // messages (typing indicator rendered inside)
            Constraint::Length(input_height), // input
        ])
        .split(area);

    let messages_area = chat_layout[0];
    let input_area = chat_layout[1];

    app.mouse_input_area = input_area;
    draw_messages(frame, app, messages_area);
    draw_input(frame, app, input_area);
    input_area
}

fn draw_messages(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = &app.theme;
    let (title_spans, title_right) = match &app.active_conversation {
        Some(id) => {
            let conv = &app.conversations[id];
            let prefix = if conv.is_group { " #" } else { " " };
            let mut spans = vec![
                Span::styled(
                    format!("{prefix}{} ", conv.name),
                    Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
                ),
            ];

            // Timer indicator when disappearing messages are enabled
            if conv.expiration_timer > 0 {
                let timer_label = format_compact_duration(conv.expiration_timer);
                let icon = if app.nerd_fonts { "\u{F0150}" } else { "\u{23F1}" };
                spans.push(Span::styled(
                    format!("{icon} {timer_label} "),
                    Style::default().fg(theme.fg_muted),
                ));
            }

            // Trust level indicator (1:1 only)
            if !conv.is_group {
                if let Some(trust) = app.identity_trust.get(id) {
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
            }

            // Scroll indicator in title
            let right = if app.scroll_offset > 0 {
                format!(" \u{2191} {} more ", app.scroll_offset)
            } else {
                String::new()
            };
            (spans, right)
        }
        None => (vec![Span::styled(
            " siggy ".to_string(),
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
        )], String::new()),
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
        Some(id) => app.conversations.get(id).map(|c| &c.messages),
        None => None,
    };

    // Build pinned message banner text
    let pinned_banner_text: Option<String> = messages_ref.and_then(|msgs| {
        let pinned: Vec<_> = msgs.iter().filter(|m| m.is_pinned && !m.is_deleted).collect();
        match pinned.len() {
            0 => None,
            1 => {
                let m = pinned[0];
                let body: String = m.body.chars().take(80).collect();
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

    if let Some(ref pin_text) = pinned_banner_text {
        if let Some(banner) = banner_area {
            let pin_line = Line::from(Span::styled(
                truncate(pin_text, banner.width as usize),
                Style::default().fg(theme.warning).add_modifier(Modifier::BOLD),
            ));
            frame.render_widget(Paragraph::new(pin_line), banner);
        }
    }

    app.mouse_messages_area = inner;

    let messages = match &app.active_conversation {
        Some(id) => {
            if let Some(conv) = app.conversations.get(id) {
                &conv.messages
            } else {
                app.focused_message_time = None;
                app.focused_msg_index = None;
                return;
            }
        }
        None => {
            draw_welcome(frame, app, inner);
            app.focused_message_time = None;
            app.focused_msg_index = None;
            return;
        }
    };

    let available_height = inner.height as usize;
    let total = messages.len();

    // Build lines from a fixed window of recent messages.
    // scroll_offset is NOT included here — it controls the Paragraph scroll position instead.
    // Including it would expand the window by 1 message per scroll increment, growing
    // content_height and base_scroll in lockstep, keeping scroll_y constant (viewport stuck).
    let start = total.saturating_sub(available_height * MSG_WINDOW_MULTIPLIER);
    let visible = &messages[start..total];

    // Get last_read_index for unread marker
    let conv_id = app.active_conversation.as_ref().unwrap();
    let last_read = app.last_read_index.get(conv_id).copied().unwrap_or(0);

    let inner_width = inner.width as usize;

    let mut lines: Vec<Line> = Vec::new();
    let mut prev_date: Option<String> = None;

    // Map each line to its source message index (None for separators/markers)
    let mut line_msg_idx: Vec<Option<usize>> = Vec::new();

    // Track images for native protocol overlay: (first_line_index, line_count, path)
    let use_native = app.native_images && app.image_protocol != ImageProtocol::Halfblock;
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
                    lines.push(build_separator(&label, inner_width, Style::default().fg(theme.fg_muted)));
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
                Style::default().fg(theme.error).add_modifier(Modifier::BOLD),
            ));
            line_msg_idx.push(None);
        }

        if msg.is_system {
            lines.push(Line::from(Span::styled(
                format!("  {}", msg.body),
                Style::default().fg(theme.system_msg),
            )));
            line_msg_idx.push(Some(msg_index));
        } else {
            // Render quoted reply line above message
            if let Some(ref quote) = msg.quote {
                let quote_body = truncate(&quote.body, 50);
                lines.push(Line::from(vec![
                    Span::styled("  \u{256D} ", Style::default().fg(theme.quote)),
                    Span::styled(
                        format!("<{}>", quote.author),
                        Style::default()
                            .fg(sender_color(&quote.author, theme))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" {quote_body}"),
                        Style::default().fg(theme.quote),
                    ),
                ]));
                line_msg_idx.push(Some(msg_index));
            }

            let time = msg.format_time();
            let mut spans = Vec::new();

            // Status symbol for outgoing messages (before timestamp)
            if app.show_receipts {
                if let Some(status) = msg.status {
                    let (sym, color) = status_symbol(status, app.nerd_fonts, app.color_receipts, theme);
                    spans.push(Span::styled(
                        format!("{sym} "),
                        Style::default().fg(color),
                    ));
                }
            }

            if msg.expires_in_seconds > 0 {
                let icon = if app.nerd_fonts { "\u{F0150}" } else { "\u{23F1}" };
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
                    Style::default().fg(theme.fg_muted).add_modifier(Modifier::ITALIC),
                ));
            }

            // "(pinned)" label
            if msg.is_pinned {
                spans.push(Span::styled(
                    " (pinned)",
                    Style::default().fg(theme.warning).add_modifier(Modifier::ITALIC),
                ));
            }

            if msg.is_deleted {
                // Deleted message body
                spans.push(Span::styled(
                    " [deleted]",
                    Style::default().fg(theme.fg_muted).add_modifier(Modifier::ITALIC),
                ));
            } else {
                // Style URIs and @mentions
                let (body_spans, hidden_url) = styled_uri_spans(&msg.body, &msg.mention_ranges, &msg.style_ranges, theme);
                if let Some(url) = hidden_url {
                    // Collect display text for link_url_map lookup
                    let display_text: String = body_spans.iter().map(|s| s.content.as_ref()).collect();
                    app.link_url_map.insert(display_text, url);
                }
                spans.push(Span::raw(" ".to_string()));
                spans.extend(body_spans);
            }

            lines.push(Line::from(spans));
            line_msg_idx.push(Some(msg_index));

            // Render inline image preview if available (skip for deleted, skip if images disabled)
            if !msg.is_deleted && app.inline_images {
                if let Some(ref image_lines) = msg.image_lines {
                    let first_idx = lines.len();
                    let count = image_lines.len();
                    for line in image_lines {
                        lines.push(line.clone());
                        line_msg_idx.push(Some(msg_index));
                    }
                    // Record for native protocol overlay
                    if use_native {
                        if let Some(ref path) = msg.image_path {
                            image_records.push((first_idx, count, path.clone()));
                        }
                    }
                }
            }

            // Render link preview block
            if !msg.is_deleted && app.show_link_previews {
                if let Some(ref preview) = msg.preview {
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
                            Span::styled(
                                truncate(desc, 60),
                                Style::default().fg(theme.fg_muted),
                            ),
                        ]));
                        line_msg_idx.push(Some(msg_index));
                    }
                    lines.push(Line::from(vec![
                        Span::styled("  \u{2570} ", Style::default().fg(theme.link)),
                        Span::styled(
                            truncate(&preview.url, 60),
                            Style::default().fg(theme.link).add_modifier(Modifier::UNDERLINED),
                        ),
                    ]));
                    line_msg_idx.push(Some(msg_index));

                    // Render link preview thumbnail (only when images enabled)
                    if app.inline_images {
                        if let Some(ref img_lines) = msg.preview_image_lines {
                            let first_idx = lines.len();
                            let count = img_lines.len();
                            for line in img_lines {
                                lines.push(line.clone());
                                line_msg_idx.push(Some(msg_index));
                            }
                            if use_native {
                                if let Some(ref path) = msg.preview_image_path {
                                    image_records.push((first_idx, count, path.clone()));
                                }
                            }
                        }
                    }
                }
            }

            // Render inline poll display
            if !msg.is_deleted {
                if let Some(ref poll_data) = msg.poll_data {
                    let poll_lines = build_poll_display(poll_data, &msg.poll_votes, &app.account, theme);
                    for line in poll_lines {
                        lines.push(line);
                        line_msg_idx.push(Some(msg_index));
                    }
                }
            }

            // Render reaction summary line (skip for deleted)
            if !msg.is_deleted && !msg.reactions.is_empty() {
                lines.push(build_reaction_summary(&msg.reactions, app.reaction_verbose, theme));
                line_msg_idx.push(Some(msg_index));
            }
        }
    }

    // Append typing indicator as the last line inside the message area
    if let Some(ref conv_id) = app.active_conversation {
        let typers: Vec<String> = app
            .typing_indicators
            .iter()
            .filter(|(key, _)| *key == conv_id)
            .map(|(_, (sender, _))| {
                if let Some(name) = app.contact_names.get(sender) {
                    name.clone()
                } else if let Some(conv) = app.conversations.get(sender) {
                    conv.name.clone()
                } else {
                    sender.clone()
                }
            })
            .collect();

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

    // Compute actual content height accounting for line wrapping
    let content_height: usize = lines.iter().map(|line| {
        let w = line.width();
        if w == 0 { 1 } else { w.div_ceil(inner_width.max(1)) }
    }).sum();

    // Bottom-align by default; scroll_offset shifts the view upward
    let base_scroll = content_height.saturating_sub(available_height);
    app.scroll_offset = app.scroll_offset.min(base_scroll);
    let mut scroll_y = base_scroll - app.scroll_offset;

    // Signal when user has scrolled to the top of loaded content
    app.at_scroll_top = app.scroll_offset >= base_scroll
        && base_scroll > 0
        && app.active_conversation.as_ref()
            .is_some_and(|id| app.has_more_messages.contains(id));

    // Determine the focused message for highlight and full-timestamp display in Normal mode.
    // Check focused_msg_index too so J/K navigation works even when content fits the viewport
    // (base_scroll == 0 clamps scroll_offset to 0, but J/K focus should persist).
    //
    // `render_focus` is used for highlighting; it may differ from app.focused_msg_index when
    // j/k line-scrolling (where we derive focus for display but don't persist it, to avoid
    // the "ensure visible" logic snapping the viewport back on the next frame).
    let render_focus;
    if app.mode == InputMode::Normal && (app.scroll_offset > 0 || app.focused_msg_index.is_some()) {
        if let Some(fi) = app.focused_msg_index {
            // J/K already set focused_msg_index — ensure it's visible by adjusting scroll.
            let iw = inner_width.max(1);
            let mut msg_start: Option<usize> = None;
            let mut msg_end = 0usize;
            let mut cumul = 0usize;
            for (idx, line) in lines.iter().enumerate() {
                let w = line.width();
                let h = if w == 0 { 1 } else { w.div_ceil(iw) };
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
                    app.scroll_offset = base_scroll.saturating_sub(start);
                    scroll_y = base_scroll - app.scroll_offset;
                } else if msg_end > scroll_y + available_height {
                    // Message is below viewport — scroll down
                    let new_scroll_y = msg_end.saturating_sub(available_height);
                    app.scroll_offset = base_scroll.saturating_sub(new_scroll_y);
                    scroll_y = base_scroll - app.scroll_offset;
                }
            }
            app.focused_message_time = messages.get(fi).map(|m| m.timestamp);
            render_focus = Some(fi);
        } else {
            // j/k line-scroll without J/K — derive focus from viewport for display only.
            // Do NOT store into focused_msg_index; that would cause the "ensure visible"
            // logic on the next frame to snap the viewport back to the bottom.
            let idx = find_focused_msg_index(&lines, &line_msg_idx, inner_width, scroll_y, available_height);
            app.focused_message_time = idx.and_then(|i| messages.get(i)).map(|m| m.timestamp);
            render_focus = idx;
        }
    } else {
        app.focused_msg_index = None;
        app.focused_message_time = None;
        render_focus = None;
    };

    // Compute screen positions for native protocol image overlay (before lines is consumed)
    if !image_records.is_empty() {
        // Build cumulative wrapped-line positions
        let mut wrapped_positions: Vec<usize> = Vec::with_capacity(lines.len() + 1);
        let mut cumulative = 0usize;
        for line in &lines {
            wrapped_positions.push(cumulative);
            let w = line.width();
            cumulative += if w == 0 { 1 } else { w.div_ceil(inner_width.max(1)) };
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

                app.visible_images.push(VisibleImage {
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
                let patched: Vec<Span> = line.spans.drain(..).map(|mut s| {
                    s.style = s.style.bg(theme.msg_selected_bg);
                    s
                }).collect();
                *line = Line::from(patched);
            }
        }
    }

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll_y as u16, 0));
    frame.render_widget(paragraph, inner);

    if use_native && app.image_protocol == ImageProtocol::Kitty {
        patch_kitty_placeholders(frame, app);
    }

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
    for img in &app.visible_images {
        let id = if let Some(&existing) = app.kitty_image_ids.get(&img.path) {
            existing
        } else {
            let new_id = app.next_kitty_image_id;
            app.next_kitty_image_id += 1;
            app.kitty_image_ids.insert(img.path.clone(), new_id);
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

        if !app.kitty_transmitted.contains(&id) {
            app.kitty_pending_transmits.push((
                id,
                img.path.clone(),
                img.width,
                img.full_height,
            ));
        }
    }
}

/// Build a reaction summary line like "    👍 2  ❤️ 1  😂 1"
pub(crate) fn build_reaction_summary(reactions: &[Reaction], verbose: bool, theme: &Theme) -> Line<'static> {
    if verbose {
        // Verbose: group by emoji, show sender names
        let mut grouped: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
        for r in reactions {
            grouped.entry(r.emoji.clone()).or_default().push(r.sender.clone());
        }
        let mut spans = vec![Span::raw("    ".to_string())];
        for (emoji, senders) in &grouped {
            spans.push(Span::raw(format!("{emoji} ")));
            spans.push(Span::styled(
                senders.join(", "),
                Style::default().fg(theme.fg_muted),
            ));
            spans.push(Span::raw("  ".to_string()));
        }
        Line::from(spans)
    } else {
        // Summary: emoji + count
        let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for r in reactions {
            *counts.entry(r.emoji.clone()).or_default() += 1;
        }
        let mut spans = vec![Span::raw("    ".to_string())];
        for (emoji, count) in &counts {
            spans.push(Span::raw(emoji.clone()));
            spans.push(Span::styled(
                format!(" {count}  "),
                Style::default().fg(theme.fg_muted),
            ));
        }
        Line::from(spans)
    }
}

fn draw_group_menu(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let state = match &app.group_menu_state {
        Some(s) => s,
        None => return,
    };
    match state {
        GroupMenuState::Menu => {
            let items = app.group_menu_items();
            if items.is_empty() {
                return;
            }
            let popup_height = items.len() as u16 + 4;
            let title = app.active_conversation.as_ref()
                .and_then(|id| app.conversations.get(id))
                .filter(|c| c.is_group)
                .map(|c| format!(" #{} ", c.name))
                .unwrap_or_else(|| " Group ".to_string());
            let (popup_area, block) = centered_popup(
                frame, area, GROUP_MENU_POPUP_WIDTH, popup_height, &title, theme,
            );
            let inner = block.inner(popup_area);
            frame.render_widget(block, popup_area);
            let content_width = inner.width as usize;
            let mut lines: Vec<Line> = Vec::new();
            for (i, action) in items.iter().enumerate() {
                let is_selected = i == app.group_menu_index;
                let icon = if app.nerd_fonts {
                    format!("{} ", action.nerd_icon)
                } else {
                    String::new()
                };
                let label_part = format!("  {icon}{}", action.label);
                let hint_width = action.key_hint.len();
                let pad = content_width.saturating_sub(label_part.chars().count() + hint_width + 2);
                let padding = " ".repeat(pad);
                let row_style = if is_selected {
                    Style::default().bg(theme.bg_selected)
                } else {
                    Style::default()
                };
                let hint_style = if is_selected {
                    Style::default().bg(theme.bg_selected).fg(theme.fg_muted).add_modifier(Modifier::DIM)
                } else {
                    Style::default().fg(theme.fg_muted)
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{label_part}{padding}"), row_style),
                    Span::styled(format!("{} ", action.key_hint), hint_style),
                ]));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Esc to close",
                Style::default().fg(theme.fg_muted),
            )));
            let popup = Paragraph::new(lines);
            frame.render_widget(popup, inner);
        }
        GroupMenuState::Members => {
            let max_visible = GROUP_MEMBER_MAX_VISIBLE.min(app.group_menu_filtered.len().max(1));
            let pref_height = max_visible as u16 + 5;
            let title = " Members ".to_string();
            let (popup_area, block) = centered_popup(
                frame, area, GROUP_MENU_POPUP_WIDTH, pref_height, &title, theme,
            );
            let inner_height = popup_area.height.saturating_sub(2) as usize;
            let footer_lines = 2;
            let visible_rows = inner_height.saturating_sub(footer_lines);
            let scroll_offset = if app.group_menu_index >= visible_rows {
                app.group_menu_index - visible_rows + 1
            } else {
                0
            };
            let mut lines: Vec<Line> = Vec::new();
            if app.group_menu_filtered.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  No members",
                    Style::default().fg(theme.fg_muted),
                )));
            } else {
                let end = (scroll_offset + visible_rows).min(app.group_menu_filtered.len());
                for (i, (phone, name)) in app.group_menu_filtered[scroll_offset..end].iter().enumerate() {
                    let actual_index = scroll_offset + i;
                    let is_selected = actual_index == app.group_menu_index;
                    let is_self = *phone == app.account;
                    let display = if is_self {
                        format!("  {} (you)", name)
                    } else {
                        format!("  {}", name)
                    };
                    let name_style = if is_selected {
                        Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.fg)
                    };
                    let phone_style = if is_selected {
                        Style::default().bg(theme.bg_selected).fg(theme.fg_muted)
                    } else {
                        Style::default().fg(theme.fg_muted)
                    };
                    lines.push(Line::from(vec![
                        Span::styled(display, name_style),
                        Span::styled(format!("  {}", phone), phone_style),
                    ]));
                }
            }
            while lines.len() < visible_rows {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Esc to go back",
                Style::default().fg(theme.fg_muted),
            )));
            let popup = Paragraph::new(lines).block(block);
            frame.render_widget(popup, popup_area);
        }
        GroupMenuState::AddMember | GroupMenuState::RemoveMember => {
            let is_add = *state == GroupMenuState::AddMember;
            let max_visible = GROUP_MEMBER_MAX_VISIBLE.min(app.group_menu_filtered.len().max(1));
            let pref_height = max_visible as u16 + 5;
            let title = if is_add {
                if app.group_menu_filter.is_empty() {
                    " Add Member ".to_string()
                } else {
                    format!(" Add Member [{}] ", app.group_menu_filter)
                }
            } else if app.group_menu_filter.is_empty() {
                " Remove Member ".to_string()
            } else {
                format!(" Remove Member [{}] ", app.group_menu_filter)
            };
            let (popup_area, block) = centered_popup(
                frame, area, CONTACTS_POPUP_WIDTH, pref_height, &title, theme,
            );
            let inner_height = popup_area.height.saturating_sub(2) as usize;
            let footer_lines = 2;
            let visible_rows = inner_height.saturating_sub(footer_lines);
            let scroll_offset = if app.group_menu_index >= visible_rows {
                app.group_menu_index - visible_rows + 1
            } else {
                0
            };
            let mut lines: Vec<Line> = Vec::new();
            if app.group_menu_filtered.is_empty() {
                let msg = if is_add { "  No contacts to add" } else { "  No members to remove" };
                lines.push(Line::from(Span::styled(
                    msg,
                    Style::default().fg(theme.fg_muted),
                )));
            } else {
                let end = (scroll_offset + visible_rows).min(app.group_menu_filtered.len());
                let inner_w = popup_area.width.saturating_sub(2) as usize;
                for (i, (phone, name)) in app.group_menu_filtered[scroll_offset..end].iter().enumerate() {
                    let actual_index = scroll_offset + i;
                    let is_selected = actual_index == app.group_menu_index;
                    let number_display = format!("  {}", phone);
                    let name_max = inner_w.saturating_sub(number_display.len() + 2);
                    let display_name = truncate(name, name_max);
                    let name_style = if is_selected {
                        Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.fg)
                    };
                    let number_style = if is_selected {
                        Style::default().bg(theme.bg_selected).fg(theme.accent)
                    } else {
                        Style::default().fg(theme.fg_muted)
                    };
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {}", display_name), name_style),
                        Span::styled(number_display, number_style),
                    ]));
                }
            }
            while lines.len() < visible_rows {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Enter to select \u{00b7} Esc to cancel",
                Style::default().fg(theme.fg_muted),
            )));
            let popup = Paragraph::new(lines).block(block);
            frame.render_widget(popup, popup_area);
        }
        GroupMenuState::Rename | GroupMenuState::Create => {
            let is_rename = *state == GroupMenuState::Rename;
            let title = if is_rename { " Rename Group " } else { " Create Group " };
            let (popup_area, block) = centered_popup(
                frame, area, GROUP_MENU_POPUP_WIDTH, 6, title, theme,
            );
            let inner = block.inner(popup_area);
            frame.render_widget(block, popup_area);
            let mut lines: Vec<Line> = Vec::new();
            let input_display = format!("  {}\u{2588}", app.group_menu_input);
            lines.push(Line::from(Span::styled(
                input_display,
                Style::default().fg(theme.fg),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Enter to confirm \u{00b7} Esc to cancel",
                Style::default().fg(theme.fg_muted),
            )));
            let popup = Paragraph::new(lines);
            frame.render_widget(popup, inner);
        }
        GroupMenuState::LeaveConfirm => {
            let group_name = app.active_conversation.as_ref()
                .and_then(|id| app.conversations.get(id))
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "this group".to_string());
            let prompt = format!("Leave #{}?", group_name);
            let (popup_area, block) = centered_popup(
                frame, area, GROUP_MENU_POPUP_WIDTH, 5, " Leave Group ", theme,
            );
            let inner = block.inner(popup_area);
            frame.render_widget(block, popup_area);
            let mut lines: Vec<Line> = Vec::new();
            lines.push(Line::from(Span::styled(
                format!("  {}", prompt),
                Style::default().fg(theme.warning),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  (y)es / (n)o",
                Style::default().fg(theme.fg_muted),
            )));
            let popup = Paragraph::new(lines);
            frame.render_widget(popup, inner);
        }
    }
}

fn draw_message_request(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let conv_id = match app.active_conversation.as_ref() {
        Some(id) => id,
        None => return,
    };
    let conv = match app.conversations.get(conv_id) {
        Some(c) => c,
        None => return,
    };

    let msg_count = conv.messages.len();
    let name = &conv.name;
    let phone = &conv.id;

    let (popup_area, block) = centered_popup(frame, area, 36, 9, " Message Request ", theme);
    frame.render_widget(block, popup_area);

    let inner = popup_area.inner(ratatui::layout::Margin { vertical: 1, horizontal: 2 });
    let lines = vec![
        Line::from(Span::styled(name.as_str(), Style::default().fg(theme.fg).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled(phone.as_str(), Style::default().fg(theme.fg_muted))),
        Line::from(Span::styled(
            format!("{} message{}", msg_count, if msg_count == 1 { "" } else { "s" }),
            Style::default().fg(theme.fg_secondary),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("(a)", Style::default().fg(theme.success).add_modifier(Modifier::BOLD)),
            Span::styled("ccept / ", Style::default().fg(theme.fg_secondary)),
            Span::styled("(d)", Style::default().fg(theme.error).add_modifier(Modifier::BOLD)),
            Span::styled("elete", Style::default().fg(theme.fg_secondary)),
        ]),
        Line::from(Span::styled("Esc to go back", Style::default().fg(theme.fg_muted))),
    ];

    let text = Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center);
    frame.render_widget(text, inner);
}

fn draw_action_menu(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let items = app.action_menu_items();
    if items.is_empty() {
        return;
    }

    let popup_width: u16 = 30;
    let popup_height = items.len() as u16 + 4;

    let (popup_area, block) = centered_popup(
        frame, area, popup_width, popup_height, " Actions ", theme,
    );

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let content_width = inner.width as usize;

    let mut lines: Vec<Line> = Vec::new();
    for (i, action) in items.iter().enumerate() {
        let is_selected = i == app.action_menu_index;
        let icon = if app.nerd_fonts {
            format!("{} ", action.nerd_icon)
        } else {
            String::new()
        };

        let label_part = format!("  {icon}{}", action.label);
        let hint_width = action.key_hint.len();
        let pad = content_width.saturating_sub(label_part.chars().count() + hint_width + 2);
        let padding = " ".repeat(pad);

        let row_style = if is_selected {
            Style::default().bg(theme.bg_selected)
        } else {
            Style::default()
        };
        let hint_style = if is_selected {
            Style::default().bg(theme.bg_selected).fg(theme.fg_muted).add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(theme.fg_muted)
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{label_part}{padding}"), row_style),
            Span::styled(format!("{} ", action.key_hint), hint_style),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Esc to close",
        Style::default().fg(theme.fg_muted),
    )));

    let popup = Paragraph::new(lines);
    frame.render_widget(popup, inner);
}

fn draw_reaction_picker(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let emoji_count = QUICK_REACTIONS.len();
    let popup_width = (emoji_count * 4 + 4) as u16;
    let popup_height = 3u16;

    let (popup_area, block) = centered_popup(
        frame, area, popup_width, popup_height, " React ", theme,
    );

    let mut spans = vec![Span::raw(" ".to_string())];
    for (i, emoji) in QUICK_REACTIONS.iter().enumerate() {
        let style = if i == app.reaction_picker_index {
            Style::default().bg(theme.bg_selected).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let prefix = if i == app.reaction_picker_index { "[" } else { " " };
        let suffix = if i == app.reaction_picker_index { "]" } else { " " };
        spans.push(Span::styled(format!("{prefix}{emoji}{suffix}"), style));
    }

    let line = Line::from(spans);
    let popup = Paragraph::new(vec![line]).block(block);
    frame.render_widget(popup, popup_area);
}

fn draw_delete_confirm(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let msg = app.selected_message();
    let is_outgoing = msg.is_some_and(|m| m.sender == "you");

    let (popup_area, block) = centered_popup(
        frame, area, 44, 5, " Delete Message ", theme,
    );

    let prompt = if is_outgoing {
        "Delete for everyone? (y)es / (l)ocal / (n)o"
    } else {
        "Delete locally? (y)es / (n)o"
    };

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {prompt}"),
            Style::default().fg(theme.fg),
        )),
    ];
    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

/// Render the welcome/empty-state screen when no conversation is active.
fn draw_welcome(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let mut lines = vec![Line::from("")];

    if let Some(ref err) = app.connection_error {
        lines.push(Line::from(Span::styled(
            "  Connection Error",
            Style::default().fg(theme.error).add_modifier(Modifier::BOLD),
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
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  {spinner_char} {}", app.startup_status),
            Style::default().fg(theme.fg_muted),
        )));
    } else if app.conversation_order.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Welcome to siggy",
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
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
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
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
fn find_focused_msg_index(
    lines: &[Line], line_msg_idx: &[Option<usize>],
    inner_width: usize, scroll_y: usize, available_height: usize,
) -> Option<usize> {
    let target_wrapped = scroll_y + available_height.saturating_sub(1);
    let mut cumul = 0usize;
    let mut focused_line_idx = None;
    for (idx, line) in lines.iter().enumerate() {
        let w = line.width();
        let h = if w == 0 { 1 } else { w.div_ceil(inner_width.max(1)) };
        if cumul + h > target_wrapped {
            focused_line_idx = Some(idx);
            break;
        }
        cumul += h;
    }
    let mut li = focused_line_idx?;
    loop {
        if let Some(Some(mi)) = line_msg_idx.get(li) {
            return Some(*mi);
        }
        if li == 0 {
            return None;
        }
        li -= 1;
    }
}

fn draw_input(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = &app.theme;
    let border_color = match app.mode {
        InputMode::Insert => theme.input_insert,
        InputMode::Normal => theme.input_normal,
    };

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color));

    // Show reply/edit indicator as block title
    if let Some((_, ref snippet, _)) = app.reply_target {
        let label = format!(" replying: {}… ", truncate(snippet, 30));
        block = block.title(Line::from(Span::styled(
            label,
            Style::default().fg(theme.fg_muted).add_modifier(Modifier::ITALIC),
        )));
    } else if app.editing_message.is_some() {
        block = block.title(Line::from(Span::styled(
            " editing… ",
            Style::default().fg(theme.accent_secondary).add_modifier(Modifier::ITALIC),
        )));
    }

    // Build attachment badge if present
    let badge = app.pending_attachment.as_ref().map(|path| {
        let fname = path.file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        // Detect type hint from extension
        let ext = path.extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let type_hint = match ext.as_str() {
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" => "image",
            "mp4" | "mov" | "avi" | "mkv" | "webm" => "video",
            "mp3" | "ogg" | "flac" | "wav" | "m4a" | "aac" => "audio",
            "pdf" | "doc" | "docx" | "txt" | "md" => "doc",
            _ => "file",
        };
        format!("[{type_hint}: {fname}] ")
    });
    let badge_len = badge.as_ref().map(|b| b.len()).unwrap_or(0);

    // Available width inside the border (minus border cells on each side)
    let inner_width = area.width.saturating_sub(2) as usize;
    let prefix = "> ";
    let prefix_len = prefix.len() + badge_len;
    app.mouse_input_prefix_len = prefix_len as u16;
    let text_width = inner_width.saturating_sub(prefix_len); // usable chars for buffer text

    if app.input_buffer.is_empty() && badge.is_none() {
        let placeholder = match app.mode {
            InputMode::Normal => "  Press i to type, / for commands",
            InputMode::Insert => "  Type a message...",
        };
        let input = Paragraph::new(Span::styled(
            placeholder,
            Style::default().fg(theme.fg_muted),
        ))
        .block(block);
        frame.render_widget(input, area);
    } else {
        let lines: Vec<&str> = app.input_buffer.split('\n').collect();
        let (cursor_line, cursor_col) = app.cursor_line_col();
        let visible_lines = area.height.saturating_sub(2) as usize;
        let vertical_scroll = if cursor_line >= visible_lines {
            cursor_line - visible_lines + 1
        } else {
            0
        };

        let mut text_lines: Vec<Line> = Vec::new();
        for (i, line_str) in lines.iter().enumerate() {
            let mut spans: Vec<Span> = Vec::new();
            if i == 0 {
                if let Some(ref badge_text) = badge {
                    spans.push(Span::styled(
                        badge_text.clone(),
                        Style::default().fg(theme.mention).add_modifier(Modifier::BOLD),
                    ));
                }
                spans.push(Span::styled(prefix, Style::default().fg(theme.fg)));
            } else {
                spans.push(Span::styled(
                    " ".repeat(prefix_len),
                    Style::default().fg(theme.fg),
                ));
            }

            if i == cursor_line {
                let char_scroll = cursor_col.saturating_sub(text_width);
                let visible_text: String = line_str.chars().skip(char_scroll).take(text_width).collect();
                spans.push(Span::styled(
                    visible_text,
                    Style::default().fg(theme.fg),
                ));
            } else {
                let visible_text: String = line_str.chars().take(text_width).collect();
                spans.push(Span::styled(
                    visible_text,
                    Style::default().fg(theme.fg),
                ));
            }
            text_lines.push(Line::from(spans));
        }

        let input = Paragraph::new(Text::from(text_lines))
            .block(block)
            .scroll((vertical_scroll as u16, 0));
        frame.render_widget(input, area);
    }

    // Place cursor (only visible in Insert mode)
    if app.mode == InputMode::Insert {
        let (cursor_line, cursor_col) = app.cursor_line_col();
        let visible_lines = area.height.saturating_sub(2) as usize;
        let vertical_scroll = if cursor_line >= visible_lines {
            cursor_line - visible_lines + 1
        } else {
            0
        };
        let line_scroll = cursor_col.saturating_sub(text_width);
        let cursor_x = area.x + 1 + prefix_len as u16 + (cursor_col - line_scroll) as u16;
        let cursor_y = area.y + 1 + (cursor_line - vertical_scroll) as u16;
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect, sidebar_auto_hidden: bool) {
    let theme = &app.theme;

    // Override status bar with quit confirmation prompt
    if app.quit_confirm {
        let bar = Line::from(Span::styled(
            " Unsent message in buffer. Press quit again to confirm.",
            Style::default().fg(theme.warning).add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(
            Paragraph::new(bar).style(Style::default().bg(theme.statusbar_bg)),
            area,
        );
        return;
    }

    let mut segments: Vec<Span> = Vec::new();

    // Mode indicator
    match app.mode {
        InputMode::Normal => {
            segments.push(Span::styled(
                " [NORMAL] ",
                Style::default().fg(theme.accent_secondary).add_modifier(Modifier::BOLD),
            ));
        }
        InputMode::Insert => {
            segments.push(Span::styled(
                " [INSERT] ",
                Style::default().fg(theme.success).add_modifier(Modifier::BOLD),
            ));
        }
    }
    segments.push(Span::styled("│ ", Style::default().fg(theme.fg_muted)));

    // Connection status dot
    if let Some(ref err) = app.connection_error {
        segments.push(Span::styled(" ● ", Style::default().fg(theme.error)));
        let display: String = err.chars().take(60).collect();
        segments.push(Span::styled(
            format!("error: {display}"),
            Style::default().fg(theme.error),
        ));
    } else if app.connected {
        segments.push(Span::styled(" ● ", Style::default().fg(theme.success)));
        segments.push(Span::styled("connected", Style::default().fg(theme.statusbar_fg)));
        if app.incognito {
            segments.push(Span::styled(" │ ", Style::default().fg(theme.fg_muted)));
            segments.push(Span::styled(
                "incognito",
                Style::default().fg(theme.mention).add_modifier(Modifier::BOLD),
            ));
        }
    } else {
        segments.push(Span::styled(" ● ", Style::default().fg(theme.error)));
        segments.push(Span::styled("disconnected", Style::default().fg(theme.statusbar_fg)));
    }

    // Pipe separator
    segments.push(Span::styled(" │ ", Style::default().fg(theme.fg_muted)));

    // Current conversation
    if let Some(ref id) = app.active_conversation {
        if let Some(conv) = app.conversations.get(id) {
            let prefix = if conv.is_group { "#" } else { "" };
            segments.push(Span::styled(
                format!("{}{}", prefix, conv.name),
                Style::default().fg(theme.accent),
            ));
        }
    } else {
        segments.push(Span::styled(
            "no conversation",
            Style::default().fg(theme.fg_muted),
        ));
    }

    // Pipe separator + conversation count
    if !app.conversation_order.is_empty() {
        segments.push(Span::styled(" │ ", Style::default().fg(theme.fg_muted)));
        segments.push(Span::styled(
            format!("{} chats", app.conversation_order.len()),
            Style::default().fg(theme.fg_secondary),
        ));
    }

    // Scroll offset indicator + focused message timestamp
    if app.scroll_offset > 0 {
        segments.push(Span::styled(" │ ", Style::default().fg(theme.fg_muted)));
        segments.push(Span::styled(
            format!("↑{}", app.scroll_offset),
            Style::default().fg(theme.warning),
        ));
        if let Some(ref ts) = app.focused_message_time {
            let local = ts.with_timezone(&chrono::Local);
            segments.push(Span::styled(" │ ", Style::default().fg(theme.fg_muted)));
            segments.push(Span::styled(
                local.format("%a %b %d, %Y %I:%M:%S %p").to_string(),
                Style::default().fg(theme.statusbar_fg),
            ));
        }
    }

    // Auto-hidden sidebar indicator
    if sidebar_auto_hidden && app.sidebar_visible {
        segments.push(Span::styled(" │ ", Style::default().fg(theme.fg_muted)));
        segments.push(Span::styled(
            "[+]",
            Style::default().fg(theme.fg_muted),
        ));
    }

    // Pad the rest with background
    let status = Paragraph::new(Line::from(segments)).style(
        Style::default()
            .fg(theme.statusbar_fg)
            .bg(theme.statusbar_bg),
    );
    frame.render_widget(status, area);
}

fn draw_autocomplete(frame: &mut Frame, app: &App, input_area: Rect) {
    let theme = &app.theme;
    let terminal_width = frame.area().width;
    let mut lines: Vec<Line> = Vec::new();
    let mut max_content_width: usize = 0;

    match app.autocomplete_mode {
        AutocompleteMode::Command => {
            for (i, &cmd_idx) in app.autocomplete_candidates.iter().enumerate() {
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

                let is_selected = i == app.autocomplete_index;
                let style = if is_selected {
                    Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
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
            for (i, (phone, name, _uuid)) in app.mention_candidates.iter().enumerate() {
                let left = format!("  @{name}");
                let right = format!("  {phone}");
                let total_len = left.len() + right.len() + 2;
                if total_len > max_content_width {
                    max_content_width = total_len;
                }

                let is_selected = i == app.autocomplete_index;
                let style = if is_selected {
                    Style::default().bg(theme.bg_selected).fg(theme.accent).add_modifier(Modifier::BOLD)
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
            for (i, (display, _value)) in app.join_candidates.iter().enumerate() {
                let left = format!("  {display}");
                let total_len = left.len() + 2;
                if total_len > max_content_width {
                    max_content_width = total_len;
                }

                let is_selected = i == app.autocomplete_index;
                let style = if is_selected {
                    Style::default().bg(theme.bg_selected).fg(theme.success).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.success)
                };

                lines.push(Line::from(vec![
                    Span::styled(left, style),
                ]));
            }
        }
    }

    let count = lines.len();

    // Size the popup, clamping to available space
    let terminal_height = frame.area().height;
    let popup_width = (max_content_width as u16 + 2).min(terminal_width.saturating_sub(2)).max(20);
    let popup_height = ((count as u16) + 2).min(input_area.y).min(terminal_height); // +2 for border
    if popup_height < 3 {
        return; // not enough space to render anything useful
    }

    // Position above the input box, left-aligned with it
    let x = input_area.x;
    let y = input_area.y.saturating_sub(popup_height);

    let area = Rect::new(x, y, popup_width.min(terminal_width.saturating_sub(x)), popup_height);
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

fn draw_settings(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let height = SETTINGS_POPUP_HEIGHT + 5; // extra lines for preview + theme + keybindings + profile + hint
    let (popup_area, block) = centered_popup(
        frame, area, SETTINGS_POPUP_WIDTH, height, " Settings ", theme,
    );

    let mut lines: Vec<Line> = Vec::new();
    for (i, def) in SETTINGS.iter().enumerate() {
        let enabled = app.setting_value(i);
        let checkbox = if enabled { "[x]" } else { "[ ]" };
        let is_selected = i == app.settings_index;
        let style = if is_selected {
            Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg_secondary)
        };
        let check_style = if is_selected {
            Style::default().bg(theme.bg_selected).fg(theme.accent).add_modifier(Modifier::BOLD)
        } else if enabled {
            Style::default().fg(theme.success)
        } else {
            Style::default().fg(theme.fg_muted)
        };

        lines.push(Line::from(vec![
            Span::styled(format!("  {} ", checkbox), check_style),
            Span::styled(def.label.to_string(), style),
        ]));
    }

    // Notification preview cycle entry (index == SETTINGS.len())
    let preview_index = SETTINGS.len();
    let is_preview_selected = app.settings_index == preview_index;
    let preview_style = if is_preview_selected {
        Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg_secondary)
    };
    let preview_value_style = if is_preview_selected {
        Style::default().bg(theme.bg_selected).fg(theme.accent)
    } else {
        Style::default().fg(theme.accent)
    };
    lines.push(Line::from(vec![
        Span::styled("  Notification preview: ", preview_style),
        Span::styled(app.notification_preview.clone(), preview_value_style),
    ]));

    // Theme selector entry (index == SETTINGS.len() + 1)
    let is_theme_selected = app.settings_index == SETTINGS.len() + 1;
    let theme_style = if is_theme_selected {
        Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg_secondary)
    };
    let theme_value_style = if is_theme_selected {
        Style::default().bg(theme.bg_selected).fg(theme.accent)
    } else {
        Style::default().fg(theme.accent)
    };
    lines.push(Line::from(vec![
        Span::styled("  Theme: ", theme_style),
        Span::styled(app.theme.name.clone(), theme_value_style),
    ]));

    // Keybindings selector entry (index == SETTINGS.len() + 2)
    let is_kb_selected = app.settings_index == SETTINGS.len() + 2;
    let kb_style = if is_kb_selected {
        Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg_secondary)
    };
    let kb_value_style = if is_kb_selected {
        Style::default().bg(theme.bg_selected).fg(theme.accent)
    } else {
        Style::default().fg(theme.accent)
    };
    lines.push(Line::from(vec![
        Span::styled("  Keybindings: ", kb_style),
        Span::styled(app.keybindings.profile_name.clone(), kb_value_style),
    ]));

    // Settings profile selector entry (index == SETTINGS.len() + 3)
    let is_profile_selected = app.settings_index == SETTINGS.len() + 3;
    let profile_style = if is_profile_selected {
        Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg_secondary)
    };
    let profile_value_style = if is_profile_selected {
        Style::default().bg(theme.bg_selected).fg(theme.accent)
    } else {
        Style::default().fg(theme.accent)
    };
    lines.push(Line::from(vec![
        Span::styled("  Profile: ", profile_style),
        Span::styled(app.settings_profile_name.clone(), profile_value_style),
    ]));

    // Hint line for the currently selected item
    let hint = if app.settings_index < SETTINGS.len() {
        SETTINGS[app.settings_index].hint
    } else {
        match app.settings_index - SETTINGS.len() {
            0 => "Control message content in notifications",
            1 => "Switch between color themes",
            2 => "Switch between keybinding presets",
            3 => "h/l cycle, Enter manage settings profiles",
            _ => "",
        }
    };
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {hint}"),
        Style::default().fg(theme.fg_muted).add_modifier(Modifier::ITALIC),
    )));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let kb = &app.keybindings;

    // Help table entries: (key, description)
    let commands: &[(&str, &str)] = &[
        ("/join <name>", "Switch to a conversation"),
        ("/part", "Leave current conversation"),
        ("/attach", "Attach a file"),
        ("/search <query>", "Search messages"),
        ("/sidebar", "Toggle sidebar visibility"),
        ("/bell [type]", "Toggle notifications"),
        ("/mute", "Mute/unmute conversation"),
        ("/contacts", "Browse contacts"),
        ("/settings", "Open settings"),
        ("/keybindings", "Configure keybindings"),
        ("/quit", "Exit siggy"),
    ];

    // Dynamic shortcuts from active keybindings
    let dk = |a: KeyAction| kb.display_key(a);
    let nav_keys = format!("{} / {}", dk(KeyAction::NextConversation), dk(KeyAction::PrevConversation));
    let scroll_keys = format!("{} / {}", dk(KeyAction::PageScrollUp), dk(KeyAction::PageScrollDown));
    let resize_keys = format!("{} / {}", dk(KeyAction::ResizeSidebarLeft), dk(KeyAction::ResizeSidebarRight));
    let quit_key = dk(KeyAction::Quit);
    let shortcuts: Vec<(String, &str)> = vec![
        (nav_keys, "Next / prev conversation"),
        ("Up / Down".to_string(), "Recall input history"),
        ("@".to_string(), "Mention autocomplete"),
        (scroll_keys, "Scroll messages"),
        (resize_keys, "Resize sidebar"),
        (quit_key, "Quit"),
    ];

    let cli: &[(&str, &str)] = &[
        ("--incognito", "No local message storage"),
        ("--demo", "Launch with dummy data"),
        ("--setup", "Re-run first-time wizard"),
    ];

    // Dynamic normal-mode keybindings
    let exit_key = dk(KeyAction::ExitInsert);
    let insert_keys = format!("{} / {} / {} / {} / {}",
        dk(KeyAction::InsertAtCursor), dk(KeyAction::InsertAfterCursor),
        dk(KeyAction::InsertLineStart), dk(KeyAction::InsertLineEnd),
        dk(KeyAction::OpenLineBelow));
    let scroll_ud = format!("{} / {}", dk(KeyAction::ScrollDown), dk(KeyAction::ScrollUp));
    let focus_ud = format!("{} / {}", dk(KeyAction::FocusNextMessage), dk(KeyAction::FocusPrevMessage));
    let top_bottom = format!("{} / {}", dk(KeyAction::ScrollToTop), dk(KeyAction::ScrollToBottom));
    let half_page = format!("{} / {}", dk(KeyAction::HalfPageDown), dk(KeyAction::HalfPageUp));
    let cursor_lr = format!("{} / {}", dk(KeyAction::CursorLeft), dk(KeyAction::CursorRight));
    let word_fb = format!("{} / {}", dk(KeyAction::WordForward), dk(KeyAction::WordBack));
    let line_se = format!("{} / {}", dk(KeyAction::LineStart), dk(KeyAction::LineEnd));
    let del_keys = format!("{} / {}", dk(KeyAction::DeleteChar), dk(KeyAction::DeleteToEnd));
    let copy_keys = format!("{} / {}", dk(KeyAction::CopyMessage), dk(KeyAction::CopyAllMessages));
    let search_keys = format!("{} / {}", dk(KeyAction::NextSearchResult), dk(KeyAction::PrevSearchResult));

    let profile_label = format!("  Keybindings [{}]", app.keybindings.profile_name);
    let vim: Vec<(String, &str)> = vec![
        (exit_key, "Normal mode"),
        (insert_keys, "Insert mode"),
        (scroll_ud, "Scroll up / down"),
        (focus_ud, "Prev / next message"),
        (top_bottom, "Top / bottom of messages"),
        (half_page, "Half-page scroll"),
        (cursor_lr, "Cursor left / right"),
        (word_fb, "Word forward / back"),
        (line_se, "Start / end of line"),
        (del_keys, "Delete char / to end"),
        (copy_keys, "Copy message / full line"),
        (dk(KeyAction::React), "React to focused message"),
        (dk(KeyAction::Quote), "Reply / quote message"),
        (dk(KeyAction::EditMessage), "Edit own message"),
        (dk(KeyAction::DeleteMessage), "Delete message"),
        (search_keys, "Next / prev search match"),
        (dk(KeyAction::StartSearch), "Start command input"),
    ];

    // Calculate popup size
    let key_col_width = 20;
    let desc_col_width = 28;
    let pref_width = (key_col_width + desc_col_width + 6) as u16;
    let content_lines =
        commands.len() + shortcuts.len() + vim.len() + cli.len() + 7;
    let pref_height = content_lines as u16 + 2;

    let (popup_area, block) = centered_popup(frame, area, pref_width, pref_height, " Help ", theme);

    let header_style = Style::default()
        .fg(theme.accent_secondary)
        .add_modifier(Modifier::BOLD);
    let key_style = Style::default().fg(theme.accent);
    let desc_style = Style::default().fg(theme.fg_secondary);

    let mut lines: Vec<Line> = Vec::new();

    let push_row = |lines: &mut Vec<Line>, key: &str, desc: &str| {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<width$}", key, width = key_col_width), key_style),
            Span::styled(desc.to_string(), desc_style),
        ]));
    };

    lines.push(Line::from(Span::styled("  Commands", header_style)));
    for &(key, desc) in commands {
        push_row(&mut lines, key, desc);
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Shortcuts", header_style)));
    for (key, desc) in &shortcuts {
        push_row(&mut lines, key, desc);
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(&profile_label, header_style)));
    for (key, desc) in &vim {
        push_row(&mut lines, key, desc);
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  CLI Options", header_style)));
    for &(key, desc) in cli {
        push_row(&mut lines, key, desc);
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Press any key to close",
        Style::default().fg(theme.fg_muted),
    )));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

fn draw_contacts(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let max_visible = CONTACTS_MAX_VISIBLE.min(app.contacts_filtered.len());
    let pref_height = max_visible as u16 + 5; // +3 border/title +2 footer/filter

    let title = if app.contacts_filter.is_empty() {
        " Contacts ".to_string()
    } else {
        format!(" Contacts [{}] ", app.contacts_filter)
    };

    let (popup_area, block) = centered_popup(
        frame, area, CONTACTS_POPUP_WIDTH, pref_height, &title, theme,
    );

    let inner_height = popup_area.height.saturating_sub(2) as usize; // minus borders
    let footer_lines = 2; // footer + empty line
    let visible_rows = inner_height.saturating_sub(footer_lines);

    // Scroll the list so the selected item is always visible
    let scroll_offset = if app.contacts_index >= visible_rows {
        app.contacts_index - visible_rows + 1
    } else {
        0
    };

    let mut lines: Vec<Line> = Vec::new();

    if app.contacts_filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No contacts found",
            Style::default().fg(theme.fg_muted),
        )));
    } else {
        let end = (scroll_offset + visible_rows).min(app.contacts_filtered.len());
        let inner_w = popup_area.width.saturating_sub(2) as usize;

        for (i, (number, name)) in app.contacts_filtered[scroll_offset..end].iter().enumerate() {
            let actual_index = scroll_offset + i;
            let is_selected = actual_index == app.contacts_index;
            let has_conversation = app.conversation_order.contains(number);

            // Checkmark for contacts that already have a conversation
            let marker = if has_conversation { " \u{2713}" } else { "  " };
            let marker_style = if has_conversation {
                Style::default().fg(theme.success)
            } else {
                Style::default()
            };

            // Truncate name to fit with number and marker
            let number_display = format!("  {}", number);
            let name_max = inner_w.saturating_sub(number_display.len() + marker.len() + 2);
            let display_name = truncate(name, name_max);

            let name_style = if is_selected {
                Style::default()
                    .bg(theme.bg_selected)
                    .fg(theme.fg)
                    .add_modifier(Modifier::BOLD)
            } else if has_conversation {
                Style::default().fg(theme.fg_secondary)
            } else {
                Style::default().fg(theme.fg)
            };
            let number_style = if is_selected {
                Style::default().bg(theme.bg_selected).fg(theme.accent)
            } else {
                Style::default().fg(theme.fg_muted)
            };
            let marker_bg = if is_selected {
                marker_style.bg(theme.bg_selected)
            } else {
                marker_style
            };

            lines.push(Line::from(vec![
                Span::styled(format!("  {}", display_name), name_style),
                Span::styled(number_display, number_style),
                Span::styled(marker.to_string(), marker_bg),
            ]));
        }
    }

    // Pad to fill visible_rows so footer is always at the bottom
    while lines.len() < visible_rows {
        lines.push(Line::from(""));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  j/k navigate  |  Enter select  |  Esc close",
        Style::default().fg(theme.fg_muted),
    )));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

fn draw_verify(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let is_group = app.active_conversation.as_ref()
        .and_then(|id| app.conversations.get(id))
        .map(|c| c.is_group)
        .unwrap_or(false);

    let pref_height: u16 = if is_group { 18 } else { 14 };
    let pref_width: u16 = 50;
    let (popup_area, block) = centered_popup(
        frame, area, pref_width, pref_height, " Verify Identity ", theme,
    );
    let inner = popup_area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
    let mut lines: Vec<Line> = Vec::new();

    if app.verify_identities.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No identity information available",
            Style::default().fg(theme.fg_muted),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Esc: close",
            Style::default().fg(theme.fg_muted),
        )));
    } else if is_group {
        // Group view: scrollable member list with trust badges
        let member_rows = inner.height.saturating_sub(7) as usize; // reserve for safety number + footer
        let scroll_offset = if app.verify_index >= member_rows {
            app.verify_index - member_rows + 1
        } else {
            0
        };
        let end = (scroll_offset + member_rows).min(app.verify_identities.len());

        for (i, identity) in app.verify_identities[scroll_offset..end].iter().enumerate() {
            let actual_idx = scroll_offset + i;
            let is_selected = actual_idx == app.verify_index;
            let number = identity.number.as_deref().unwrap_or("unknown");
            let name = app.contact_names.get(number).cloned().unwrap_or_else(|| number.to_string());
            let (badge, badge_color) = match identity.trust_level {
                TrustLevel::TrustedVerified => ("\u{2713}", theme.accent),
                TrustLevel::Untrusted => ("\u{26A0}", theme.warning),
                TrustLevel::TrustedUnverified => ("\u{2500}", theme.fg_muted),
            };
            let prefix = if is_selected { "> " } else { "  " };
            let style = if is_selected {
                Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            let badge_style = if is_selected {
                Style::default().bg(theme.bg_selected).fg(badge_color)
            } else {
                Style::default().fg(badge_color)
            };
            lines.push(Line::from(vec![
                Span::styled(prefix.to_string(), style),
                Span::styled(format!("{badge} "), badge_style),
                Span::styled(name, style),
            ]));
        }

        lines.push(Line::from(""));

        // Show selected member's safety number
        if let Some(identity) = app.verify_identities.get(app.verify_index) {
            if !identity.safety_number.is_empty() {
                lines.push(Line::from(Span::styled("  Safety Number:", Style::default().fg(theme.fg_secondary))));
                let sn = &identity.safety_number;
                let formatted = format_safety_number(sn);
                for row in formatted {
                    lines.push(Line::from(Span::styled(format!("  {row}"), Style::default().fg(theme.fg))));
                }
            } else {
                lines.push(Line::from(Span::styled("  Safety number not available", Style::default().fg(theme.fg_muted))));
            }
        }

        lines.push(Line::from(""));
        if app.verify_confirming {
            lines.push(Line::from(Span::styled(
                "  Compare safety numbers, then press v to confirm",
                Style::default().fg(theme.warning),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "  j/k: navigate  v: verify  Esc: close",
                Style::default().fg(theme.fg_muted),
            )));
        }
    } else {
        // 1:1 view: single identity with full details
        let identity = &app.verify_identities[0];
        let number = identity.number.as_deref().unwrap_or("unknown");
        let name = app.contact_names.get(number).cloned().unwrap_or_else(|| number.to_string());

        lines.push(Line::from(Span::styled(
            format!("  {} ({})", name, number),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        )));

        let (trust_label, trust_color) = match identity.trust_level {
            TrustLevel::TrustedVerified => ("\u{2713} Verified", theme.accent),
            TrustLevel::Untrusted => ("\u{26A0} Untrusted", theme.warning),
            TrustLevel::TrustedUnverified => ("\u{2500} Unverified", theme.fg_muted),
        };
        lines.push(Line::from(Span::styled(
            format!("  Trust: {trust_label}"),
            Style::default().fg(trust_color),
        )));
        lines.push(Line::from(""));

        if !identity.safety_number.is_empty() {
            lines.push(Line::from(Span::styled("  Safety Number:", Style::default().fg(theme.fg_secondary))));
            let formatted = format_safety_number(&identity.safety_number);
            for row in formatted {
                lines.push(Line::from(Span::styled(format!("  {row}"), Style::default().fg(theme.fg))));
            }
        } else {
            lines.push(Line::from(Span::styled("  Safety number not available", Style::default().fg(theme.fg_muted))));
        }

        lines.push(Line::from(""));
        if !identity.fingerprint.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("  Fingerprint: {}", identity.fingerprint),
                Style::default().fg(theme.fg_muted),
            )));
            lines.push(Line::from(""));
        }

        if app.verify_confirming {
            lines.push(Line::from(Span::styled(
                "  Compare safety numbers, then press v to confirm",
                Style::default().fg(theme.warning),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "  v: verify key  Esc: close",
                Style::default().fg(theme.fg_muted),
            )));
        }
    }

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

/// Format a safety number string as groups of 5 digits, 6 per line.
fn format_safety_number(sn: &str) -> Vec<String> {
    let digits: String = sn.chars().filter(|c| c.is_ascii_digit()).collect();
    let chunks: Vec<&str> = digits.as_bytes()
        .chunks(5)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or(""))
        .collect();
    chunks.chunks(6)
        .map(|row| row.join(" "))
        .collect()
}

fn draw_search(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let max_visible = SEARCH_MAX_VISIBLE.min(app.search.results.len().max(1));
    let pref_height = max_visible as u16 + 5; // +3 border/title +2 footer

    let title = if app.search.query.is_empty() {
        " Search ".to_string()
    } else {
        format!(" Search [{}] ", app.search.query)
    };

    let (popup_area, block) = centered_popup(
        frame, area, SEARCH_POPUP_WIDTH, pref_height, &title, theme,
    );

    let inner_height = popup_area.height.saturating_sub(2) as usize; // minus borders
    let footer_lines = 2; // footer + empty line
    let visible_rows = inner_height.saturating_sub(footer_lines);

    // Scroll the list so the selected item is always visible
    let scroll_offset = if app.search.index >= visible_rows {
        app.search.index - visible_rows + 1
    } else {
        0
    };

    let mut lines: Vec<Line> = Vec::new();
    let inner_w = popup_area.width.saturating_sub(2) as usize;

    if app.search.results.is_empty() {
        let msg = if app.search.query.is_empty() {
            "  Type to search..."
        } else {
            "  No results found"
        };
        lines.push(Line::from(Span::styled(
            msg,
            Style::default().fg(theme.fg_muted),
        )));
    } else {
        let end = (scroll_offset + visible_rows).min(app.search.results.len());

        for (i, result) in app.search.results[scroll_offset..end].iter().enumerate() {
            let actual_index = scroll_offset + i;
            let is_selected = actual_index == app.search.index;

            // Format: [conv_name] sender: body_snippet
            let conv_prefix = if app.active_conversation.is_some() {
                String::new()
            } else {
                format!("[{}] ", truncate(&result.conv_name, 12))
            };

            let sender_display = truncate(&result.sender, 10);
            let prefix = format!("  {conv_prefix}{sender_display}: ");
            let body_max = inner_w.saturating_sub(prefix.len());
            // Show a snippet of the body around the match
            let body_snippet = search_snippet(&result.body, &app.search.query, body_max);

            let prefix_style = if is_selected {
                Style::default().bg(theme.bg_selected).fg(theme.accent)
            } else {
                Style::default().fg(theme.accent)
            };
            let body_style = if is_selected {
                Style::default().bg(theme.bg_selected).fg(theme.fg)
            } else {
                Style::default().fg(theme.fg_secondary)
            };

            // Build spans with highlighted match
            let mut spans = vec![Span::styled(prefix, prefix_style)];
            spans.extend(highlight_match_spans(&body_snippet, &app.search.query, body_style, is_selected, theme));

            lines.push(Line::from(spans));
        }
    }

    // Pad to fill visible_rows so footer is always at the bottom
    while lines.len() < visible_rows {
        lines.push(Line::from(""));
    }

    lines.push(Line::from(""));
    let count_text = if app.search.results.is_empty() {
        String::new()
    } else {
        format!("  {}/{}", app.search.index + 1, app.search.results.len())
    };
    lines.push(Line::from(vec![
        Span::styled(
            count_text,
            Style::default().fg(theme.warning),
        ),
        Span::styled(
            "  j/k nav | Enter jump | n/N cycle | Esc close",
            Style::default().fg(theme.fg_muted),
        ),
    ]));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

/// Extract a snippet of text centered around the first match of `query`.
pub(crate) fn search_snippet(body: &str, query: &str, max_len: usize) -> String {
    let char_count = body.chars().count();
    if char_count <= max_len {
        return body.to_string();
    }

    // Find the match position in char indices (case-insensitive)
    let body_lower = body.to_lowercase();
    let query_lower = query.to_lowercase();
    let match_byte_pos = body_lower.find(&query_lower).unwrap_or(0);
    // Convert byte position in lowered text to char index
    let match_char_pos = body_lower[..match_byte_pos].chars().count();

    // Center the snippet around the match
    let half = max_len / 2;
    let start = match_char_pos.saturating_sub(half);
    let end = (start + max_len).min(char_count);
    let start = if end == char_count {
        end.saturating_sub(max_len)
    } else {
        start
    };

    let snippet: String = body.chars().skip(start).take(end - start).collect();
    let mut result = snippet;
    if start > 0 {
        result = format!("…{}", result.chars().skip(1).collect::<String>());
    }
    if end < char_count {
        let trimmed: String = result.chars().take(result.chars().count().saturating_sub(1)).collect();
        result = format!("{trimmed}…");
    }
    result
}

/// Build spans with the matching portions highlighted.
/// Uses character-level case-insensitive matching to avoid byte-boundary issues.
fn highlight_match_spans<'a>(
    text: &str,
    query: &str,
    base_style: Style,
    is_selected: bool,
    theme: &Theme,
) -> Vec<Span<'a>> {
    if query.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }

    let match_style = if is_selected {
        Style::default()
            .bg(theme.bg_selected)
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD)
    };

    // Find all match positions using the lowercased strings
    let text_lower = text.to_lowercase();
    let query_lower = query.to_lowercase();
    let query_len = query_lower.len();

    // Collect match byte ranges in the lowered text, then map back to original
    // For ASCII and most Unicode, to_lowercase preserves byte length per-character.
    // Use the lowered text offsets directly since they share the same structure.
    let mut match_ranges: Vec<(usize, usize)> = Vec::new();
    let mut search_pos = 0;
    while search_pos < text_lower.len() {
        if let Some(m) = text_lower[search_pos..].find(&query_lower) {
            let start = search_pos + m;
            let end = start + query_len;
            match_ranges.push((start, end));
            search_pos = end;
        } else {
            break;
        }
    }

    if match_ranges.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }

    // Build a char-index mapping: for each char, record its byte start in original and lowered
    let orig_chars: Vec<(usize, char)> = text.char_indices().collect();
    let lower_chars: Vec<(usize, char)> = text_lower.char_indices().collect();

    // Build byte-position mapping from lowered → original using char alignment
    // Both should have the same number of chars
    let char_count = orig_chars.len().min(lower_chars.len());

    // Convert match_ranges from lowered byte positions to original byte positions
    let mut orig_ranges: Vec<(usize, usize)> = Vec::new();
    for &(low_start, low_end) in &match_ranges {
        let start_char = lower_chars.iter().position(|&(pos, _)| pos == low_start);
        let end_char = lower_chars.iter().position(|&(pos, _)| pos == low_end)
            .unwrap_or(char_count);
        if let Some(sc) = start_char {
            let orig_start = orig_chars[sc].0;
            let orig_end = if end_char < orig_chars.len() {
                orig_chars[end_char].0
            } else {
                text.len()
            };
            orig_ranges.push((orig_start, orig_end));
        }
    }

    // Build spans from original ranges
    let mut spans = Vec::new();
    let mut pos = 0;
    for (start, end) in orig_ranges {
        if start > pos {
            spans.push(Span::styled(text[pos..start].to_string(), base_style));
        }
        spans.push(Span::styled(text[start..end].to_string(), match_style));
        pos = end;
    }
    if pos < text.len() {
        spans.push(Span::styled(text[pos..].to_string(), base_style));
    }

    if spans.is_empty() {
        spans.push(Span::styled(text.to_string(), base_style));
    }
    spans
}

/// Format a file size in human-readable form (B, K, M, G).
pub(crate) fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{}K", bytes / 1024)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn draw_file_browser(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let visible_count = FILE_BROWSER_MAX_VISIBLE.min(
        if app.file_picker.filtered.is_empty() { 1 } else { app.file_picker.filtered.len() }
    );
    let pref_height = visible_count as u16 + 5; // border + header + footer

    let title = if app.file_picker.filter.is_empty() {
        " Attach File ".to_string()
    } else {
        format!(" Attach File [{}] ", app.file_picker.filter)
    };

    let (popup_area, block) = centered_popup(
        frame, area, FILE_BROWSER_POPUP_WIDTH, pref_height, &title, theme,
    );

    let inner_height = popup_area.height.saturating_sub(2) as usize;
    let header_lines = 1; // path header
    let footer_lines = 2; // empty + key hints
    let visible_rows = inner_height.saturating_sub(header_lines + footer_lines);
    let inner_w = popup_area.width.saturating_sub(2) as usize;

    let mut lines: Vec<Line> = Vec::new();

    // Current path header
    let dir_display = app.file_picker.dir.to_string_lossy();
    let dir_truncated = truncate(&dir_display, inner_w.saturating_sub(2));
    lines.push(Line::from(Span::styled(
        format!("  {dir_truncated}"),
        Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
    )));

    if let Some(ref err) = app.file_picker.error {
        lines.push(Line::from(Span::styled(
            format!("  {}", truncate(err, inner_w.saturating_sub(2))),
            Style::default().fg(theme.error),
        )));
    } else if app.file_picker.filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Empty directory",
            Style::default().fg(theme.fg_muted),
        )));
    } else {
        // Scroll the list so the selected item is always visible
        let scroll_offset = if app.file_picker.index >= visible_rows {
            app.file_picker.index - visible_rows + 1
        } else {
            0
        };

        let end = (scroll_offset + visible_rows).min(app.file_picker.filtered.len());

        for (i, &entry_idx) in app.file_picker.filtered[scroll_offset..end].iter().enumerate() {
            let actual_index = scroll_offset + i;
            let is_selected = actual_index == app.file_picker.index;
            let (ref name, is_dir, size) = app.file_picker.entries[entry_idx];

            let size_str = if is_dir {
                String::new()
            } else {
                format_file_size(size)
            };

            let display_name = if is_dir {
                format!("{name}/")
            } else {
                name.clone()
            };

            // Leave room for size column
            let size_col_width = 8;
            let name_max = inner_w.saturating_sub(size_col_width + 4);
            let display_name = truncate(&display_name, name_max);

            let name_style = if is_selected {
                if is_dir {
                    Style::default().bg(theme.bg_selected).fg(theme.accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
                }
            } else if is_dir {
                Style::default().fg(theme.accent)
            } else {
                Style::default().fg(theme.fg)
            };

            let size_style = if is_selected {
                Style::default().bg(theme.bg_selected).fg(theme.fg_muted)
            } else {
                Style::default().fg(theme.fg_muted)
            };

            // Pad name to align size column
            let name_padded = format!("  {display_name:width$}", width = name_max);
            let size_padded = format!("{size_str:>width$}  ", width = size_col_width);

            lines.push(Line::from(vec![
                Span::styled(name_padded, name_style),
                Span::styled(size_padded, size_style),
            ]));
        }
    }

    // Pad to fill visible rows
    while lines.len() < header_lines + visible_rows {
        lines.push(Line::from(""));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  j/k nav  Enter open/select  Backspace/- up  Esc cancel",
        Style::default().fg(theme.fg_muted),
    )));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

fn draw_theme_picker(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let max_visible = 12usize.min(app.available_themes.len());
    let pref_height = max_visible as u16 + 5; // border + title + footer

    let (popup_area, block) = centered_popup(
        frame, area, 50, pref_height, " Theme ", theme,
    );

    let inner_height = popup_area.height.saturating_sub(2) as usize;
    let footer_lines = 2;
    let visible_rows = inner_height.saturating_sub(footer_lines);

    // Scroll the list so the selected item is always visible
    let scroll_offset = if app.theme_index >= visible_rows {
        app.theme_index - visible_rows + 1
    } else {
        0
    };

    let mut lines: Vec<Line> = Vec::new();

    let end = (scroll_offset + visible_rows).min(app.available_themes.len());
    for (i, t) in app.available_themes[scroll_offset..end].iter().enumerate() {
        let actual_index = scroll_offset + i;
        let is_selected = actual_index == app.theme_index;
        let is_active = t.name == app.theme.name;

        let marker = if is_active { "[*]" } else { "[ ]" };
        let row_style = if is_selected {
            Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };
        let marker_style = if is_selected {
            Style::default().bg(theme.bg_selected).fg(if is_active { theme.success } else { theme.fg_muted })
        } else {
            Style::default().fg(if is_active { theme.success } else { theme.fg_muted })
        };

        // Color swatches: show accent, success, error as colored blocks
        let swatch_bg = if is_selected { theme.bg_selected } else { theme.bg };
        let swatch_accent = Span::styled("\u{2588}\u{2588}", Style::default().fg(t.accent).bg(swatch_bg));
        let swatch_success = Span::styled("\u{2588}\u{2588}", Style::default().fg(t.success).bg(swatch_bg));
        let swatch_error = Span::styled("\u{2588}\u{2588}", Style::default().fg(t.error).bg(swatch_bg));

        // Pad name to align swatches
        let name_width = 28;
        let display_name = truncate(&t.name, name_width);
        let padded_name = format!("{display_name:width$}", width = name_width);

        lines.push(Line::from(vec![
            Span::styled(format!("  {marker} "), marker_style),
            Span::styled(padded_name, row_style),
            Span::raw(" "),
            swatch_accent,
            Span::raw(" "),
            swatch_success,
            Span::raw(" "),
            swatch_error,
        ]));
    }

    // Pad to fill visible rows
    while lines.len() < visible_rows {
        lines.push(Line::from(""));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  j/k navigate  |  Enter apply  |  Esc cancel",
        Style::default().fg(theme.fg_muted),
    )));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

fn draw_keybindings(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    // If the profile picker sub-overlay is open, draw it instead
    if app.keybindings_profile_picker {
        draw_keybindings_profile_picker(frame, app, area);
        return;
    }

    let total_rows = app.keybindings_overlay_total();
    let max_visible = 24usize.min(total_rows);
    let pref_height = max_visible as u16 + 4; // borders + footer
    let pref_width = 52;

    let (popup_area, block) = centered_popup(
        frame, area, pref_width, pref_height, " Keybindings ", theme,
    );

    let inner_height = popup_area.height.saturating_sub(2) as usize;
    let footer_lines = 2;
    let visible_rows = inner_height.saturating_sub(footer_lines);

    let scroll_offset = if app.keybindings_index >= visible_rows {
        app.keybindings_index - visible_rows + 1
    } else {
        0
    };

    let mut lines: Vec<Line> = Vec::new();
    let key_col_width = 26;
    let val_col_width = 20;

    let end = (scroll_offset + visible_rows).min(total_rows);
    for row in scroll_offset..end {
        let is_selected = row == app.keybindings_index;
        let (mode, action): (BindingMode, Option<KeyAction>) = app.keybindings_overlay_item(row);

        if row == 0 {
            // Profile row
            let style = if is_selected {
                Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg_secondary)
            };
            let val_style = if is_selected {
                Style::default().bg(theme.bg_selected).fg(theme.accent)
            } else {
                Style::default().fg(theme.accent)
            };
            lines.push(Line::from(vec![
                Span::styled("  Profile: ", style),
                Span::styled(app.keybindings.profile_name.clone(), val_style),
            ]));
        } else if action.is_none() {
            // Section header
            let label = match mode {
                BindingMode::Global => "Global",
                BindingMode::Normal => "Normal Mode",
                BindingMode::Insert => "Insert Mode",
            };
            let header_style = Style::default()
                .fg(theme.accent_secondary)
                .add_modifier(Modifier::BOLD);
            lines.push(Line::from(Span::styled(format!("  -- {label} --"), header_style)));
        } else {
            // Action row
            let action = action.unwrap();
            let label = keybindings::action_label(action);
            let key_display = if is_selected && app.keybindings_capturing {
                "[Press key...]".to_string()
            } else {
                app.keybindings.display_key(action)
            };

            let row_style = if is_selected {
                Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg_secondary)
            };
            let key_style = if is_selected {
                Style::default().bg(theme.bg_selected).fg(theme.accent)
            } else {
                Style::default().fg(theme.accent)
            };

            let padded_label = format!("{label:width$}", width = key_col_width);
            lines.push(Line::from(vec![
                Span::styled(format!("  {padded_label}"), row_style),
                Span::styled(format!("{key_display:>width$}", width = val_col_width), key_style),
            ]));
        }
    }

    // Pad
    while lines.len() < visible_rows {
        lines.push(Line::from(""));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Enter rebind | Backspace reset | Esc close",
        Style::default().fg(theme.fg_muted),
    )));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

fn draw_keybindings_profile_picker(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let max_visible = 8usize.min(app.available_kb_profiles.len());
    let pref_height = max_visible as u16 + 5;

    let (popup_area, block) = centered_popup(
        frame, area, 36, pref_height, " Keybinding Profile ", theme,
    );

    let inner_height = popup_area.height.saturating_sub(2) as usize;
    let footer_lines = 2;
    let visible_rows = inner_height.saturating_sub(footer_lines);

    let scroll_offset = if app.keybindings_profile_index >= visible_rows {
        app.keybindings_profile_index - visible_rows + 1
    } else {
        0
    };

    let mut lines: Vec<Line> = Vec::new();
    let end = (scroll_offset + visible_rows).min(app.available_kb_profiles.len());
    for i in scroll_offset..end {
        let is_selected = i == app.keybindings_profile_index;
        let is_active = app.available_kb_profiles[i] == app.keybindings.profile_name;
        let marker = if is_active { "[*]" } else { "[ ]" };

        let row_style = if is_selected {
            Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };
        let marker_style = if is_selected {
            Style::default().bg(theme.bg_selected).fg(if is_active { theme.success } else { theme.fg_muted })
        } else {
            Style::default().fg(if is_active { theme.success } else { theme.fg_muted })
        };

        lines.push(Line::from(vec![
            Span::styled(format!("  {marker} "), marker_style),
            Span::styled(app.available_kb_profiles[i].clone(), row_style),
        ]));
    }

    while lines.len() < visible_rows {
        lines.push(Line::from(""));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  j/k navigate  |  Enter apply  |  Esc cancel",
        Style::default().fg(theme.fg_muted),
    )));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

fn draw_settings_profile_manager(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;

    // If save-as input is active, draw that sub-overlay instead
    if app.settings_profile_save_as {
        draw_settings_profile_save_as(frame, app, area);
        return;
    }

    let max_visible = 10usize.min(app.available_settings_profiles.len());
    let pref_height = max_visible as u16 + 5; // borders + footer

    let (popup_area, block) = centered_popup(
        frame, area, 42, pref_height, " Settings Profiles ", theme,
    );

    let inner_height = popup_area.height.saturating_sub(2) as usize;
    let footer_lines = 2;
    let visible_rows = inner_height.saturating_sub(footer_lines);

    let scroll_offset = if app.settings_profile_manager_index >= visible_rows {
        app.settings_profile_manager_index - visible_rows + 1
    } else {
        0
    };

    // Determine if current settings differ from loaded profile
    let has_changes = !app.available_settings_profiles.iter()
        .any(|p| p.name == app.settings_profile_name && p.matches_app(app));

    let mut lines: Vec<Line> = Vec::new();
    let end = (scroll_offset + visible_rows).min(app.available_settings_profiles.len());
    for i in scroll_offset..end {
        let profile = &app.available_settings_profiles[i];
        let is_selected = i == app.settings_profile_manager_index;
        let is_active = profile.name == app.settings_profile_name;

        let marker = if is_active { ">" } else { " " };
        let row_style = if is_selected {
            Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };
        let marker_style = if is_selected {
            Style::default().bg(theme.bg_selected).fg(if is_active { theme.accent } else { theme.fg_muted })
        } else {
            Style::default().fg(if is_active { theme.accent } else { theme.fg_muted })
        };

        lines.push(Line::from(vec![
            Span::styled(format!("  {marker} "), marker_style),
            Span::styled(profile.name.clone(), row_style),
        ]));
    }

    while lines.len() < visible_rows {
        lines.push(Line::from(""));
    }

    // Build contextual footer hints
    let selected_profile = app.available_settings_profiles.get(app.settings_profile_manager_index);
    let is_builtin = selected_profile
        .map(|p| crate::settings_profile::is_builtin(&p.name))
        .unwrap_or(true);

    let mut hints = vec!["j/k nav", "Enter load", "Esc close"];
    if has_changes {
        if !is_builtin {
            hints.push("s save");
        }
        hints.push("S save as");
    }
    if !is_builtin {
        hints.push("d delete");
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {}", hints.join("  ")),
        Style::default().fg(theme.fg_muted),
    )));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

fn draw_settings_profile_save_as(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let (popup_area, block) = centered_popup(
        frame, area, 40, 7, " Save Profile As ", theme,
    );

    let cursor_char = if app.settings_profile_save_as_input.is_empty() { "_" } else { "" };
    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Name: ", Style::default().fg(theme.fg_secondary)),
            Span::styled(
                format!("{}{cursor_char}", app.settings_profile_save_as_input),
                Style::default().fg(theme.fg).add_modifier(Modifier::UNDERLINED),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Enter save  |  Esc cancel",
            Style::default().fg(theme.fg_muted),
        )),
    ];

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

fn draw_pin_duration_picker(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let item_count = PIN_DURATIONS.len();
    let popup_height = item_count as u16 + 4; // borders + footer

    let (popup_area, block) = centered_popup(
        frame, area, 24, popup_height, " Pin Duration ", theme,
    );

    let mut lines: Vec<Line> = Vec::new();

    for (i, (_seconds, label)) in PIN_DURATIONS.iter().enumerate() {
        let style = if i == app.pin_duration_index {
            Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };
        let marker = if i == app.pin_duration_index { ">" } else { " " };
        lines.push(Line::from(Span::styled(
            format!(" {marker} {label}"),
            style,
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " j/k  Enter  Esc",
        Style::default().fg(theme.fg_muted),
    )));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

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
        let pct = if total_votes > 0 { (count * 100) / total_votes } else { 0 };
        let filled = if total_votes > 0 { (count * bar_width) / total_votes } else { 0 };
        let empty = bar_width - filled;

        let bar: String = "\u{2588}".repeat(filled) + &"\u{2591}".repeat(empty);

        let voted_marker = if own_selections[i] { "\u{2713} " } else { "  " };
        let text_style = if own_selections[i] {
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
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
            Span::styled(
                format!("{:<12}", label),
                text_style,
            ),
            Span::styled(bar, Style::default().fg(theme.accent)),
            Span::styled(
                format!("  {count} ({pct}%)"),
                Style::default().fg(theme.fg_muted),
            ),
        ]));
    }

    let mode = if poll.allow_multiple { "multi-select" } else { "single choice" };
    let status = if poll.closed { " [CLOSED]" } else { "" };
    lines.push(Line::from(Span::styled(
        format!("    {total_votes} votes \u{00b7} {mode}{status}"),
        Style::default().fg(theme.fg_muted),
    )));

    lines
}

fn draw_poll_vote_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let pending = match &app.poll_vote_pending {
        Some(p) => p,
        None => return,
    };

    let option_count = pending.options.len();
    let max_text_len = pending.options.iter().map(|o| o.text.len()).max().unwrap_or(8);
    let popup_width = (max_text_len as u16 + 12).max(24).min(area.width.saturating_sub(4));
    let popup_height = option_count as u16 + 5;

    let (popup_area, block) = centered_popup(
        frame, area, popup_width, popup_height, " Vote ", theme,
    );

    let mut lines: Vec<Line> = Vec::new();

    for (i, opt) in pending.options.iter().enumerate() {
        let selected = app.poll_vote_selections.get(i).copied().unwrap_or(false);
        let marker = if i == app.poll_vote_index { ">" } else { " " };
        let checkbox = if selected { "[x]" } else { "[ ]" };
        let style = if i == app.poll_vote_index {
            Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };
        lines.push(Line::from(Span::styled(
            format!(" {marker} {checkbox} {}", opt.text),
            style,
        )));
    }

    lines.push(Line::from(""));
    let mode_hint = if pending.allow_multiple { "Space: toggle" } else { "Space: select" };
    lines.push(Line::from(Span::styled(
        format!(" {mode_hint}  Enter: submit  Esc"),
        Style::default().fg(theme.fg_muted),
    )));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

fn draw_about(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let version = env!("CARGO_PKG_VERSION");

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  siggy v{version}"),
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  A terminal Signal messenger client",
            Style::default().fg(theme.fg),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Created by John Sideserf",
            Style::default().fg(theme.fg_secondary),
        )),
        Line::from(Span::styled(
            "  License: GPL-3.0",
            Style::default().fg(theme.fg_secondary),
        )),
        Line::from(Span::styled(
            "  github.com/johnsideserf/siggy",
            Style::default().fg(theme.link),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Press any key to close",
            Style::default().fg(theme.fg_muted),
        )),
    ];

    let pref_height = lines.len() as u16 + 2; // +2 for borders
    let (popup_area, block) = centered_popup(
        frame, area, ABOUT_POPUP_WIDTH, pref_height, " About ", theme,
    );

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

fn draw_profile(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let labels = ["Given name", "Family name", "About", "About emoji"];

    let mut lines: Vec<Line> = Vec::new();

    for (i, label) in labels.iter().enumerate() {
        let is_selected = i == app.profile_index;
        let is_editing = is_selected && app.profile_editing;

        let label_style = if is_selected {
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg_secondary)
        };

        let value = if is_editing {
            format!("{}\u{2588}", app.profile_edit_buffer) // block cursor
        } else {
            let v = &app.profile_fields[i];
            if v.is_empty() { "(empty)".to_string() } else { v.clone() }
        };

        let value_style = if is_editing || is_selected {
            Style::default().bg(theme.bg_selected).fg(theme.fg)
        } else if app.profile_fields[i].is_empty() {
            Style::default().fg(theme.fg_muted)
        } else {
            Style::default().fg(theme.fg)
        };

        let row_style = if is_selected {
            Style::default().bg(theme.bg_selected)
        } else {
            Style::default()
        };

        lines.push(Line::from(vec![
            Span::styled(format!("  {:<14} ", label), label_style),
            Span::styled(value, value_style),
            // Pad remaining width with bg color for selected row
            Span::styled("", row_style),
        ]));
    }

    // Blank line before Save
    lines.push(Line::from(""));

    // Save button
    let save_selected = app.profile_index == 4;
    let save_style = if save_selected {
        Style::default().bg(theme.bg_selected).fg(theme.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg_secondary)
    };
    lines.push(Line::from(Span::styled("  [ Save ]", save_style)));

    // Footer
    lines.push(Line::from(""));
    let footer = if app.profile_editing {
        "  Type to edit | Enter confirm | Esc cancel"
    } else {
        "  j/k navigate | Enter edit | Esc close"
    };
    lines.push(Line::from(Span::styled(
        footer,
        Style::default().fg(theme.fg_muted),
    )));

    let pref_height = lines.len() as u16 + 2; // +2 for borders
    let (popup_area, block) = centered_popup(
        frame, area, PROFILE_POPUP_WIDTH, pref_height, " Edit Profile ", theme,
    );

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

fn draw_forward(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let max_rows = 10usize;
    let list_height = app.forward_filtered.len().min(max_rows);
    let pref_height = (list_height + 4) as u16; // filter line + blank + list + footer
    let (popup_area, block) = centered_popup(
        frame, area, 45, pref_height, " Forward to ", theme,
    );
    let inner = popup_area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });

    let mut lines: Vec<Line> = Vec::new();

    // Filter input
    let filter_display = if app.forward_filter.is_empty() {
        "type to filter...".to_string()
    } else {
        app.forward_filter.clone()
    };
    let filter_style = if app.forward_filter.is_empty() {
        Style::default().fg(theme.fg_muted)
    } else {
        Style::default().fg(theme.fg)
    };
    lines.push(Line::from(Span::styled(format!("  > {filter_display}"), filter_style)));
    lines.push(Line::from(""));

    // Conversation list
    let visible_rows = inner.height.saturating_sub(3) as usize;
    let scroll_offset = if app.forward_index >= visible_rows {
        app.forward_index - visible_rows + 1
    } else {
        0
    };
    let end = (scroll_offset + visible_rows).min(app.forward_filtered.len());

    if app.forward_filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No conversations found",
            Style::default().fg(theme.fg_muted),
        )));
    } else {
        for (i, (_id, name)) in app.forward_filtered[scroll_offset..end].iter().enumerate() {
            let actual_idx = scroll_offset + i;
            let is_selected = actual_idx == app.forward_index;
            let prefix = if is_selected { "> " } else { "  " };
            let style = if is_selected {
                Style::default().bg(theme.bg_selected).fg(theme.fg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            lines.push(Line::from(Span::styled(
                format!("{prefix}{name}"),
                style,
            )));
        }
    }

    // Footer
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Enter: forward | Esc: cancel",
        Style::default().fg(theme.fg_muted),
    )));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
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
            Reaction { emoji: "\u{1f44d}".to_string(), sender: "Alice".to_string() },
            Reaction { emoji: "\u{1f44d}".to_string(), sender: "Bob".to_string() },
        ];
        let line = build_reaction_summary(&reactions, false, &theme);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("2"), "expected count '2' in: {text}");
    }

    #[test]
    fn reaction_summary_verbose_names() {
        let theme = default_theme();
        let reactions = vec![
            Reaction { emoji: "\u{2764}".to_string(), sender: "Alice".to_string() },
        ];
        let line = build_reaction_summary(&reactions, true, &theme);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(text.contains("Alice"), "expected sender name in: {text}");
    }

    #[test]
    fn reaction_summary_empty() {
        let theme = default_theme();
        let line = build_reaction_summary(&[], false, &theme);
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
                PollOption { id: 0, text: "A".to_string() },
                PollOption { id: 1, text: "B".to_string() },
            ],
            allow_multiple: false,
            closed: false,
        };
        let votes = vec![
            PollVote { voter: "+1".to_string(), voter_name: None, option_indexes: vec![0], vote_count: 1 },
            PollVote { voter: "+2".to_string(), voter_name: None, option_indexes: vec![0], vote_count: 1 },
        ];
        let lines = build_poll_display(&poll, &votes, "+99", &theme);
        assert_eq!(lines.len(), 3);
        let summary: String = lines.last().unwrap().spans.iter().map(|s| s.content.to_string()).collect();
        assert!(summary.contains("votes"), "expected 'votes' in: {summary}");
    }

    #[test]
    fn poll_display_own_vote_marked() {
        let theme = default_theme();
        let poll = PollData {
            question: "Q?".to_string(),
            options: vec![PollOption { id: 0, text: "Yes".to_string() }],
            allow_multiple: false,
            closed: false,
        };
        let votes = vec![
            PollVote { voter: "+me".to_string(), voter_name: None, option_indexes: vec![0], vote_count: 1 },
        ];
        let lines = build_poll_display(&poll, &votes, "+me", &theme);
        let option_text: String = lines[0].spans.iter().map(|s| s.content.to_string()).collect();
        assert!(option_text.contains("\u{2713}"), "expected checkmark in: {option_text}");
    }

    #[test]
    fn poll_display_closed() {
        let theme = default_theme();
        let poll = PollData {
            question: "Q?".to_string(),
            options: vec![PollOption { id: 0, text: "X".to_string() }],
            allow_multiple: false,
            closed: true,
        };
        let lines = build_poll_display(&poll, &[], "+me", &theme);
        let summary: String = lines.last().unwrap().spans.iter().map(|s| s.content.to_string()).collect();
        assert!(summary.contains("[CLOSED]"), "expected [CLOSED] in: {summary}");
    }

    #[test]
    fn poll_display_no_votes() {
        let theme = default_theme();
        let poll = PollData {
            question: "Q?".to_string(),
            options: vec![PollOption { id: 0, text: "A".to_string() }],
            allow_multiple: false,
            closed: false,
        };
        let lines = build_poll_display(&poll, &[], "+me", &theme);
        let option_text: String = lines[0].spans.iter().map(|s| s.content.to_string()).collect();
        assert!(option_text.contains("0 (0%)"), "expected '0 (0%)' in: {option_text}");
        let summary: String = lines.last().unwrap().spans.iter().map(|s| s.content.to_string()).collect();
        assert!(summary.contains("0 votes"), "expected '0 votes' in: {summary}");
    }

    // --- format_file_size ---

    #[rstest]
    #[case(0, "0B")]
    #[case(512, "512B")]
    #[case(1024, "1K")]
    #[case(2048, "2K")]
    #[case(1_048_576, "1.0M")]
    #[case(1_073_741_824, "1.0G")]
    fn format_file_size_cases(#[case] bytes: u64, #[case] expected: &str) {
        assert_eq!(format_file_size(bytes), expected);
    }

    // --- search_snippet ---

    #[test]
    fn search_snippet_short_passthrough() {
        let body = "short text";
        assert_eq!(search_snippet(body, "short", 100), body);
    }

    #[test]
    fn search_snippet_centers_on_match() {
        let body = "a".repeat(100) + "NEEDLE" + &"b".repeat(100);
        let snippet = search_snippet(&body, "NEEDLE", 30);
        assert!(snippet.chars().count() <= 30, "snippet too long ({} chars): {snippet}", snippet.chars().count());
        assert!(snippet.contains("NEEDLE"), "expected query in snippet: {snippet}");
    }
}
