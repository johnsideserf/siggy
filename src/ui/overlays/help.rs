//! Help overlay: commands, shortcuts, keybindings, CLI options.
//!
//! Reads the active keybinding profile to render the dynamic
//! shortcut and Vim-mode columns so help reflects whatever the user
//! has rebound. Static sections (slash commands and CLI options)
//! are hard-coded since they don't depend on user config.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::centered_popup;
use crate::app::App;
use crate::keybindings::KeyAction;

pub(in crate::ui) fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let kb = &app.keybindings;

    // Help table entries: (key, description)
    let commands: &[(&str, &str)] = &[
        ("/join <name>", "Switch to a conversation"),
        ("/part", "Leave current conversation"),
        ("/delete", "Delete current conversation"),
        ("/attach", "Attach a file"),
        ("/search <query>", "Search messages"),
        ("/sidebar", "Toggle sidebar visibility"),
        ("/bell [type]", "Toggle notifications"),
        ("/mute", "Mute/unmute conversation"),
        ("/contacts", "Browse contacts"),
        ("/settings", "Open settings"),
        ("/keybindings", "Configure keybindings"),
        ("/lock", "Lock the session"),
        ("/lock-reset", "Change the lock passphrase"),
        ("/quit", "Exit siggy"),
    ];

    // Dynamic shortcuts from active keybindings
    let dk = |a: KeyAction| kb.display_key(a);
    let nav_keys = format!(
        "{} / {}",
        dk(KeyAction::NextConversation),
        dk(KeyAction::PrevConversation)
    );
    let scroll_keys = format!(
        "{} / {}",
        dk(KeyAction::PageScrollUp),
        dk(KeyAction::PageScrollDown)
    );
    let resize_keys = format!(
        "{} / {}",
        dk(KeyAction::ResizeSidebarLeft),
        dk(KeyAction::ResizeSidebarRight)
    );
    let quit_key = dk(KeyAction::Quit);
    let lock_key = dk(KeyAction::Lock);
    let shortcuts: Vec<(String, &str)> = vec![
        (nav_keys, "Next / prev conversation"),
        ("Up / Down".to_string(), "Recall input history"),
        ("@".to_string(), "Mention autocomplete"),
        (scroll_keys, "Scroll messages"),
        (resize_keys, "Resize sidebar"),
        (lock_key, "Lock the session"),
        (quit_key, "Quit"),
    ];

    let cli: &[(&str, &str)] = &[
        ("--incognito", "No local message storage"),
        ("--demo", "Launch with dummy data"),
        ("--setup", "Re-run first-time wizard"),
    ];

    // Dynamic normal-mode keybindings
    let exit_key = dk(KeyAction::ExitInsert);
    let insert_keys = format!(
        "{} / {} / {} / {} / {}",
        dk(KeyAction::InsertAtCursor),
        dk(KeyAction::InsertAfterCursor),
        dk(KeyAction::InsertLineStart),
        dk(KeyAction::InsertLineEnd),
        dk(KeyAction::OpenLineBelow)
    );
    let scroll_ud = format!(
        "{} / {}",
        dk(KeyAction::ScrollDown),
        dk(KeyAction::ScrollUp)
    );
    let focus_ud = format!(
        "{} / {}",
        dk(KeyAction::FocusNextMessage),
        dk(KeyAction::FocusPrevMessage)
    );
    let top_bottom = format!(
        "{} / {}",
        dk(KeyAction::ScrollToTop),
        dk(KeyAction::ScrollToBottom)
    );
    let half_page = format!(
        "{} / {}",
        dk(KeyAction::HalfPageDown),
        dk(KeyAction::HalfPageUp)
    );
    let cursor_lr = format!(
        "{} / {}",
        dk(KeyAction::CursorLeft),
        dk(KeyAction::CursorRight)
    );
    let word_fb = format!(
        "{} / {}",
        dk(KeyAction::WordForward),
        dk(KeyAction::WordBack)
    );
    let line_se = format!("{} / {}", dk(KeyAction::LineStart), dk(KeyAction::LineEnd));
    let del_keys = format!(
        "{} / {}",
        dk(KeyAction::DeleteChar),
        dk(KeyAction::DeleteToEnd)
    );
    let copy_keys = format!(
        "{} / {}",
        dk(KeyAction::CopyMessage),
        dk(KeyAction::CopyAllMessages)
    );
    let search_keys = format!(
        "{} / {}",
        dk(KeyAction::NextSearchResult),
        dk(KeyAction::PrevSearchResult)
    );

    let profile_label = format!("  Keybindings [{}]", app.keybindings.profile_name);
    let vim: Vec<(String, &str)> = vec![
        (exit_key, "Normal mode"),
        (insert_keys, "Insert mode"),
        (dk(KeyAction::InsertNewline), "Insert newline in input"),
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
    let content_lines = commands.len() + shortcuts.len() + vim.len() + cli.len() + 7;
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
            Span::styled(
                format!("  {:<width$}", key, width = key_col_width),
                key_style,
            ),
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
