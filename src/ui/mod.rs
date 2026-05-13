//! Stateless rendering layer.
//!
//! [`draw`] takes the current [`App`] and renders sidebar + chat + status
//! bar each frame. Sender colors are hash-based across an 8-color palette;
//! groups are prefixed with `#`. OSC 8 hyperlinks are injected post-render
//! to dodge ratatui width calculation bugs (see [`LinkRegion`]).

mod autocomplete;
mod chat_pane;
mod composer;
mod links;
mod overlays;
mod sidebar;
mod status_bar;
mod welcome;

use autocomplete::draw_autocomplete;
use chat_pane::draw_chat_area;
pub use links::LinkRegion;
use links::collect_link_regions;
use overlays::about::draw_about;
use overlays::action_menu::{draw_action_menu, draw_delete_confirm};
use overlays::contacts::draw_contacts;
use overlays::delete_conversation_confirm::draw_delete_conversation_confirm;
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
use overlays::settings::{draw_customize, draw_settings};
use overlays::settings_profile::draw_settings_profile_manager;
use overlays::theme_picker::draw_theme_picker;
use overlays::verify::draw_verify;
use sidebar::draw_sidebar;
use status_bar::draw_status_bar;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear},
};

use crate::app::{App, OverlayKind};
use crate::signal::types::MessageStatus;
use crate::theme::Theme;

// Layout constants
const SIDEBAR_AUTO_HIDE_WIDTH: u16 = 60;
const MIN_CHAT_WIDTH: u16 = 30;
pub(super) const MSG_WINDOW_MULTIPLIER: usize = 10;

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

    // Delete conversation confirmation overlay (/delete and message-request 'd')
    if app.is_overlay(OverlayKind::DeleteConversationConfirm) {
        draw_delete_conversation_confirm(frame, app, size);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::types::MessageStatus;
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
    use tempfile::tempdir;

    /// Fixed date for deterministic timestamps in snapshots.
    fn fixed_date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2025, 1, 15).unwrap()
    }

    /// Create a fully-populated demo App with deterministic data.
    fn demo_app() -> App {
        let dir = tempdir().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        // Leak the tempdir so its drop doesn't reclaim the path mid-test.
        // The OS reclaims temp dirs on process exit.
        std::mem::forget(dir);
        let db = Database::open_in_memory().unwrap();
        let mut app = App::new("+15559999999".to_string(), db, &config_path);
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
