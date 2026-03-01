mod app;
mod config;
mod db;
mod image_render;
mod input;
mod link;
mod setup;
mod signal;
mod ui;

use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    cursor::{MoveTo, RestorePosition, SavePosition},
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Print, ResetColor, SetForegroundColor},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Flex, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Terminal,
};

use app::{App, Conversation, DisplayMessage, InputMode};
use config::Config;
use setup::SetupResult;
use signal::client::SignalClient;

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI args
    let args: Vec<String> = std::env::args().collect();
    let mut config_path: Option<&str> = None;
    let mut account: Option<String> = None;
    let mut force_setup = false;
    let mut demo_mode = false;
    let mut incognito = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-c" | "--config" => {
                if i + 1 < args.len() {
                    config_path = Some(&args[i + 1]);
                    i += 2;
                } else {
                    eprintln!("--config requires a path argument");
                    std::process::exit(1);
                }
            }
            "-a" | "--account" => {
                if i + 1 < args.len() {
                    account = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    eprintln!("--account requires a phone number");
                    std::process::exit(1);
                }
            }
            "--setup" => {
                force_setup = true;
                i += 1;
            }
            "--demo" => {
                demo_mode = true;
                i += 1;
            }
            "--incognito" => {
                incognito = true;
                i += 1;
            }
            "--help" => {
                eprintln!("signal-tui - Terminal Signal client");
                eprintln!();
                eprintln!("Usage: signal-tui [OPTIONS]");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  -a, --account <NUMBER>  Phone number (E.164 format)");
                eprintln!("  -c, --config <PATH>     Config file path");
                eprintln!("      --setup             Run first-time setup wizard");
                eprintln!("      --demo              Launch with dummy data (no signal-cli needed)");
                eprintln!("      --incognito         No local message storage (in-memory only)");
                eprintln!("      --help              Show this help");
                std::process::exit(0);
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    // Load config
    let mut config = Config::load(config_path)?;
    if let Some(acct) = account {
        config.account = acct;
    }

    // Set up terminal BEFORE anything else so all errors render in the TUI
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run the main flow inside a closure so we can always restore the terminal
    let result = run_main_flow(&mut terminal, &mut config, force_setup, demo_mode, incognito).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Err(e) = result {
        eprintln!("Error: {e:?}");
        std::process::exit(1);
    }

    Ok(())
}

async fn run_main_flow(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: &mut Config,
    force_setup: bool,
    demo_mode: bool,
    incognito: bool,
) -> Result<()> {
    if demo_mode {
        let database = db::Database::open_in_memory()?;
        return run_demo_app(terminal, database).await;
    }

    // Run setup wizard if needed
    let mut setup_handled_linking = false;
    if config.needs_setup() || force_setup {
        match setup::run_setup(terminal, config, force_setup).await? {
            SetupResult::Completed(new_config) => {
                *config = new_config;
                setup_handled_linking = true;
            }
            SetupResult::Skipped => {}
            SetupResult::Cancelled => {
                return Ok(());
            }
        }
    }

    // Create download directory
    if !config.download_dir.exists() {
        std::fs::create_dir_all(&config.download_dir)?;
    }

    // Open database (in-memory for incognito mode)
    let database = if incognito {
        db::Database::open_in_memory()?
    } else {
        let db_dir = dirs::data_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("signal-tui");
        std::fs::create_dir_all(&db_dir)?;
        let db_path = db_dir.join("signal-tui.db");
        db::Database::open(&db_path)?
    };

    // Quick pre-flight: check if account is registered (skip if wizard already handled it)
    if !setup_handled_linking {
        match link::check_account_registered(config).await {
            Ok(false) => {
                // Not registered — run linking flow
                match link::run_linking_flow(terminal, config).await {
                    Ok(link::LinkResult::Success) => {}
                    Ok(link::LinkResult::Cancelled) => {
                        return Ok(());
                    }
                    Err(e) => {
                        let msg = format!("{e}");
                        show_error_screen(terminal, "Device Linking Failed", &msg).await?;
                        return Ok(());
                    }
                }
            }
            Ok(true) => {} // Good to go
            Err(_) => {}   // Can't check, proceed anyway (graceful degradation)
        }
    }

    // Spawn signal-cli backend
    let signal_result = SignalClient::spawn(config).await;
    let mut signal_client = match signal_result {
        Ok(client) => client,
        Err(e) => {
            let msg = format!("{e}");
            show_error_screen(terminal, "Failed to Start signal-cli", &msg).await?;
            return Ok(());
        }
    };

    // Run the app
    let result = run_app(terminal, &mut signal_client, config, database, incognito).await;

    // Shut down signal-cli
    signal_client.shutdown().await?;

    result
}

/// Show a full-screen error in the TUI instead of crashing to stderr.
async fn show_error_screen(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    title: &str,
    message: &str,
) -> Result<()> {
    let title = title.to_string();
    let message = message.to_string();

    loop {
        let title = title.clone();
        let message = message.clone();
        terminal.draw(|frame| {
            let area = frame.area();

            let [_, content_area, _] = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(12),
                Constraint::Min(1),
            ])
            .flex(Flex::Center)
            .areas(area);

            let [content] = Layout::horizontal([Constraint::Percentage(70)])
                .flex(Flex::Center)
                .areas(content_area);

            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Red))
                .title(format!(" {} ", title))
                .title_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
            let inner = block.inner(content);
            frame.render_widget(block, content);

            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {message}"),
                    Style::default().fg(Color::Red),
                )),
                Line::from(""),
                Line::from(""),
                Line::from(Span::styled(
                    "  Check that signal-cli is installed and accessible.",
                    Style::default().fg(Color::Gray),
                )),
                Line::from(Span::styled(
                    "  Run with --setup to reconfigure.",
                    Style::default().fg(Color::Gray),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  Press any key to exit",
                    Style::default().fg(Color::DarkGray),
                )),
            ];

            let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
            frame.render_widget(paragraph, inner);
        })?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    return Ok(());
                }
            }
        }
    }
}

/// Write OSC 8 terminal hyperlink escape sequences directly to the terminal
/// for each detected link region, bypassing ratatui's buffer.
fn emit_osc8_links(
    backend: &mut CrosstermBackend<io::Stdout>,
    links: &[ui::LinkRegion],
) -> Result<()> {
    if links.is_empty() {
        return Ok(());
    }
    use std::io::Write;
    queue!(backend, SavePosition)?;
    for link in links {
        queue!(backend, MoveTo(link.x, link.y))?;
        queue!(
            backend,
            SetForegroundColor(crossterm::style::Color::Blue)
        )?;
        queue!(
            backend,
            Print(format!(
                "\x1b]8;;{}\x07{}\x1b]8;;\x07",
                link.url, link.text
            ))
        )?;
        queue!(backend, ResetColor)?;
    }
    queue!(backend, RestorePosition)?;
    backend.flush()?;
    Ok(())
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    signal_client: &mut SignalClient,
    config: &Config,
    db: db::Database,
    incognito: bool,
) -> Result<()> {
    let mut app = App::new(config.account.clone(), db);
    app.notify_direct = config.notify_direct;
    app.notify_group = config.notify_group;
    app.inline_images = config.inline_images;
    app.incognito = incognito;
    app.load_from_db()?;
    app.set_connected();

    // Ask primary device to sync contacts/groups, then fetch them (best-effort)
    let _ = signal_client.send_sync_request().await;
    let _ = signal_client.list_contacts().await;
    let _ = signal_client.list_groups().await;

    loop {
        // Render
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        emit_osc8_links(terminal.backend_mut(), &app.link_regions)?;

        // Poll for events with a short timeout so we stay responsive to signal events
        let has_terminal_event = event::poll(Duration::from_millis(50))?;

        if has_terminal_event {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                // === Global keys (both modes) ===
                let handled = match (key.modifiers, key.code) {
                    (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                        app.should_quit = true;
                        true
                    }
                    (KeyModifiers::NONE, KeyCode::Tab) => {
                        app.next_conversation();
                        true
                    }
                    (KeyModifiers::SHIFT, KeyCode::BackTab) => {
                        app.prev_conversation();
                        true
                    }
                    (KeyModifiers::CONTROL, KeyCode::Left) => {
                        app.resize_sidebar(-2);
                        true
                    }
                    (KeyModifiers::CONTROL, KeyCode::Right) => {
                        app.resize_sidebar(2);
                        true
                    }
                    (_, KeyCode::PageUp) => {
                        app.scroll_offset = app.scroll_offset.saturating_add(5);
                        true
                    }
                    (_, KeyCode::PageDown) => {
                        app.scroll_offset = app.scroll_offset.saturating_sub(5);
                        true
                    }
                    _ => false,
                };

                if !handled {
                    if app.show_help {
                        // Any key dismisses the help overlay
                        app.show_help = false;
                    } else if app.show_settings {
                        app.handle_settings_key(key.code);
                    } else if app.autocomplete_visible {
                        if let Some((recipient, body, is_group)) =
                            app.handle_autocomplete_key(key.code)
                        {
                            if let Err(e) =
                                signal_client
                                    .send_message(&recipient, &body, is_group)
                                    .await
                            {
                                app.status_message = format!("send error: {e}");
                            }
                        }
                    } else {
                    match app.mode {
                        // === Normal mode ===
                        InputMode::Normal => match (key.modifiers, key.code) {
                            // Scrolling
                            (_, KeyCode::Char('j')) => {
                                app.scroll_offset = app.scroll_offset.saturating_sub(1);
                            }
                            (_, KeyCode::Char('k')) => {
                                app.scroll_offset = app.scroll_offset.saturating_add(1);
                            }
                            (KeyModifiers::CONTROL, KeyCode::Char('d')) => {
                                app.scroll_offset = app.scroll_offset.saturating_sub(10);
                            }
                            (KeyModifiers::CONTROL, KeyCode::Char('u')) => {
                                app.scroll_offset = app.scroll_offset.saturating_add(10);
                            }
                            (_, KeyCode::Char('g')) => {
                                // Scroll to top
                                if let Some(ref id) = app.active_conversation {
                                    if let Some(conv) = app.conversations.get(id) {
                                        app.scroll_offset = conv.messages.len();
                                    }
                                }
                            }
                            (_, KeyCode::Char('G')) => {
                                // Scroll to bottom
                                app.scroll_offset = 0;
                            }

                            // Switch to Insert mode
                            (_, KeyCode::Char('i')) => {
                                app.mode = InputMode::Insert;
                            }
                            (_, KeyCode::Char('a')) => {
                                // Cursor right 1, then Insert
                                if app.input_cursor < app.input_buffer.len() {
                                    app.input_cursor += 1;
                                }
                                app.mode = InputMode::Insert;
                            }
                            (_, KeyCode::Char('I')) => {
                                app.input_cursor = 0;
                                app.mode = InputMode::Insert;
                            }
                            (_, KeyCode::Char('A')) => {
                                app.input_cursor = app.input_buffer.len();
                                app.mode = InputMode::Insert;
                            }
                            (_, KeyCode::Char('o')) => {
                                app.input_buffer.clear();
                                app.input_cursor = 0;
                                app.mode = InputMode::Insert;
                            }

                            // Cursor movement (Normal mode)
                            (_, KeyCode::Char('h')) => {
                                app.input_cursor = app.input_cursor.saturating_sub(1);
                            }
                            (_, KeyCode::Char('l')) => {
                                if app.input_cursor < app.input_buffer.len() {
                                    app.input_cursor += 1;
                                }
                            }
                            (_, KeyCode::Char('0')) => {
                                app.input_cursor = 0;
                            }
                            (_, KeyCode::Char('$')) => {
                                app.input_cursor = app.input_buffer.len();
                            }
                            (_, KeyCode::Char('w')) => {
                                // Move cursor forward one word (Unicode-safe)
                                let buf = &app.input_buffer;
                                let mut pos = app.input_cursor;
                                // Skip current word chars
                                while pos < buf.len() {
                                    let c = buf[pos..].chars().next().unwrap();
                                    if c.is_whitespace() { break; }
                                    pos += c.len_utf8();
                                }
                                // Skip whitespace
                                while pos < buf.len() {
                                    let c = buf[pos..].chars().next().unwrap();
                                    if !c.is_whitespace() { break; }
                                    pos += c.len_utf8();
                                }
                                app.input_cursor = pos;
                            }
                            (_, KeyCode::Char('b')) => {
                                // Move cursor back one word (Unicode-safe)
                                let buf = &app.input_buffer;
                                let mut pos = app.input_cursor;
                                // Skip whitespace backwards
                                while pos > 0 {
                                    let prev = buf[..pos].chars().next_back().unwrap();
                                    if !prev.is_whitespace() { break; }
                                    pos -= prev.len_utf8();
                                }
                                // Skip word chars backwards
                                while pos > 0 {
                                    let prev = buf[..pos].chars().next_back().unwrap();
                                    if prev.is_whitespace() { break; }
                                    pos -= prev.len_utf8();
                                }
                                app.input_cursor = pos;
                            }

                            // Buffer editing (stay in Normal mode)
                            (_, KeyCode::Char('x')) => {
                                if app.input_cursor < app.input_buffer.len() {
                                    app.input_buffer.remove(app.input_cursor);
                                    // Keep cursor within bounds
                                    if app.input_cursor > 0
                                        && app.input_cursor >= app.input_buffer.len()
                                    {
                                        app.input_cursor = app.input_buffer.len().saturating_sub(1);
                                    }
                                }
                            }
                            (_, KeyCode::Char('D')) => {
                                // Delete from cursor to end
                                app.input_buffer.truncate(app.input_cursor);
                            }

                            // Quick actions
                            (_, KeyCode::Char('/')) => {
                                app.input_buffer = "/".to_string();
                                app.input_cursor = 1;
                                app.mode = InputMode::Insert;
                                app.update_autocomplete();
                            }
                            (_, KeyCode::Esc) => {
                                // Clear buffer if non-empty
                                if !app.input_buffer.is_empty() {
                                    app.input_buffer.clear();
                                    app.input_cursor = 0;
                                }
                            }

                            _ => {}
                        },

                        // === Insert mode ===
                        InputMode::Insert => match (key.modifiers, key.code) {
                            (_, KeyCode::Esc) => {
                                app.mode = InputMode::Normal;
                                app.autocomplete_visible = false;
                            }
                            (_, KeyCode::Enter) => {
                                if let Some((recipient, body, is_group)) = app.handle_input() {
                                    if let Err(e) =
                                        signal_client
                                            .send_message(&recipient, &body, is_group)
                                            .await
                                    {
                                        app.status_message = format!("send error: {e}");
                                    }
                                }
                            }
                            _ => {
                                let needs_ac_update = matches!(
                                    key.code,
                                    KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_)
                                );
                                app.apply_input_edit(key.code);
                                if needs_ac_update {
                                    app.update_autocomplete();
                                }
                            }
                        },
                    }
                    }
                }
            }
        }

        // Drain signal events (non-blocking), detect disconnect
        loop {
            match signal_client.event_rx.try_recv() {
                Ok(ev) => app.handle_signal_event(ev),
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    if app.connection_error.is_none() {
                        app.connection_error = Some("signal-cli disconnected".to_string());
                        app.connected = false;
                    }
                    break;
                }
                Err(_) => break, // Empty, no more events
            }
        }

        // Expire stale typing indicators
        app.cleanup_typing();

        // Terminal bell on new messages in background conversations
        if app.pending_bell {
            app.pending_bell = false;
            execute!(terminal.backend_mut(), crossterm::style::Print("\x07"))?;
        }

        // Update terminal title with unread count
        let unread = app.total_unread();
        let title = if unread > 0 {
            format!("signal-tui ({unread})")
        } else {
            "signal-tui".to_string()
        };
        execute!(terminal.backend_mut(), crossterm::terminal::SetTitle(&title))?;

        if app.should_quit {
            break;
        }
    }

    // Restore terminal title on exit
    execute!(terminal.backend_mut(), crossterm::terminal::SetTitle(""))
        .ok();

    Ok(())
}

async fn run_demo_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    db: db::Database,
) -> Result<()> {
    let mut app = App::new("+15551234567".to_string(), db);
    app.connected = true;
    app.status_message = "connected | demo mode".to_string();

    populate_demo_data(&mut app);

    loop {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        emit_osc8_links(terminal.backend_mut(), &app.link_regions)?;

        let has_terminal_event = event::poll(Duration::from_millis(50))?;

        if has_terminal_event {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let handled = match (key.modifiers, key.code) {
                    (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                        app.should_quit = true;
                        true
                    }
                    (KeyModifiers::NONE, KeyCode::Tab) => {
                        app.next_conversation();
                        true
                    }
                    (KeyModifiers::SHIFT, KeyCode::BackTab) => {
                        app.prev_conversation();
                        true
                    }
                    (KeyModifiers::CONTROL, KeyCode::Left) => {
                        app.resize_sidebar(-2);
                        true
                    }
                    (KeyModifiers::CONTROL, KeyCode::Right) => {
                        app.resize_sidebar(2);
                        true
                    }
                    (_, KeyCode::PageUp) => {
                        app.scroll_offset = app.scroll_offset.saturating_add(5);
                        true
                    }
                    (_, KeyCode::PageDown) => {
                        app.scroll_offset = app.scroll_offset.saturating_sub(5);
                        true
                    }
                    _ => false,
                };

                if !handled {
                    if app.show_help {
                        app.show_help = false;
                    } else if app.show_settings {
                        app.handle_settings_key(key.code);
                    } else if app.autocomplete_visible {
                        // In demo mode, autocomplete commands are no-ops for sending
                        app.handle_autocomplete_key(key.code);
                    } else {
                        match app.mode {
                            InputMode::Normal => match (key.modifiers, key.code) {
                                (_, KeyCode::Char('j')) => {
                                    app.scroll_offset = app.scroll_offset.saturating_sub(1);
                                }
                                (_, KeyCode::Char('k')) => {
                                    app.scroll_offset = app.scroll_offset.saturating_add(1);
                                }
                                (KeyModifiers::CONTROL, KeyCode::Char('d')) => {
                                    app.scroll_offset = app.scroll_offset.saturating_sub(10);
                                }
                                (KeyModifiers::CONTROL, KeyCode::Char('u')) => {
                                    app.scroll_offset = app.scroll_offset.saturating_add(10);
                                }
                                (_, KeyCode::Char('g')) => {
                                    if let Some(ref id) = app.active_conversation {
                                        if let Some(conv) = app.conversations.get(id) {
                                            app.scroll_offset = conv.messages.len();
                                        }
                                    }
                                }
                                (_, KeyCode::Char('G')) => {
                                    app.scroll_offset = 0;
                                }
                                (_, KeyCode::Char('i')) => {
                                    app.mode = InputMode::Insert;
                                }
                                (_, KeyCode::Char('a')) => {
                                    if app.input_cursor < app.input_buffer.len() {
                                        app.input_cursor += 1;
                                    }
                                    app.mode = InputMode::Insert;
                                }
                                (_, KeyCode::Char('I')) => {
                                    app.input_cursor = 0;
                                    app.mode = InputMode::Insert;
                                }
                                (_, KeyCode::Char('A')) => {
                                    app.input_cursor = app.input_buffer.len();
                                    app.mode = InputMode::Insert;
                                }
                                (_, KeyCode::Char('o')) => {
                                    app.input_buffer.clear();
                                    app.input_cursor = 0;
                                    app.mode = InputMode::Insert;
                                }
                                (_, KeyCode::Char('h')) => {
                                    app.input_cursor = app.input_cursor.saturating_sub(1);
                                }
                                (_, KeyCode::Char('l')) => {
                                    if app.input_cursor < app.input_buffer.len() {
                                        app.input_cursor += 1;
                                    }
                                }
                                (_, KeyCode::Char('0')) => {
                                    app.input_cursor = 0;
                                }
                                (_, KeyCode::Char('$')) => {
                                    app.input_cursor = app.input_buffer.len();
                                }
                                (_, KeyCode::Char('w')) => {
                                    let buf = &app.input_buffer;
                                    let mut pos = app.input_cursor;
                                    while pos < buf.len() {
                                        let c = buf[pos..].chars().next().unwrap();
                                        if c.is_whitespace() { break; }
                                        pos += c.len_utf8();
                                    }
                                    while pos < buf.len() {
                                        let c = buf[pos..].chars().next().unwrap();
                                        if !c.is_whitespace() { break; }
                                        pos += c.len_utf8();
                                    }
                                    app.input_cursor = pos;
                                }
                                (_, KeyCode::Char('b')) => {
                                    let buf = &app.input_buffer;
                                    let mut pos = app.input_cursor;
                                    while pos > 0 {
                                        let prev = buf[..pos].chars().next_back().unwrap();
                                        if !prev.is_whitespace() { break; }
                                        pos -= prev.len_utf8();
                                    }
                                    while pos > 0 {
                                        let prev = buf[..pos].chars().next_back().unwrap();
                                        if prev.is_whitespace() { break; }
                                        pos -= prev.len_utf8();
                                    }
                                    app.input_cursor = pos;
                                }
                                (_, KeyCode::Char('x')) => {
                                    if app.input_cursor < app.input_buffer.len() {
                                        app.input_buffer.remove(app.input_cursor);
                                        if app.input_cursor > 0
                                            && app.input_cursor >= app.input_buffer.len()
                                        {
                                            app.input_cursor = app.input_buffer.len().saturating_sub(1);
                                        }
                                    }
                                }
                                (_, KeyCode::Char('D')) => {
                                    app.input_buffer.truncate(app.input_cursor);
                                }
                                (_, KeyCode::Char('/')) => {
                                    app.input_buffer = "/".to_string();
                                    app.input_cursor = 1;
                                    app.mode = InputMode::Insert;
                                    app.update_autocomplete();
                                }
                                (_, KeyCode::Esc) => {
                                    if !app.input_buffer.is_empty() {
                                        app.input_buffer.clear();
                                        app.input_cursor = 0;
                                    }
                                }
                                _ => {}
                            },
                            InputMode::Insert => match (key.modifiers, key.code) {
                                (_, KeyCode::Esc) => {
                                    app.mode = InputMode::Normal;
                                    app.autocomplete_visible = false;
                                }
                                (_, KeyCode::Enter) => {
                                    // In demo mode, messages echo locally but don't send
                                    app.handle_input();
                                }
                                _ => {
                                    let needs_ac_update = matches!(
                                        key.code,
                                        KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_)
                                    );
                                    app.apply_input_edit(key.code);
                                    if needs_ac_update {
                                        app.update_autocomplete();
                                    }
                                }
                            },
                        }
                    }
                }
            }
        }

        app.cleanup_typing();

        let unread = app.total_unread();
        let title = if unread > 0 {
            format!("signal-tui ({unread})")
        } else {
            "signal-tui".to_string()
        };
        execute!(terminal.backend_mut(), crossterm::terminal::SetTitle(&title))?;

        if app.should_quit {
            break;
        }
    }

    execute!(terminal.backend_mut(), crossterm::terminal::SetTitle(""))
        .ok();

    Ok(())
}

fn populate_demo_data(app: &mut App) {
    use chrono::{TimeZone, Utc};

    let today = Utc::now().date_naive();
    let ts = |hour: u32, min: u32| -> chrono::DateTime<Utc> {
        Utc.from_utc_datetime(
            &today
                .and_hms_opt(hour, min, 0)
                .unwrap_or_else(|| today.and_hms_opt(12, 0, 0).unwrap()),
        )
    };

    let dm = |sender: &str, time: chrono::DateTime<Utc>, body: &str| -> DisplayMessage {
        DisplayMessage {
            sender: sender.to_string(),
            timestamp: time,
            body: body.to_string(),
            is_system: false,
            image_lines: None,
        }
    };

    // --- Alice: weekend plans ---
    let alice_id = "+15550001111".to_string();
    let alice = Conversation {
        name: "Alice".to_string(),
        id: alice_id.clone(),
        messages: vec![
            dm("Alice", ts(9, 15), "Hey! Are you free this weekend?"),
            dm("you", ts(9, 17), "Yeah, what did you have in mind?"),
            dm("Alice", ts(9, 18), "There's a farmers market Saturday morning"),
            dm("you", ts(9, 20), "Sounds great, what time?"),
            dm("Alice", ts(9, 21), "Opens at 8, but 9 is fine. Less crowded."),
            dm("you", ts(9, 23), "Perfect, let's do 9"),
        ],
        unread: 0,
        is_group: false,
    };

    // --- Bob: code review ---
    let bob_id = "+15550002222".to_string();
    let bob = Conversation {
        name: "Bob".to_string(),
        id: bob_id.clone(),
        messages: vec![
            dm("Bob", ts(10, 5), "Can you review my PR? It's the auth refactor"),
            dm("you", ts(10, 12), "Sure, I'll take a look after lunch"),
            dm("Bob", ts(10, 13), "Thanks! No rush, just need it before Thursday"),
        ],
        unread: 0,
        is_group: false,
    };

    // --- Carol: single unread ---
    let carol_id = "+15550003333".to_string();
    let carol = Conversation {
        name: "Carol".to_string(),
        id: carol_id.clone(),
        messages: vec![
            dm("Carol", ts(11, 45), "Did you see the announcement about the office move?"),
        ],
        unread: 1,
        is_group: false,
    };

    // --- Dave: older meetup conversation ---
    let dave_id = "+15550004444".to_string();
    let dave = Conversation {
        name: "Dave".to_string(),
        id: dave_id.clone(),
        messages: vec![
            dm("Dave", ts(8, 0), "Meetup is at the usual place, 7pm"),
            dm("you", ts(8, 5), "Got it, I'll be there"),
            dm("Dave", ts(8, 6), "Bring your laptop if you want to hack on stuff"),
        ],
        unread: 0,
        is_group: false,
    };

    // --- #Rust Devs: group technical discussion ---
    let rust_id = "group_rustdevs".to_string();
    let rust_group = Conversation {
        name: "#Rust Devs".to_string(),
        id: rust_id.clone(),
        messages: vec![
            dm("Alice", ts(10, 30), "Has anyone tried the new async trait syntax?"),
            dm("Bob", ts(10, 32), "Yeah, it's so much cleaner than the pin-based approach"),
            dm("Dave", ts(10, 35), "I'm still wrapping my head around it"),
            dm("you", ts(10, 40), "The desugaring docs helped me a lot"),
            dm("Alice", ts(10, 42), "Can you share the link?"),
            dm("you", ts(10, 43), "Sure, one sec"),
        ],
        unread: 0,
        is_group: true,
    };

    // --- #Family: group with unread ---
    let family_id = "group_family".to_string();
    let family_group = Conversation {
        name: "#Family".to_string(),
        id: family_id.clone(),
        messages: vec![
            dm("Mom", ts(12, 0), "Dinner at our place Sunday?"),
            dm("Dad", ts(12, 5), "I'll fire up the grill"),
            dm("you", ts(12, 10), "Count me in!"),
            dm("Mom", ts(13, 30), "Great! Bring dessert if you can"),
            dm("Dad", ts(13, 35), "I picked up some corn and burgers"),
        ],
        unread: 2,
        is_group: true,
    };

    // Insert conversations and set ordering
    let order = vec![
        family_id.clone(),
        carol_id.clone(),
        rust_id.clone(),
        bob_id.clone(),
        alice_id.clone(),
        dave_id.clone(),
    ];

    for conv in [alice, bob, carol, dave, rust_group, family_group] {
        let id = conv.id.clone();
        let msg_count = conv.messages.len();
        let unread = conv.unread;
        app.conversations.insert(id.clone(), conv);
        if msg_count > 0 {
            app.last_read_index
                .insert(id, msg_count.saturating_sub(unread));
        }
    }

    app.conversation_order = order;
    app.active_conversation = Some(alice_id);
    app.status_message = "connected | demo mode".to_string();
}
