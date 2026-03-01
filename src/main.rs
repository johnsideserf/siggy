mod app;
mod config;
mod db;
mod debug_log;
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
    event::{self, Event, KeyEventKind},
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

use app::{App, Conversation, DisplayMessage, InputMode, SendRequest};
use config::Config;
use setup::SetupResult;
use signal::client::SignalClient;

/// Keyboard polling interval for the main event loop.
const POLL_TIMEOUT: Duration = Duration::from_millis(50);

#[tokio::main]
async fn main() -> Result<()> {
    // Disable the default Windows Ctrl+C handler — crossterm captures it as a
    // key event in raw mode, so the OS handler just causes a noisy exit code.
    #[cfg(windows)]
    unsafe {
        extern "system" {
            fn SetConsoleCtrlHandler(handler: usize, add: i32) -> i32;
        }
        SetConsoleCtrlHandler(0, 1);
    }

    // Parse CLI args
    let args: Vec<String> = std::env::args().collect();
    let mut config_path: Option<&str> = None;
    let mut account: Option<String> = None;
    let mut force_setup = false;
    let mut demo_mode = false;
    let mut incognito = false;
    let mut debug = false;

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
            "--debug" => {
                debug = true;
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
                eprintln!("      --debug             Write debug log to signal-tui-debug.log");
                eprintln!("      --help              Show this help");
                std::process::exit(0);
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    if debug {
        debug_log::enable();
        debug_log::log("=== signal-tui debug session started ===");
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

/// Write native terminal image protocol escape sequences to overlay
/// pre-resized images on top of the halfblock placeholders.
fn emit_native_images(
    backend: &mut CrosstermBackend<io::Stdout>,
    app: &mut App,
) -> Result<()> {
    let protocol = app.image_protocol;
    if app.visible_images.is_empty() || protocol == image_render::ImageProtocol::Halfblock {
        return Ok(());
    }
    use std::io::Write;

    // Take images out to avoid borrow conflict with native_image_cache
    let images = std::mem::take(&mut app.visible_images);

    queue!(backend, SavePosition)?;

    for img in &images {
        // Get or compute cached base64 PNG data
        let b64 = if let Some(cached) = app.native_image_cache.get(&img.path) {
            cached.clone()
        } else {
            let encoded = image_render::encode_native_png(
                std::path::Path::new(&img.path),
                img.width as u32,
                img.height as u32,
            );
            match encoded {
                Some(data) => {
                    app.native_image_cache.insert(img.path.clone(), data.clone());
                    data
                }
                None => continue,
            }
        };

        queue!(backend, MoveTo(img.x, img.y))?;

        match protocol {
            image_render::ImageProtocol::Kitty => {
                // f=100 = detect format, a=T = transmit+display
                // c/r = display size in cells, C=1 = don't move cursor
                let chunks: Vec<&[u8]> = b64.as_bytes().chunks(4096).collect();
                for (i, chunk) in chunks.iter().enumerate() {
                    let m = if i == chunks.len() - 1 { 0 } else { 1 };
                    let chunk_str = std::str::from_utf8(chunk).unwrap_or("");
                    if i == 0 {
                        write!(
                            backend,
                            "\x1b_Gf=100,a=T,c={},r={},C=1,m={m};{chunk_str}\x1b\\",
                            img.width, img.height
                        )?;
                    } else {
                        write!(backend, "\x1b_Gm={m};{chunk_str}\x1b\\")?;
                    }
                }
            }
            image_render::ImageProtocol::Iterm2 => {
                write!(
                    backend,
                    "\x1b]1337;File=inline=1;width={};height={};preserveAspectRatio=0:{b64}\x07",
                    img.width, img.height
                )?;
            }
            image_render::ImageProtocol::Halfblock => {}
        }
    }

    queue!(backend, RestorePosition)?;
    backend.flush()?;
    Ok(())
}

/// Send a message via signal-cli, tracking the pending send for receipt correlation.
async fn send_msg(
    signal_client: &mut SignalClient,
    app: &mut App,
    recipient: &str,
    body: &str,
    is_group: bool,
    local_ts_ms: i64,
) {
    match signal_client.send_message(recipient, body, is_group).await {
        Ok(rpc_id) => {
            debug_log::logf(format_args!("send: to={recipient} ts={local_ts_ms}"));
            app.pending_sends
                .insert(rpc_id, (recipient.to_string(), local_ts_ms));
        }
        Err(e) => {
            app.status_message = format!("send error: {e}");
        }
    }
}

/// Dispatch a SendRequest to signal-cli.
async fn dispatch_send(
    signal_client: &mut SignalClient,
    app: &mut App,
    req: SendRequest,
) {
    match req {
        SendRequest::Message { recipient, body, is_group, local_ts_ms } => {
            send_msg(signal_client, app, &recipient, &body, is_group, local_ts_ms).await;
        }
        SendRequest::Reaction {
            conv_id, emoji, is_group, target_author, target_timestamp, remove,
        } => {
            if let Err(e) = signal_client
                .send_reaction(&conv_id, is_group, &emoji, &target_author, target_timestamp, remove)
                .await
            {
                app.status_message = format!("reaction error: {e}");
            }
        }
    }
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
    app.native_images = config.native_images;
    app.incognito = incognito;
    app.show_receipts = config.show_receipts;
    app.color_receipts = config.color_receipts;
    app.nerd_fonts = config.nerd_fonts;
    app.reaction_verbose = config.reaction_verbose;
    app.load_from_db()?;
    app.set_connected();

    // Ask primary device to sync contacts/groups, then fetch them (best-effort)
    let _ = signal_client.send_sync_request().await;
    let _ = signal_client.list_contacts().await;
    let _ = signal_client.list_groups().await;

    loop {
        // Force full redraw when active conversation changes (clears native image artifacts)
        if app.native_images && app.active_conversation != app.prev_active_conversation {
            app.prev_active_conversation = app.active_conversation.clone();
            terminal.clear()?;
        }

        // Render
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        emit_osc8_links(terminal.backend_mut(), &app.link_regions)?;
        if app.native_images {
            emit_native_images(terminal.backend_mut(), &mut app)?;
        }

        // Poll for events with a short timeout so we stay responsive to signal events
        let has_terminal_event = event::poll(POLL_TIMEOUT)?;

        if has_terminal_event {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if !app.handle_global_key(key.modifiers, key.code) {
                    let (overlay_handled, send_request) = app.handle_overlay_key(key.code);
                    if let Some(req) = send_request {
                        dispatch_send(signal_client, &mut app, req).await;
                    }
                    if !overlay_handled {
                        let send_request = match app.mode {
                            InputMode::Normal => {
                                app.handle_normal_key(key.modifiers, key.code);
                                None
                            }
                            InputMode::Insert => app.handle_insert_key(key.modifiers, key.code),
                        };
                        if let Some(req) = send_request {
                            dispatch_send(signal_client, &mut app, req).await;
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
        if app.native_images && app.active_conversation != app.prev_active_conversation {
            app.prev_active_conversation = app.active_conversation.clone();
            terminal.clear()?;
        }

        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        emit_osc8_links(terminal.backend_mut(), &app.link_regions)?;
        if app.native_images {
            emit_native_images(terminal.backend_mut(), &mut app)?;
        }

        let has_terminal_event = event::poll(POLL_TIMEOUT)?;

        if has_terminal_event {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if !app.handle_global_key(key.modifiers, key.code) {
                    let (overlay_handled, _) = app.handle_overlay_key(key.code);
                    if !overlay_handled {
                        match app.mode {
                            InputMode::Normal => app.handle_normal_key(key.modifiers, key.code),
                            // In demo mode, messages echo locally but don't send
                            InputMode::Insert => { app.handle_insert_key(key.modifiers, key.code); }
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
        let is_outgoing = sender == "you";
        DisplayMessage {
            sender: sender.to_string(),
            timestamp: time,
            body: body.to_string(),
            is_system: false,
            image_lines: None,
            image_path: None,
            status: if is_outgoing { Some(crate::signal::types::MessageStatus::Sent) } else { None },
            timestamp_ms: time.timestamp_millis(),
            reactions: Vec::new(),
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
    app.active_conversation = Some(alice_id.clone());
    app.status_message = "connected | demo mode".to_string();

    // Add sample reactions to some messages
    use crate::signal::types::Reaction;
    if let Some(conv) = app.conversations.get_mut(&alice_id) {
        // Alice's first message gets a thumbs up from "you"
        if let Some(msg) = conv.messages.get_mut(0) {
            msg.reactions.push(Reaction { emoji: "\u{1f44d}".to_string(), sender: "you".to_string() });
        }
        // "you" message gets a heart from Alice
        if let Some(msg) = conv.messages.get_mut(1) {
            msg.reactions.push(Reaction { emoji: "\u{2764}\u{fe0f}".to_string(), sender: "Alice".to_string() });
        }
    }
    if let Some(conv) = app.conversations.get_mut("group_rustdevs") {
        // Group message gets multiple reactions
        if let Some(msg) = conv.messages.get_mut(3) {
            msg.reactions.push(Reaction { emoji: "\u{1f44d}".to_string(), sender: "Alice".to_string() });
            msg.reactions.push(Reaction { emoji: "\u{1f44d}".to_string(), sender: "Bob".to_string() });
            msg.reactions.push(Reaction { emoji: "\u{2764}\u{fe0f}".to_string(), sender: "Dave".to_string() });
        }
    }
}
