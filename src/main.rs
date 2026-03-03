mod app;
mod config;
mod db;
mod debug_log;
mod image_render;
mod input;
mod link;
mod setup;
mod signal;
mod theme;
mod ui;

use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    cursor::{MoveTo, RestorePosition, SavePosition},
    event::{self, EnableMouseCapture, DisableMouseCapture, Event, KeyEventKind},
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
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run the main flow inside a closure so we can always restore the terminal
    let result = run_main_flow(&mut terminal, &mut config, force_setup, demo_mode, incognito).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
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
            Err(e) => {
                debug_log::logf(format_args!("check_account_registered failed: {e}"));
            }
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

/// Convert a ratatui Color to a crossterm Color for direct terminal output.
fn ratatui_color_to_crossterm(c: Color) -> crossterm::style::Color {
    match c {
        Color::Reset => crossterm::style::Color::Reset,
        Color::Black => crossterm::style::Color::Black,
        Color::Red => crossterm::style::Color::DarkRed,
        Color::Green => crossterm::style::Color::DarkGreen,
        Color::Yellow => crossterm::style::Color::DarkYellow,
        Color::Blue => crossterm::style::Color::DarkBlue,
        Color::Magenta => crossterm::style::Color::DarkMagenta,
        Color::Cyan => crossterm::style::Color::DarkCyan,
        Color::Gray => crossterm::style::Color::Grey,
        Color::DarkGray => crossterm::style::Color::DarkGrey,
        Color::LightRed => crossterm::style::Color::Red,
        Color::LightGreen => crossterm::style::Color::Green,
        Color::LightYellow => crossterm::style::Color::Yellow,
        Color::LightBlue => crossterm::style::Color::Blue,
        Color::LightMagenta => crossterm::style::Color::Magenta,
        Color::LightCyan => crossterm::style::Color::Cyan,
        Color::White => crossterm::style::Color::White,
        Color::Rgb(r, g, b) => crossterm::style::Color::Rgb { r, g, b },
        Color::Indexed(i) => crossterm::style::Color::AnsiValue(i),
    }
}

/// Write OSC 8 terminal hyperlink escape sequences directly to the terminal
/// for each detected link region, bypassing ratatui's buffer.
fn emit_osc8_links(
    backend: &mut CrosstermBackend<io::Stdout>,
    links: &[ui::LinkRegion],
    link_color: Color,
) -> Result<()> {
    if links.is_empty() {
        return Ok(());
    }
    use crossterm::style::SetBackgroundColor;
    use std::io::Write;
    queue!(backend, SavePosition)?;
    for link in links {
        queue!(backend, MoveTo(link.x, link.y))?;
        queue!(
            backend,
            SetForegroundColor(ratatui_color_to_crossterm(link_color))
        )?;
        if let Some(bg) = link.bg {
            // Preserve the background color (e.g. highlight) that ratatui rendered.
            let ct_bg = ratatui_color_to_crossterm(bg);
            queue!(backend, SetBackgroundColor(ct_bg))?;
        }
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

/// Dispatch a SendRequest to signal-cli.
async fn dispatch_send(
    signal_client: &mut SignalClient,
    app: &mut App,
    req: SendRequest,
) {
    match req {
        SendRequest::Message { recipient, body, is_group, local_ts_ms, mentions, attachment, quote_timestamp, quote_author, quote_body } => {
            let attachments: Vec<std::path::PathBuf> = attachment.into_iter().collect();
            let quote = match (quote_author, quote_timestamp, quote_body) {
                (Some(author), Some(ts), Some(body_text)) => Some((author, ts, body_text)),
                _ => None,
            };
            let att_refs: Vec<&std::path::Path> = attachments.iter().map(|p| p.as_path()).collect();
            match signal_client.send_message(&recipient, &body, is_group, &mentions, &att_refs, quote.as_ref().map(|(a, t, b)| (a.as_str(), *t, b.as_str()))).await {
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
        SendRequest::Edit { recipient, body, is_group, edit_timestamp, local_ts_ms, mentions, quote_timestamp, quote_author, quote_body } => {
            let quote = match (quote_author, quote_timestamp, quote_body) {
                (Some(author), Some(ts), Some(body_text)) => Some((author, ts, body_text)),
                _ => None,
            };
            match signal_client.send_edit_message(&recipient, &body, is_group, edit_timestamp, &mentions, quote.as_ref().map(|(a, t, b)| (a.as_str(), *t, b.as_str()))).await {
                Ok(rpc_id) => {
                    debug_log::logf(format_args!("edit: to={recipient} ts={edit_timestamp}"));
                    app.pending_sends.insert(rpc_id, (recipient.to_string(), local_ts_ms));
                }
                Err(e) => {
                    app.status_message = format!("edit error: {e}");
                }
            }
        }
        SendRequest::RemoteDelete { recipient, is_group, target_timestamp } => {
            if let Err(e) = signal_client.send_remote_delete(&recipient, is_group, target_timestamp).await {
                app.status_message = format!("delete error: {e}");
            }
        }
        SendRequest::Typing { recipient, is_group, stop } => {
            let _ = signal_client.send_typing(&recipient, is_group, stop).await;
        }
        SendRequest::ReadReceipt { recipient, timestamps } => {
            if let Err(e) = signal_client.send_read_receipt(&recipient, &timestamps).await {
                crate::debug_log::logf(format_args!("read receipt error: {e}"));
            }
        }
        SendRequest::UpdateExpiration { conv_id, is_group, seconds } => {
            let result = if is_group {
                signal_client.send_update_group_expiration(&conv_id, seconds).await
            } else {
                signal_client.send_update_contact_expiration(&conv_id, seconds).await
            };
            if let Err(e) = result {
                app.status_message = format!("expiration error: {e}");
            } else if seconds == 0 {
                app.status_message = "Disappearing messages disabled".to_string();
            } else {
                app.status_message = format!(
                    "Disappearing messages set to {}",
                    input::format_compact_duration(seconds),
                );
            }
        }
        SendRequest::CreateGroup { name } => {
            if let Err(e) = signal_client.create_group(&name, &[]).await {
                app.status_message = format!("create group error: {e}");
            } else {
                app.status_message = format!("Created group \"{}\"", name);
                let _ = signal_client.list_groups().await;
            }
        }
        SendRequest::AddGroupMembers { group_id, members } => {
            if let Err(e) = signal_client.add_group_members(&group_id, &members).await {
                app.status_message = format!("add member error: {e}");
            } else {
                let names: Vec<String> = members.iter()
                    .map(|m| app.contact_names.get(m).cloned().unwrap_or_else(|| m.clone()))
                    .collect();
                app.status_message = format!("Added {}", names.join(", "));
                let _ = signal_client.list_groups().await;
            }
        }
        SendRequest::RemoveGroupMembers { group_id, members } => {
            if let Err(e) = signal_client.remove_group_members(&group_id, &members).await {
                app.status_message = format!("remove member error: {e}");
            } else {
                let names: Vec<String> = members.iter()
                    .map(|m| app.contact_names.get(m).cloned().unwrap_or_else(|| m.clone()))
                    .collect();
                app.status_message = format!("Removed {}", names.join(", "));
                let _ = signal_client.list_groups().await;
            }
        }
        SendRequest::RenameGroup { group_id, name } => {
            if let Err(e) = signal_client.rename_group(&group_id, &name).await {
                app.status_message = format!("rename group error: {e}");
            } else {
                // Update locally for instant visual feedback
                if let Some(conv) = app.conversations.get_mut(&group_id) {
                    conv.name = name.clone();
                }
                app.contact_names.insert(group_id.clone(), name.clone());
                app.status_message = format!("Renamed group to \"{}\"", name);
                let _ = signal_client.list_groups().await;
            }
        }
        SendRequest::LeaveGroup { group_id } => {
            if let Err(e) = signal_client.quit_group(&group_id).await {
                app.status_message = format!("leave group error: {e}");
            } else {
                let name = app.conversations.get(&group_id)
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| group_id.clone());
                app.conversations.remove(&group_id);
                app.conversation_order.retain(|id| id != &group_id);
                app.groups.remove(&group_id);
                if app.active_conversation.as_ref() == Some(&group_id) {
                    app.active_conversation = None;
                }
                app.status_message = format!("Left group \"{}\"", name);
            }
        }
        SendRequest::Block { recipient, is_group } => {
            if let Err(e) = signal_client.block_contact(&recipient, is_group).await {
                app.status_message = format!("block error: {e}");
            }
        }
        SendRequest::Unblock { recipient, is_group } => {
            if let Err(e) = signal_client.unblock_contact(&recipient, is_group).await {
                app.status_message = format!("unblock error: {e}");
            }
        }
        SendRequest::MessageRequestResponse { recipient, is_group, response_type } => {
            if let Err(e) = signal_client.send_message_request_response(&recipient, is_group, &response_type).await {
                app.status_message = format!("message request error: {e}");
            } else {
                app.status_message = match response_type.as_str() {
                    "accept" => "Message request accepted".to_string(),
                    "delete" => "Message request deleted".to_string(),
                    _ => String::new(),
                };
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
    app.desktop_notifications = config.desktop_notifications;
    app.inline_images = config.inline_images;
    app.native_images = config.native_images;
    app.incognito = incognito;
    app.show_receipts = config.show_receipts;
    app.color_receipts = config.color_receipts;
    app.nerd_fonts = config.nerd_fonts;
    app.reaction_verbose = config.reaction_verbose;
    app.send_read_receipts = config.send_read_receipts;
    app.mouse_enabled = config.mouse_enabled;
    app.available_themes = theme::all_themes();
    app.theme = theme::find_theme(&config.theme);
    app.load_from_db()?;
    app.set_connected();

    // Purge messages that expired while the app was closed
    app.sweep_expired_messages();

    // Ask primary device to sync contacts/groups, then fetch them (best-effort)
    let _ = signal_client.send_sync_request().await;
    let _ = signal_client.list_contacts().await;
    let _ = signal_client.list_groups().await;

    let mut last_expiry_sweep = Instant::now();

    loop {
        // Force full redraw when active conversation changes (clears native image artifacts)
        if app.native_images && app.active_conversation != app.prev_active_conversation {
            app.prev_active_conversation = app.active_conversation.clone();
            terminal.clear()?;
        }

        // Render
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        emit_osc8_links(terminal.backend_mut(), &app.link_regions, app.theme.link)?;
        if app.native_images {
            emit_native_images(terminal.backend_mut(), &mut app)?;
        }

        // Poll for events with a short timeout so we stay responsive to signal events
        let has_terminal_event = event::poll(POLL_TIMEOUT)?;

        if has_terminal_event {
            match event::read()? {
                Event::Key(key) => {
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
                Event::Mouse(mouse) => {
                    if let Some(req) = app.handle_mouse_event(mouse) {
                        dispatch_send(signal_client, &mut app, req).await;
                    }
                }
                _ => {}
            }
        }

        // Drain signal events (non-blocking), detect disconnect
        loop {
            match signal_client.event_rx.try_recv() {
                Ok(ev) => app.handle_signal_event(ev),
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    if app.connection_error.is_none() {
                        let stderr = signal_client.stderr_output();
                        let exit_info = signal_client.try_child_exit();
                        let msg = if let Some(last_line) = stderr.lines().last().filter(|l| !l.is_empty()) {
                            format!("signal-cli: {last_line}")
                        } else if let Some(code) = exit_info {
                            match code {
                                Some(c) => format!("signal-cli exited with code {c}"),
                                None => "signal-cli killed by signal".to_string(),
                            }
                        } else {
                            "signal-cli disconnected".to_string()
                        };
                        debug_log::logf(format_args!("disconnect: {msg}"));
                        app.connection_error = Some(msg);
                        app.connected = false;
                    }
                    break;
                }
                Err(_) => break, // Empty, no more events
            }
        }

        // Dispatch queued read receipts
        for (recipient, timestamps) in std::mem::take(&mut app.pending_read_receipts) {
            dispatch_send(
                signal_client,
                &mut app,
                SendRequest::ReadReceipt { recipient, timestamps },
            )
            .await;
        }

        // Expire stale typing indicators
        app.cleanup_typing();

        // Check if our outgoing typing indicator has timed out
        if let Some(typing_stop) = app.check_typing_timeout() {
            dispatch_send(signal_client, &mut app, typing_stop).await;
        }
        // Drain pending typing stop from conversation switches
        if let Some(typing_stop) = app.pending_typing_stop.take() {
            dispatch_send(signal_client, &mut app, typing_stop).await;
        }

        // Periodic sweep of expired disappearing messages (every 10s)
        if last_expiry_sweep.elapsed() >= Duration::from_secs(10) {
            app.sweep_expired_messages();
            last_expiry_sweep = Instant::now();
        }

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
    app.is_demo = true;
    app.connected = true;
    app.loading = false;
    app.status_message = "connected | demo mode".to_string();

    populate_demo_data(&mut app);

    loop {
        if app.native_images && app.active_conversation != app.prev_active_conversation {
            app.prev_active_conversation = app.active_conversation.clone();
            terminal.clear()?;
        }

        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        emit_osc8_links(terminal.backend_mut(), &app.link_regions, app.theme.link)?;
        if app.native_images {
            emit_native_images(terminal.backend_mut(), &mut app)?;
        }

        let has_terminal_event = event::poll(POLL_TIMEOUT)?;

        if has_terminal_event {
            match event::read()? {
                Event::Key(key) => {
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
                Event::Mouse(mouse) => {
                    app.handle_mouse_event(mouse);
                }
                _ => {}
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
            mention_ranges: Vec::new(),
            style_ranges: Vec::new(),
            quote: None,
            is_edited: false,
            is_deleted: false,
            sender_id: String::new(),
            expires_in_seconds: 0,
            expiration_start_ms: 0,
        }
    };

    // --- Alice: weekend plans ---
    let alice_id = "+15550001111".to_string();
    let alice = Conversation {
        name: "Alice".to_string(),
        id: alice_id.clone(),
        messages: vec![
            dm("Alice", ts(8, 0), "Good morning! How's your day going?"),
            dm("you", ts(8, 5), "Just getting started, coffee in hand"),
            dm("Alice", ts(8, 10), "Nice! I've been up since 6, went for a run"),
            dm("you", ts(8, 15), "Impressive. I can barely get out of bed before 7"),
            dm("Alice", ts(8, 20), "Ha! It gets easier once you build the habit"),
            dm("you", ts(8, 25), "That's what everyone says..."),
            dm("Alice", ts(8, 30), "Trust me, after a week it becomes automatic"),
            dm("you", ts(8, 35), "Maybe I'll try next week"),
            dm("Alice", ts(8, 40), "I'll hold you to that!"),
            dm("you", ts(8, 45), "Please don't"),
            dm("Alice", ts(9, 0), "Too late, setting a reminder now"),
            dm("you", ts(9, 5), "Oh no..."),
            dm("Alice", ts(9, 10), "Anyway, are you free this weekend?"),
            dm("you", ts(9, 12), "Yeah, what did you have in mind?"),
            dm("Alice", ts(9, 15), "There's a farmers market Saturday morning"),
            dm("you", ts(9, 17), "Sounds great, what time?"),
            dm("Alice", ts(9, 18), "Opens at 8, but 9 is fine. Less crowded."),
            dm("you", ts(9, 20), "Perfect, let's do 9"),
            dm("Alice", ts(9, 22), "Great! I'll pick you up at 8:45"),
            dm("you", ts(9, 23), "Sounds like a plan"),
            dm("Alice", ts(9, 25), "We should also check out that new bakery nearby"),
            dm("you", ts(9, 27), "Oh yeah, I heard their sourdough is amazing"),
            dm("Alice", ts(9, 30), "And they have those fancy croissants too"),
            dm("you", ts(9, 32), "You had me at croissants"),
            dm("Alice", ts(9, 35), "Perfect, it's a date then! See you Saturday"),
        ],
        unread: 0,
        is_group: false,
        expiration_timer: 0,
        accepted: true,
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
        expiration_timer: 0,
        accepted: true,
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
        expiration_timer: 0,
        accepted: true,
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
        expiration_timer: 0,
        accepted: true,
    };

    // --- #Rust Devs: group technical discussion with @mentions ---
    let rust_id = "group_rustdevs".to_string();
    let mut alice_mention = dm("Alice", ts(10, 42), "Can you share the link? @Bob might want it too");
    // "@Bob" starts at byte 24, ends at byte 28
    alice_mention.mention_ranges = vec![(24, 28)];
    let rust_group = Conversation {
        name: "#Rust Devs".to_string(),
        id: rust_id.clone(),
        messages: vec![
            dm("Alice", ts(10, 30), "Has anyone tried the new async trait syntax?"),
            dm("Bob", ts(10, 32), "Yeah, it's so much cleaner than the pin-based approach"),
            dm("Dave", ts(10, 35), "I'm still wrapping my head around it"),
            dm("you", ts(10, 40), "The desugaring docs helped me a lot"),
            alice_mention,
            dm("you", ts(10, 43), "Sure, one sec"),
        ],
        unread: 0,
        is_group: true,
        expiration_timer: 0,
        accepted: true,
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
        expiration_timer: 0,
        accepted: true,
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

    // Populate contact names and UUID maps for @mention autocomplete
    let mom_id = "+15550005555".to_string();
    let dad_id = "+15550006666".to_string();
    let demo_contacts: Vec<(&str, &str, &str)> = vec![
        (&alice_id, "Alice", "aaaa-alice-uuid"),
        (&bob_id, "Bob", "bbbb-bob-uuid"),
        (&carol_id, "Carol", "cccc-carol-uuid"),
        (&dave_id, "Dave", "dddd-dave-uuid"),
        (&mom_id, "Mom", "eeee-mom-uuid"),
        (&dad_id, "Dad", "ffff-dad-uuid"),
    ];
    for (phone, name, uuid) in &demo_contacts {
        app.contact_names.insert(phone.to_string(), name.to_string());
        app.uuid_to_name.insert(uuid.to_string(), name.to_string());
        app.number_to_uuid.insert(phone.to_string(), uuid.to_string());
    }

    // Populate groups with correct members
    use crate::signal::types::Group;
    app.groups.insert(
        rust_id.clone(),
        Group {
            id: rust_id,
            name: "#Rust Devs".to_string(),
            members: vec![alice_id.clone(), bob_id.clone(), dave_id.clone()],
            member_uuids: vec![],
        },
    );
    app.groups.insert(
        family_id.clone(),
        Group {
            id: family_id,
            name: "#Family".to_string(),
            members: vec![mom_id, dad_id],
            member_uuids: vec![],
        },
    );

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
