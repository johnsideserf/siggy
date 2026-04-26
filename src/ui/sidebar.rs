//! Sidebar (conversation list) rendering.
//!
//! Renders the left/right pane that lists conversations: active marker,
//! unread / message-request indicators, group `#` prefix, mute and
//! blocked decorations. Honors the sidebar filter overlay (`/_`) by
//! swapping the title and the candidate list. Writes the inner Rect
//! to `app.mouse.sidebar_inner` so click-to-focus knows where to hit.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem},
};

use super::truncate;
use crate::app::{App, OverlayKind};

pub(super) fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = &app.theme;
    let max_name_width = (area.width as usize).saturating_sub(5); // "• # " + margin

    // Use filtered list when sidebar filter is active.
    // When filtering, show everything (so users can find hidden conversations).
    // In normal view, hide stale conversations (empty groups, unresolvable contacts).
    let display_order: Vec<String> = if app.is_overlay(OverlayKind::SidebarFilter) {
        if app.sidebar_filter.is_empty() {
            app.store.conversation_order.clone()
        } else {
            app.sidebar_filtered.clone()
        }
    } else {
        app.store
            .conversation_order
            .iter()
            .filter(|id| {
                app.active_conversation.as_ref() == Some(id)
                    || app
                        .store
                        .conversations
                        .get(*id)
                        .is_some_and(|c| !c.is_stale())
            })
            .cloned()
            .collect()
    };

    let now = chrono::Utc::now();
    let items: Vec<ListItem> = display_order
        .iter()
        .map(|id| {
            let conv = &app.store.conversations[id];
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
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
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
                spans.push(Span::styled("#", Style::default().fg(theme.fg_muted)));
            }

            // Conversation name
            let mute_state = app.active_mute(id, now);
            let name_style = if is_active {
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
            } else if has_unread {
                Style::default().fg(theme.warning)
            } else if mute_state.is_some() {
                Style::default().fg(theme.fg_muted)
            } else {
                Style::default().fg(theme.fg_secondary)
            };
            spans.push(Span::styled(name, name_style));

            if has_unread && !is_active {
                spans.push(Span::styled(
                    format!(" ({})", conv.unread),
                    Style::default().fg(theme.warning),
                ));
            }

            if let Some(indicator) = mute_state.and_then(|m| m.sidebar_indicator(now)) {
                spans.push(Span::styled(indicator, Style::default().fg(theme.fg_muted)));
            }
            if app.blocked_conversations.contains(id) {
                spans.push(Span::styled(" x", Style::default().fg(theme.error)));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    let border_side = if app.sidebar_on_right {
        Borders::LEFT
    } else {
        Borders::RIGHT
    };
    let title = if app.is_overlay(OverlayKind::SidebarFilter) {
        if app.sidebar_filter.is_empty() {
            " /_ ".to_string()
        } else {
            format!(" /{} ", app.sidebar_filter)
        }
    } else {
        " Chats ".to_string()
    };
    let title_style = if app.is_overlay(OverlayKind::SidebarFilter) {
        Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    };
    let block = Block::default()
        .borders(border_side)
        .border_type(BorderType::Rounded)
        .title(title)
        .title_style(title_style);
    app.mouse.sidebar_inner = Some(block.inner(area));

    let sidebar = List::new(items).block(block);
    frame.render_widget(sidebar, area);
}
