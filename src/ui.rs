use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
    Frame,
};

use crate::app::{App, InputMode, VisibleImage, QUICK_REACTIONS, SETTINGS};
use crate::signal::types::{MessageStatus, Reaction};
use crate::image_render::ImageProtocol;
use crate::input::COMMANDS;

// Layout constants
const SIDEBAR_AUTO_HIDE_WIDTH: u16 = 60;
const MIN_CHAT_WIDTH: u16 = 30;
const MSG_WINDOW_MULTIPLIER: usize = 3;

// Popup dimensions
const SETTINGS_POPUP_WIDTH: u16 = 42;
const SETTINGS_POPUP_HEIGHT: u16 = 15;
const CONTACTS_POPUP_WIDTH: u16 = 50;
const CONTACTS_MAX_VISIBLE: usize = 20;

/// Map a MessageStatus to its display symbol and color.
fn status_symbol(status: MessageStatus, nerd_fonts: bool, color: bool) -> (&'static str, Color) {
    let (unicode_sym, nerd_sym, colored) = match status {
        MessageStatus::Failed   => ("\u{2717}", "\u{f055c}", Color::Red),       // ✗ / 󰅜
        MessageStatus::Sending  => ("\u{25cc}", "\u{f0996}", Color::DarkGray),  // ◌ / 󰦖
        MessageStatus::Sent     => ("\u{25cb}", "\u{f0954}", Color::DarkGray),  // ○ / 󰥔
        MessageStatus::Delivered=> ("\u{2713}", "\u{f012c}", Color::White),     // ✓ / 󰄬
        MessageStatus::Read     => ("\u{25cf}", "\u{f012d}", Color::Green),     // ● / 󰄭
        MessageStatus::Viewed   => ("\u{25c9}", "\u{f0208}", Color::Cyan),     // ◉ / 󰈈
    };
    let sym = if nerd_fonts { nerd_sym } else { unicode_sym };
    let fg = if color { colored } else { Color::DarkGray };
    (sym, fg)
}

/// Hash a sender name to one of ~8 distinct colors. "you" always gets Green.
fn sender_color(name: &str) -> Color {
    if name == "you" {
        return Color::Green;
    }
    let hash: u32 = name.bytes().fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    const COLORS: [Color; 8] = [
        Color::Cyan,
        Color::Magenta,
        Color::Yellow,
        Color::Blue,
        Color::LightRed,
        Color::LightGreen,
        Color::LightCyan,
        Color::LightMagenta,
    ];
    COLORS[(hash as usize) % COLORS.len()]
}

/// Truncate a string to fit within `max_width`, appending `…` if truncated.
fn truncate(s: &str, max_width: usize) -> String {
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
fn build_separator(label: &str, width: usize, style: Style) -> Line<'static> {
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
        .border_style(Style::default().fg(Color::Cyan))
        .title(title.to_string())
        .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(Color::Black));
    (popup_area, block)
}

/// A clickable link region detected in the rendered buffer.
pub struct LinkRegion {
    pub x: u16,
    pub y: u16,
    pub url: String,
    pub text: String,
}

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

/// Check if a cell's style matches the link style (Blue fg + UNDERLINED).
fn is_link_style(style: &Style) -> bool {
    style.fg == Some(Color::Blue) && style.add_modifier.contains(Modifier::UNDERLINED)
}

/// Scan a rendered buffer area for consecutive cells with the link style,
/// and collect them into LinkRegion structs.
fn collect_link_regions(buf: &Buffer, area: Rect) -> Vec<LinkRegion> {
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

            if !is_link_style(&cell.style()) {
                x += 1;
                continue;
            }

            // Start of a link run
            let start_x = x;
            let mut text = String::new();

            while x < right_edge {
                match buf.cell(Position::new(x, y)) {
                    Some(c) if is_link_style(&c.style()) => {
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

            regions.push(LinkRegion {
                x: start_x,
                y,
                url,
                text,
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
fn styled_uri_spans(body: &str) -> (Vec<Span<'static>>, Option<String>) {
    let link_style = Style::default()
        .fg(Color::Blue)
        .add_modifier(Modifier::UNDERLINED);

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

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut rest = body;

    while !rest.is_empty() {
        // Find the earliest URI scheme
        let next_uri = ["https://", "http://", "file:///"]
            .iter()
            .filter_map(|scheme| rest.find(scheme).map(|pos| (pos, scheme)))
            .min_by_key(|(pos, _)| *pos);

        match next_uri {
            Some((pos, _scheme)) => {
                // Push text before the URI
                if pos > 0 {
                    spans.push(Span::raw(rest[..pos].to_string()));
                }
                // Find the end of the URI (first whitespace or end of string)
                let uri_start = &rest[pos..];
                let uri_end = uri_start
                    .find(|c: char| c.is_whitespace())
                    .unwrap_or(uri_start.len());
                spans.push(Span::styled(uri_start[..uri_end].to_string(), link_style));
                rest = &uri_start[uri_end..];
            }
            None => {
                spans.push(Span::raw(rest.to_string()));
                break;
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
        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(app.sidebar_width),
                Constraint::Min(MIN_CHAT_WIDTH),
            ])
            .split(body_area);

        draw_sidebar(frame, app, horizontal[0]);
        draw_chat_area(frame, app, horizontal[1])
    } else {
        draw_chat_area(frame, app, body_area)
    };

    draw_status_bar(frame, app, status_area, sidebar_auto_hidden);

    // Autocomplete popup (overlays everything)
    if app.autocomplete_visible && !app.autocomplete_candidates.is_empty() {
        draw_autocomplete(frame, app, input_area);
    }

    // Settings overlay (overlays everything)
    if app.show_settings {
        draw_settings(frame, app, size);
    }

    // Help overlay (overlays everything)
    if app.show_help {
        draw_help(frame, size);
    }

    // Contacts overlay (overlays everything)
    if app.show_contacts {
        draw_contacts(frame, app, size);
    }

    // Reaction picker overlay
    if app.show_reaction_picker {
        draw_reaction_picker(frame, app, size);
    }

    // Collect link regions from the rendered buffer for OSC 8 injection
    let area = frame.area();
    app.link_regions = collect_link_regions(frame.buffer_mut(), area);

    // Resolve hidden URLs for attachment links (display text has no URI scheme)
    for link in &mut app.link_regions {
        if !link.url.contains("://") {
            if let Some(url) = app.link_url_map.get(&link.text) {
                link.url = url.clone();
            }
        }
    }
}

fn draw_sidebar(frame: &mut Frame, app: &App, area: Rect) {
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
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::raw("  "));
            }

            // Unread dot
            if has_unread && !is_active {
                spans.push(Span::styled("• ", Style::default().fg(Color::Yellow)));
            } else {
                spans.push(Span::raw("  "));
            }

            // Group prefix (dimmed #)
            if conv.is_group {
                spans.push(Span::styled(
                    "#",
                    Style::default().fg(Color::DarkGray),
                ));
            }

            // Conversation name
            let is_muted = app.muted_conversations.contains(id);
            let name_style = if is_active {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if has_unread {
                Style::default().fg(Color::Yellow)
            } else if is_muted {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::Gray)
            };
            spans.push(Span::styled(name, name_style));

            if is_muted {
                spans.push(Span::styled(" ~", Style::default().fg(Color::DarkGray)));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    let sidebar = List::new(items).block(
        Block::default()
            .borders(Borders::RIGHT)
            .border_type(BorderType::Rounded)
            .title(" Chats ")
            .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
    );

    frame.render_widget(sidebar, area);
}

fn draw_chat_area(frame: &mut Frame, app: &mut App, area: Rect) -> Rect {
    let chat_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),   // messages (typing indicator rendered inside)
            Constraint::Length(3), // input
        ])
        .split(area);

    let messages_area = chat_layout[0];
    let input_area = chat_layout[1];

    draw_messages(frame, app, messages_area);
    draw_input(frame, app, input_area);
    input_area
}

fn draw_messages(frame: &mut Frame, app: &mut App, area: Rect) {
    let (title_left, title_right) = match &app.active_conversation {
        Some(id) => {
            let conv = &app.conversations[id];
            let prefix = if conv.is_group { " #" } else { " " };
            let left = format!("{prefix}{} ", conv.name);

            // Scroll indicator in title
            let right = if app.scroll_offset > 0 {
                format!(" ↑ {} more ", app.scroll_offset)
            } else {
                String::new()
            };
            (left, right)
        }
        None => (" signal-tui ".to_string(), String::new()),
    };

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(title_left)
        .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

    if !title_right.is_empty() {
        block = block
            .title_bottom(Line::from(title_right).alignment(Alignment::Right))
            .title_style(Style::default().fg(Color::Cyan));
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let messages = match &app.active_conversation {
        Some(id) => {
            if let Some(conv) = app.conversations.get(id) {
                &conv.messages
            } else {
                app.focused_message_time = None;
                return;
            }
        }
        None => {
            draw_welcome(frame, app, inner);
            app.focused_message_time = None;
            return;
        }
    };

    let available_height = inner.height as usize;
    let total = messages.len();

    // Build lines from a generous window covering the viewport at the current scroll position.
    // Always include messages up to `total`; scroll_offset controls the paragraph scroll instead.
    let start = total.saturating_sub(available_height * MSG_WINDOW_MULTIPLIER + app.scroll_offset);
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
        let local = msg.timestamp.with_timezone(&chrono::Local);
        let date_str = local.format("%b %d, %Y").to_string();
        if prev_date.as_ref() != Some(&date_str) {
            if prev_date.is_some() {
                let label = format!(" {} ", date_str);
                lines.push(build_separator(&label, inner_width, Style::default().fg(Color::DarkGray)));
                line_msg_idx.push(None);
            }
            prev_date = Some(date_str);
        }

        // Unread marker: between last_read - 1 and last_read
        if msg_index == last_read && last_read > 0 && last_read < total {
            lines.push(build_separator(
                " new messages ",
                inner_width,
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
            line_msg_idx.push(None);
        }

        if msg.is_system {
            lines.push(Line::from(Span::styled(
                format!("  {}", msg.body),
                Style::default().fg(Color::DarkGray),
            )));
            line_msg_idx.push(Some(msg_index));
        } else {
            let time = msg.format_time();
            let mut spans = Vec::new();

            // Status symbol for outgoing messages (before timestamp)
            if app.show_receipts {
                if let Some(status) = msg.status {
                    let (sym, color) = status_symbol(status, app.nerd_fonts, app.color_receipts);
                    spans.push(Span::styled(
                        format!("{sym} "),
                        Style::default().fg(color),
                    ));
                }
            }

            spans.push(Span::styled(
                format!("[{}] ", time),
                Style::default().fg(Color::DarkGray),
            ));
            spans.push(Span::styled(
                format!("<{}>", msg.sender),
                Style::default()
                    .fg(sender_color(&msg.sender))
                    .add_modifier(Modifier::BOLD),
            ));

            // Style URIs (https://, http://, file:///) as underlined links
            let (body_spans, hidden_url) = styled_uri_spans(&msg.body);
            if let Some(url) = hidden_url {
                // Collect display text for link_url_map lookup
                let display_text: String = body_spans.iter().map(|s| s.content.as_ref()).collect();
                app.link_url_map.insert(display_text, url);
            }
            spans.push(Span::raw(" ".to_string()));
            spans.extend(body_spans);

            lines.push(Line::from(spans));
            line_msg_idx.push(Some(msg_index));

            // Render inline image preview if available
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

            // Render reaction summary line
            if !msg.reactions.is_empty() {
                lines.push(build_reaction_summary(&msg.reactions, app.reaction_verbose));
                line_msg_idx.push(Some(msg_index));
            }
        }
    }

    // Append typing indicator as the last line inside the message area
    if let Some(ref conv_id) = app.active_conversation {
        let typers: Vec<String> = app
            .typing_indicators
            .keys()
            .filter(|sender| {
                *sender == conv_id
                    || app
                        .conversations
                        .get(conv_id)
                        .is_some_and(|c| c.is_group)
            })
            .map(|s| {
                if let Some(name) = app.contact_names.get(s) {
                    name.clone()
                } else if let Some(conv) = app.conversations.get(s) {
                    conv.name.clone()
                } else {
                    s.clone()
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
                    .fg(Color::DarkGray)
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
    let scroll_y = base_scroll - app.scroll_offset;

    // Determine the focused message for full-timestamp display in Normal mode.
    app.focused_message_time = if app.mode == InputMode::Normal && app.scroll_offset > 0 {
        find_focused_message_time(&lines, &line_msg_idx, messages, inner_width, scroll_y, available_height)
    } else {
        None
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

                app.visible_images.push(VisibleImage {
                    x: inner.x + 2, // account for 2-char indent
                    y: inner.y + vis_start,
                    width: img_width,
                    height: vis_end - vis_start,
                    path: path.clone(),
                });
            }
        }
    }

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll_y as u16, 0));
    frame.render_widget(paragraph, inner);

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

/// Build a reaction summary line like "    👍 2  ❤️ 1  😂 1"
fn build_reaction_summary(reactions: &[Reaction], verbose: bool) -> Line<'static> {
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
                Style::default().fg(Color::DarkGray),
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
                Style::default().fg(Color::DarkGray),
            ));
        }
        Line::from(spans)
    }
}

fn draw_reaction_picker(frame: &mut Frame, app: &App, area: Rect) {
    let emoji_count = QUICK_REACTIONS.len();
    let popup_width = (emoji_count * 4 + 4) as u16;
    let popup_height = 3u16;

    let (popup_area, block) = centered_popup(
        frame, area, popup_width, popup_height, " React ",
    );

    let mut spans = vec![Span::raw(" ".to_string())];
    for (i, emoji) in QUICK_REACTIONS.iter().enumerate() {
        let style = if i == app.reaction_picker_index {
            Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD)
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

/// Render the welcome/empty-state screen when no conversation is active.
fn draw_welcome(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![Line::from("")];

    if let Some(ref err) = app.connection_error {
        lines.push(Line::from(Span::styled(
            "  Connection Error",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            format!("  {err}"),
            Style::default().fg(Color::Red),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Run with --setup to reconfigure.",
            Style::default().fg(Color::Gray),
        )));
    } else if app.conversation_order.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Welcome to signal-tui",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  No conversations yet",
            Style::default().fg(Color::Gray),
        )));
        lines.push(Line::from(Span::styled(
            "  Messages you send and receive will appear here.",
            Style::default().fg(Color::Gray),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Use /join +1234567890 to message someone",
            Style::default().fg(Color::Gray),
        )));
        lines.push(Line::from(Span::styled(
            "  Use /help for all commands",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  Welcome to signal-tui",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Use /join <contact> to start a conversation",
            Style::default().fg(Color::Gray),
        )));
        lines.push(Line::from(Span::styled(
            "  Use /help for all commands",
            Style::default().fg(Color::Gray),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Ctrl+←/→ to resize sidebar",
            Style::default().fg(Color::DarkGray),
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Find the timestamp of the message at the bottom of the visible viewport.
/// Used for the full-timestamp status bar display when scrolled in Normal mode.
fn find_focused_message_time(
    lines: &[Line], line_msg_idx: &[Option<usize>],
    messages: &[crate::app::DisplayMessage],
    inner_width: usize, scroll_y: usize, available_height: usize,
) -> Option<chrono::DateTime<chrono::Utc>> {
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
            return Some(messages[*mi].timestamp);
        }
        if li == 0 {
            return None;
        }
        li -= 1;
    }
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let border_color = match app.mode {
        InputMode::Insert => Color::Cyan,
        InputMode::Normal => Color::Yellow,
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color));

    // Available width inside the border (minus border cells on each side)
    let inner_width = area.width.saturating_sub(2) as usize;
    let prefix = "> ";
    let prefix_len = prefix.len(); // 2
    let text_width = inner_width.saturating_sub(prefix_len); // usable chars for buffer text

    if app.input_buffer.is_empty() {
        let placeholder = match app.mode {
            InputMode::Normal => "  Press i to type, / for commands",
            InputMode::Insert => "  Type a message...",
        };
        let input = Paragraph::new(Span::styled(
            placeholder,
            Style::default().fg(Color::DarkGray),
        ))
        .block(block);
        frame.render_widget(input, area);
    } else {
        // Scroll the visible window so the cursor is always on screen
        let scroll_offset = app.input_cursor.saturating_sub(text_width);
        let visible_end = (scroll_offset + text_width).min(app.input_buffer.len());
        let visible = &app.input_buffer[scroll_offset..visible_end];
        let input_text = format!("{prefix}{visible}");
        let input = Paragraph::new(input_text)
            .style(Style::default().fg(Color::White))
            .block(block);
        frame.render_widget(input, area);
    }

    // Place cursor (only visible in Insert mode)
    if app.mode == InputMode::Insert {
        let scroll_offset = app.input_cursor.saturating_sub(text_width);
        let cursor_x = area.x + 1 + prefix_len as u16 + (app.input_cursor - scroll_offset) as u16;
        let cursor_y = area.y + 1;
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect, sidebar_auto_hidden: bool) {
    let mut segments: Vec<Span> = Vec::new();

    // Mode indicator
    match app.mode {
        InputMode::Normal => {
            segments.push(Span::styled(
                " [NORMAL] ",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ));
        }
        InputMode::Insert => {
            segments.push(Span::styled(
                " [INSERT] ",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ));
        }
    }
    segments.push(Span::styled("│ ", Style::default().fg(Color::DarkGray)));

    // Connection status dot
    if let Some(ref err) = app.connection_error {
        segments.push(Span::styled(" ● ", Style::default().fg(Color::Red)));
        let display: String = err.chars().take(30).collect();
        segments.push(Span::styled(
            format!("error: {display}"),
            Style::default().fg(Color::Red),
        ));
    } else if app.connected {
        segments.push(Span::styled(" ● ", Style::default().fg(Color::Green)));
        segments.push(Span::styled("connected", Style::default().fg(Color::White)));
        if app.incognito {
            segments.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
            segments.push(Span::styled(
                "incognito",
                Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
            ));
        }
    } else {
        segments.push(Span::styled(" ● ", Style::default().fg(Color::Red)));
        segments.push(Span::styled("disconnected", Style::default().fg(Color::White)));
    }

    // Pipe separator
    segments.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));

    // Current conversation
    if let Some(ref id) = app.active_conversation {
        if let Some(conv) = app.conversations.get(id) {
            let prefix = if conv.is_group { "#" } else { "" };
            segments.push(Span::styled(
                format!("{}{}", prefix, conv.name),
                Style::default().fg(Color::Cyan),
            ));
        }
    } else {
        segments.push(Span::styled(
            "no conversation",
            Style::default().fg(Color::DarkGray),
        ));
    }

    // Pipe separator + conversation count
    if !app.conversation_order.is_empty() {
        segments.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
        segments.push(Span::styled(
            format!("{} chats", app.conversation_order.len()),
            Style::default().fg(Color::Gray),
        ));
    }

    // Scroll offset indicator + focused message timestamp
    if app.scroll_offset > 0 {
        segments.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
        segments.push(Span::styled(
            format!("↑{}", app.scroll_offset),
            Style::default().fg(Color::Yellow),
        ));
        if let Some(ref ts) = app.focused_message_time {
            let local = ts.with_timezone(&chrono::Local);
            segments.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
            segments.push(Span::styled(
                local.format("%a %b %d, %Y %I:%M:%S %p").to_string(),
                Style::default().fg(Color::White),
            ));
        }
    }

    // Auto-hidden sidebar indicator
    if sidebar_auto_hidden && app.sidebar_visible {
        segments.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
        segments.push(Span::styled(
            "[+]",
            Style::default().fg(Color::DarkGray),
        ));
    }

    // Pad the rest with background
    let status = Paragraph::new(Line::from(segments)).style(
        Style::default()
            .fg(Color::White)
            .bg(Color::DarkGray),
    );
    frame.render_widget(status, area);
}

fn draw_autocomplete(frame: &mut Frame, app: &App, input_area: Rect) {
    let candidates = &app.autocomplete_candidates;
    let count = candidates.len();
    let terminal_width = frame.area().width;

    // Build lines and measure max width
    let mut lines: Vec<Line> = Vec::with_capacity(count);
    let mut max_content_width: usize = 0;
    for (i, &cmd_idx) in candidates.iter().enumerate() {
        let cmd = &COMMANDS[cmd_idx];
        let args_part = if cmd.args.is_empty() {
            String::new()
        } else {
            format!(" {}", cmd.args)
        };
        let left = format!("  {}{}", cmd.name, args_part);
        let right = format!("  {}", cmd.description);
        let total_len = left.len() + right.len() + 2; // padding
        if total_len > max_content_width {
            max_content_width = total_len;
        }

        let is_selected = i == app.autocomplete_index;
        let style = if is_selected {
            Style::default().bg(Color::DarkGray).fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let desc_style = if is_selected {
            Style::default().bg(Color::DarkGray).fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        lines.push(Line::from(vec![
            Span::styled(left, style),
            Span::styled(right, desc_style),
        ]));
    }

    // Size the popup
    let popup_width = (max_content_width as u16 + 2).min(terminal_width.saturating_sub(2)).max(20);
    let popup_height = (count as u16) + 2; // +2 for border

    // Position above the input box, left-aligned with it
    let x = input_area.x;
    let y = input_area.y.saturating_sub(popup_height);

    let area = Rect::new(x, y, popup_width, popup_height);

    // Clear the area behind the popup so chat text doesn't leak through
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .style(Style::default().bg(Color::Black));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, area);
}

fn draw_settings(frame: &mut Frame, app: &App, area: Rect) {
    let (popup_area, block) = centered_popup(
        frame, area, SETTINGS_POPUP_WIDTH, SETTINGS_POPUP_HEIGHT, " Settings ",
    );

    let mut lines: Vec<Line> = Vec::new();
    for (i, def) in SETTINGS.iter().enumerate() {
        let enabled = app.setting_value(i);
        let checkbox = if enabled { "[x]" } else { "[ ]" };
        let is_selected = i == app.settings_index;
        let style = if is_selected {
            Style::default().bg(Color::DarkGray).fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let check_style = if is_selected {
            Style::default().bg(Color::DarkGray).fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else if enabled {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        lines.push(Line::from(vec![
            Span::styled(format!("  {} ", checkbox), check_style),
            Span::styled(def.label.to_string(), style),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Esc to close  |  Space to toggle",
        Style::default().fg(Color::DarkGray),
    )));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    // Help table entries: (key, description)
    let commands: &[(&str, &str)] = &[
        ("/join <name>", "Switch to a conversation"),
        ("/part", "Leave current conversation"),
        ("/sidebar", "Toggle sidebar visibility"),
        ("/bell [type]", "Toggle notifications"),
        ("/mute", "Mute/unmute conversation"),
        ("/contacts", "Browse contacts"),
        ("/settings", "Open settings"),
        ("/quit", "Exit signal-tui"),
    ];
    let shortcuts: &[(&str, &str)] = &[
        ("Tab / Shift+Tab", "Next / prev conversation"),
        ("Up / Down", "Recall input history"),
        ("PgUp / PgDn", "Scroll messages"),
        ("Ctrl+Left/Right", "Resize sidebar"),
        ("Ctrl+C", "Quit"),
    ];
    let cli: &[(&str, &str)] = &[
        ("--incognito", "No local message storage"),
        ("--demo", "Launch with dummy data"),
        ("--setup", "Re-run first-time wizard"),
    ];
    let vim: &[(&str, &str)] = &[
        ("Esc", "Normal mode"),
        ("i / a / I / A / o", "Insert mode"),
        ("j / k", "Scroll up / down"),
        ("g / G", "Top / bottom of messages"),
        ("Ctrl+D / U", "Half-page scroll"),
        ("h / l", "Cursor left / right"),
        ("w / b", "Word forward / back"),
        ("0 / $", "Start / end of line"),
        ("x / D", "Delete char / to end"),
        ("y / Y", "Copy message / full line"),
        ("r", "React to focused message"),
        ("/", "Start command input"),
    ];

    // Calculate popup size
    let key_col_width = 20;
    let desc_col_width = 28;
    let pref_width = (key_col_width + desc_col_width + 6) as u16;
    let content_lines =
        commands.len() + shortcuts.len() + vim.len() + cli.len() + 7; // headers + footer + spacing
    let pref_height = content_lines as u16 + 2;

    let (popup_area, block) = centered_popup(frame, area, pref_width, pref_height, " Help ");

    let header_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let key_style = Style::default().fg(Color::Cyan);
    let desc_style = Style::default().fg(Color::Gray);

    let mut lines: Vec<Line> = Vec::new();

    // Helper to push a row
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
    for &(key, desc) in shortcuts {
        push_row(&mut lines, key, desc);
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Vim Keybindings", header_style)));
    for &(key, desc) in vim {
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
        Style::default().fg(Color::DarkGray),
    )));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

fn draw_contacts(frame: &mut Frame, app: &App, area: Rect) {
    let max_visible = CONTACTS_MAX_VISIBLE.min(app.contacts_filtered.len());
    let pref_height = max_visible as u16 + 5; // +3 border/title +2 footer/filter

    let title = if app.contacts_filter.is_empty() {
        " Contacts ".to_string()
    } else {
        format!(" Contacts [{}] ", app.contacts_filter)
    };

    let (popup_area, block) = centered_popup(
        frame, area, CONTACTS_POPUP_WIDTH, pref_height, &title,
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
            Style::default().fg(Color::DarkGray),
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
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };

            // Truncate name to fit with number and marker
            let number_display = format!("  {}", number);
            let name_max = inner_w.saturating_sub(number_display.len() + marker.len() + 2);
            let display_name = truncate(name, name_max);

            let name_style = if is_selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if has_conversation {
                Style::default().fg(Color::Gray)
            } else {
                Style::default().fg(Color::White)
            };
            let number_style = if is_selected {
                Style::default().bg(Color::DarkGray).fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let marker_bg = if is_selected {
                marker_style.bg(Color::DarkGray)
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
        Style::default().fg(Color::DarkGray),
    )));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}
