mod app;
mod config;
mod db;
mod input;
mod link;
mod setup;
mod signal;
mod ui;

use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
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

use app::{App, InputMode};
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
            "--help" => {
                eprintln!("signal-tui - Terminal Signal client");
                eprintln!();
                eprintln!("Usage: signal-tui [OPTIONS]");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  -a, --account <NUMBER>  Phone number (E.164 format)");
                eprintln!("  -c, --config <PATH>     Config file path");
                eprintln!("      --setup             Run first-time setup wizard");
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
    let result = run_main_flow(&mut terminal, &mut config, force_setup).await;

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
) -> Result<()> {
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

    // Open database
    let db_dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("signal-tui");
    std::fs::create_dir_all(&db_dir)?;
    let db_path = db_dir.join("signal-tui.db");
    let database = db::Database::open(&db_path)?;

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
    let result = run_app(terminal, &mut signal_client, config, database).await;

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

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    signal_client: &mut SignalClient,
    config: &Config,
    db: db::Database,
) -> Result<()> {
    let mut app = App::new(config.account.clone(), db);
    app.load_from_db()?;
    app.set_connected();

    // Ask primary device to sync contacts/groups, then fetch them (best-effort)
    let _ = signal_client.send_sync_request().await;
    let _ = signal_client.list_contacts().await;
    let _ = signal_client.list_groups().await;

    loop {
        // Render
        terminal.draw(|frame| ui::draw(frame, &app))?;

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
                            (_, KeyCode::Backspace) => {
                                if app.input_cursor > 0 {
                                    app.input_cursor -= 1;
                                    app.input_buffer.remove(app.input_cursor);
                                }
                            }
                            (_, KeyCode::Delete) => {
                                if app.input_cursor < app.input_buffer.len() {
                                    app.input_buffer.remove(app.input_cursor);
                                }
                            }
                            (_, KeyCode::Left) => {
                                app.input_cursor = app.input_cursor.saturating_sub(1);
                            }
                            (_, KeyCode::Right) => {
                                if app.input_cursor < app.input_buffer.len() {
                                    app.input_cursor += 1;
                                }
                            }
                            (_, KeyCode::Home) => {
                                app.input_cursor = 0;
                            }
                            (_, KeyCode::End) => {
                                app.input_cursor = app.input_buffer.len();
                            }
                            (_, KeyCode::Char(c)) => {
                                app.input_buffer.insert(app.input_cursor, c);
                                app.input_cursor += 1;
                            }
                            _ => {}
                        },
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

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
