//! Composer (input box) rendering.
//!
//! Draws the bordered text input pane at the bottom of the chat area:
//! mode-coloured border, optional `replying:` / `editing…` title,
//! optional attachment badge, the buffer text with horizontal +
//! vertical scrolling tied to the cursor position, and the placeholder
//! shown when both buffer and badge are empty. Sets the terminal
//! cursor position only in Insert mode and writes
//! `app.mouse.input_prefix_len` so click-to-position routing knows
//! where the prefix ends and the editable text begins.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph},
};

use super::truncate;
use crate::app::{App, InputMode};

pub(super) fn draw_input(frame: &mut Frame, app: &mut App, area: Rect) {
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
            Style::default()
                .fg(theme.fg_muted)
                .add_modifier(Modifier::ITALIC),
        )));
    } else if app.editing_message.is_some() {
        block = block.title(Line::from(Span::styled(
            " editing… ",
            Style::default()
                .fg(theme.accent_secondary)
                .add_modifier(Modifier::ITALIC),
        )));
    }

    // Build attachment badge if present
    let badge = app.pending_attachment.as_ref().map(|path| {
        let fname = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        // Detect type hint from extension
        let ext = path
            .extension()
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
    app.mouse.input_prefix_len = prefix_len as u16;
    let text_width = inner_width.saturating_sub(prefix_len); // usable chars for buffer text

    if app.input.buffer.is_empty() && badge.is_none() {
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
        let lines: Vec<&str> = app.input.buffer.split('\n').collect();
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
                        Style::default()
                            .fg(theme.mention)
                            .add_modifier(Modifier::BOLD),
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
                let visible_text: String = line_str
                    .chars()
                    .skip(char_scroll)
                    .take(text_width)
                    .collect();
                spans.push(Span::styled(visible_text, Style::default().fg(theme.fg)));
            } else {
                let visible_text: String = line_str.chars().take(text_width).collect();
                spans.push(Span::styled(visible_text, Style::default().fg(theme.fg)));
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
