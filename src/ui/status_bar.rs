//! Status bar rendering: mode, connection, conversation, scroll position.
//!
//! Renders the bottom status line with the input-mode indicator
//! (`[NORMAL]` / `[INSERT]`), connection dot and label, current
//! conversation name with `#` prefix for groups, conversation count,
//! and a scroll-position indicator (`↑N` plus focused-message
//! timestamp) when scrolled up. Two override paths short-circuit the
//! normal layout: the quit-confirm prompt and the sync progress
//! banner. The `[+]` glyph signals an auto-hidden sidebar.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{App, InputMode};

pub(super) fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect, sidebar_auto_hidden: bool) {
    let theme = &app.theme;

    // Override status bar with quit confirmation prompt
    if app.quit_confirm {
        let bar = Line::from(Span::styled(
            " Unsent message in buffer. Press quit again to confirm.",
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(
            Paragraph::new(bar).style(Style::default().bg(theme.statusbar_bg)),
            area,
        );
        return;
    }

    // Sync progress indicator (overrides normal status bar)
    if app.sync.active && app.sync.message_count > 0 {
        let bar = Line::from(vec![
            Span::styled(
                " Syncing... ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("({} messages received)", app.sync.message_count),
                Style::default().fg(theme.statusbar_fg),
            ),
        ]);
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
            let label = if let Some(pk) = app.pending_normal_key {
                format!(" [NORMAL] {pk}")
            } else {
                " [NORMAL] ".to_string()
            };
            segments.push(Span::styled(
                label,
                Style::default()
                    .fg(theme.accent_secondary)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        InputMode::Insert => {
            segments.push(Span::styled(
                " [INSERT] ",
                Style::default()
                    .fg(theme.success)
                    .add_modifier(Modifier::BOLD),
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
        segments.push(Span::styled(
            "connected",
            Style::default().fg(theme.statusbar_fg),
        ));
        if app.incognito {
            segments.push(Span::styled(" │ ", Style::default().fg(theme.fg_muted)));
            segments.push(Span::styled(
                "incognito",
                Style::default()
                    .fg(theme.mention)
                    .add_modifier(Modifier::BOLD),
            ));
        }
    } else {
        segments.push(Span::styled(" ● ", Style::default().fg(theme.error)));
        segments.push(Span::styled(
            "disconnected",
            Style::default().fg(theme.statusbar_fg),
        ));
    }

    // Pipe separator
    segments.push(Span::styled(" │ ", Style::default().fg(theme.fg_muted)));

    // Current conversation
    if let Some(ref id) = app.active_conversation {
        if let Some(conv) = app.store.conversations.get(id) {
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
    if !app.store.conversation_order.is_empty() {
        segments.push(Span::styled(" │ ", Style::default().fg(theme.fg_muted)));
        segments.push(Span::styled(
            format!("{} chats", app.store.conversation_order.len()),
            Style::default().fg(theme.fg_secondary),
        ));
    }

    // Scroll offset indicator + focused message timestamp
    if app.scroll.offset > 0 {
        segments.push(Span::styled(" │ ", Style::default().fg(theme.fg_muted)));
        segments.push(Span::styled(
            format!("↑{}", app.scroll.offset),
            Style::default().fg(theme.warning),
        ));
        if let Some(ref ts) = app.scroll.focused_time {
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
        segments.push(Span::styled("[+]", Style::default().fg(theme.fg_muted)));
    }

    // Pad the rest with background
    let status = Paragraph::new(Line::from(segments)).style(
        Style::default()
            .fg(theme.statusbar_fg)
            .bg(theme.statusbar_bg),
    );
    frame.render_widget(status, area);
}
