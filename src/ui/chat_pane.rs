//! Chat pane: messages list, layout dispatcher, and Kitty image patch pass.
//!
//! `draw_chat_area` is the layout dispatcher that splits the chat area
//! into messages + composer; `draw_messages` is the per-conversation
//! message list with its long pipeline of body wrapping, scroll
//! window, sender/timestamp/status decoration, quotes, mentions,
//! polls, attachments, image previews, reactions, link styling, and
//! the inline typing indicator. `patch_kitty_placeholders` runs after
//! the buffer is filled to swap halfblock cells for Kitty Unicode
//! Placeholder symbols so terminals with the Kitty graphics protocol
//! render image data inline. `emoji_to_text` rewrites emoji as text
//! emoticons or `:shortcode:` form when the user enables that setting;
//! `build_reaction_summary` and `build_poll_display` produce the
//! per-message reaction badge and poll bars consumed inside
//! `draw_messages`.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
        Wrap,
    },
};

use super::composer::draw_input;
use super::links::{split_spans_by_newline, styled_uri_spans};
use super::welcome::draw_welcome;
use super::{MSG_WINDOW_MULTIPLIER, build_separator, sender_color, status_symbol, truncate};
use crate::app::{App, InputMode, VisibleImage};
use crate::image_render::{self, ImageProtocol};
use crate::input::format_compact_duration;
use crate::signal::types::{PollData, PollVote, Reaction, TrustLevel};
use crate::theme::Theme;
use ratatui::layout::Alignment;

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

pub(super) fn draw_chat_area(frame: &mut Frame, app: &mut App, area: Rect) -> Rect {
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

    // Sync viewport pin (#394): when a pin anchor was captured at sync start,
    // derive scroll.offset from the pin message's current line position so it
    // stays at its original screen offset regardless of how many lines the
    // incoming sync messages add. Without this, scroll.offset += 1 per arriving
    // message under-compensates against multi-line messages and the viewport
    // drifts downward.
    if app.sync.active
        && !app.sync.user_scrolled
        && let Some((pin_ts, pin_offset)) = app.sync.pin
        && let Some(pin_idx) = visible.iter().position(|m| m.timestamp == pin_ts)
        && let Some(new_offset) = compute_sync_pin_offset(
            &line_heights,
            &line_msg_idx,
            content_height,
            pin_idx,
            pin_offset,
        )
    {
        app.scroll.offset = new_offset;
    }

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
pub(super) fn patch_kitty_placeholders(frame: &mut Frame, app: &mut App) {
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
fn build_reaction_summary(
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

/// Build the per-poll display lines (option bars, vote totals, mode footer).
fn build_poll_display(
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

/// Compute the `scroll.offset` that keeps the pinned message at its original
/// screen position, given the current rendered content. See #394.
///
/// Math: `pin_last_line` is the index of the pin message's last rendered line
/// in the current content. `lines_below_pin` is how many rendered lines now
/// sit after that line. The new offset = original offset at pin time + lines
/// added below pin since then. Adding the number of lines that grew below pin
/// preserves pin's distance from the bottom of the rendered content, and thus
/// its distance from the bottom of the viewport.
///
/// Returns `None` when `pin_idx` does not appear in `line_msg_idx` (e.g., the
/// pin message has been evicted from the renderer's sliding window).
fn compute_sync_pin_offset(
    line_heights: &[usize],
    line_msg_idx: &[Option<usize>],
    content_height: usize,
    pin_idx: usize,
    pin_offset_at_capture: usize,
) -> Option<usize> {
    let mut last_line: Option<usize> = None;
    let mut cumul = 0usize;
    for (idx, &h) in line_heights.iter().enumerate() {
        if line_msg_idx.get(idx) == Some(&Some(pin_idx)) {
            last_line = Some(cumul + h - 1);
        }
        cumul += h;
    }
    let pin_last_line = last_line?;
    let lines_below_pin = content_height
        .saturating_sub(1)
        .saturating_sub(pin_last_line);
    Some(pin_offset_at_capture.saturating_add(lines_below_pin))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::types::{PollData, PollOption, PollVote, Reaction};
    use crate::theme::default_theme;

    // --- compute_sync_pin_offset ---

    #[test]
    fn pin_offset_single_row_message_addition() {
        // Pre-sync: 5 messages, each rendered as one Line of height 1.
        // Pin = msg 4 (the last pre-sync message). One sync message arrives
        // also as 1 Line, height 1. Old `+= 1` per message would set offset=1;
        // the render-time math agrees here -- this is the boundary case where
        // the old code happened to be correct.
        let line_heights = vec![1, 1, 1, 1, 1, 1];
        let line_msg_idx: Vec<Option<usize>> = (0..6).map(Some).collect();
        let content_height = 6;
        let new_offset =
            compute_sync_pin_offset(&line_heights, &line_msg_idx, content_height, 4, 0).unwrap();
        assert_eq!(new_offset, 1);
    }

    #[test]
    fn pin_offset_multi_line_message_addition() {
        // Pre-sync: 5 messages, each as 2 Lines (sender/timestamp + body),
        // each Line of height 1 -> 10 rows total. Pin = msg 4.
        // One sync message arrives as 2 Lines, with body wrapping to 2 rows
        // (height = 2) -> 3 new rows. The old `+= 1` would put scroll.offset
        // at 1, leaving pin 2 rows above the original viewport bottom -- the
        // exact bug from #394. The new math sets offset = 3 so pin lands
        // exactly at the bottom.
        let mut line_heights = vec![1; 10]; // 5 messages * 2 lines * 1 row
        line_heights.extend([1, 2]); // sync: timestamp Line (1 row) + body Line (wrapped 2 rows)
        let mut line_msg_idx: Vec<Option<usize>> = Vec::new();
        for i in 0..5 {
            line_msg_idx.push(Some(i));
            line_msg_idx.push(Some(i));
        }
        line_msg_idx.push(Some(5));
        line_msg_idx.push(Some(5));
        let content_height = 13;
        let new_offset =
            compute_sync_pin_offset(&line_heights, &line_msg_idx, content_height, 4, 0).unwrap();
        assert_eq!(
            new_offset, 3,
            "offset must grow by the row count of new content (3), not by 1 per message"
        );
    }

    #[test]
    fn pin_offset_preserves_user_scroll_distance() {
        // User was scrolled up by 2 rows when sync started (pin_offset=2).
        // After sync adds 2 rows of new content, new offset should be 4 --
        // preserving the user's 2-row distance from pin while compensating
        // for the 2 new rows below pin.
        let mut line_heights = vec![1, 1, 1]; // 3 pre-sync single-line messages
        line_heights.extend([1, 1]); // sync adds 2 rows (one 2-line message)
        let mut line_msg_idx: Vec<Option<usize>> = (0..3).map(Some).collect();
        line_msg_idx.push(Some(3));
        line_msg_idx.push(Some(3));
        let content_height = 5;
        let new_offset =
            compute_sync_pin_offset(&line_heights, &line_msg_idx, content_height, 2, 2).unwrap();
        assert_eq!(new_offset, 4);
    }

    #[test]
    fn pin_offset_returns_none_when_pin_message_evicted() {
        // Pin message isn't present in line_msg_idx (e.g., evicted from the
        // renderer's sliding window). Caller should fall back to default
        // behavior, which is what `None` signals.
        let line_heights = vec![1, 1, 1];
        let line_msg_idx = vec![Some(10), Some(11), Some(12)];
        let pin_idx = 5; // not present
        let result = compute_sync_pin_offset(&line_heights, &line_msg_idx, 3, pin_idx, 0);
        assert!(result.is_none());
    }

    #[test]
    fn pin_offset_skips_separator_lines() {
        // Some lines are date separators / unread markers (None in line_msg_idx).
        // The walk must ignore those when locating pin's last line.
        let line_heights = vec![1, 1, 1, 1, 1];
        let line_msg_idx = vec![Some(0), None, Some(1), Some(1), Some(2)];
        // Pin = msg 1, which spans lines 2..=3. Last line of pin is index 3.
        // content_height=5, lines_below_pin = 5 - 1 - 3 = 1 (the line for msg 2).
        let new_offset = compute_sync_pin_offset(&line_heights, &line_msg_idx, 5, 1, 0).unwrap();
        assert_eq!(new_offset, 1);
    }

    // --- end compute_sync_pin_offset ---

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
