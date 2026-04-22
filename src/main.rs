//! Binary entry point and event loop.
//!
//! Polls the keyboard at 50 ms, drains [`signal::types::SignalEvent`]s into
//! the [`app::App`] state, and renders each frame via [`ui::draw`].
//! Orchestrates the first-run flow: setup wizard -> device linking -> app
//! startup.

mod app;
mod autocomplete;
mod config;
mod conversation_store;
mod db;
mod debug_log;
mod domain;
mod image_render;
mod input;
mod keybindings;
mod link;
mod list_overlay;
mod mute;
mod settings_profile;
mod setup;
mod signal;
mod theme;
mod ui;

use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, RestorePosition, SavePosition, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyEventKind,
    },
    execute, queue,
    style::{Print, ResetColor, SetForegroundColor},
    terminal::{
        BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
        disable_raw_mode, enable_raw_mode,
    },
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Flex, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
};

use app::{App, InputMode, SendRequest};
use config::Config;
use setup::SetupResult;
use signal::client::SignalClient;

/// Keyboard polling interval for the main event loop.
const POLL_TIMEOUT: Duration = Duration::from_millis(50);

/// Set restrictive permissions (0600) on a sensitive file (Unix only).
#[cfg(unix)]
fn set_file_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_file_permissions(_path: &std::path::Path) {}

/// Set restrictive permissions (0700) on a sensitive directory (Unix only).
#[cfg(unix)]
fn set_dir_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn set_dir_permissions(_path: &std::path::Path) {}

#[tokio::main]
async fn main() -> Result<()> {
    // Disable the default Windows Ctrl+C handler — crossterm captures it as a
    // key event in raw mode, so the OS handler just causes a noisy exit code.
    #[cfg(windows)]
    unsafe {
        unsafe extern "system" {
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
    let mut debug_full = false;

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
            "--debug-full" => {
                debug_full = true;
                i += 1;
            }
            "--help" => {
                eprintln!("siggy - Terminal Signal client");
                eprintln!();
                eprintln!("Usage: siggy [OPTIONS]");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  -a, --account <NUMBER>  Phone number (E.164 format)");
                eprintln!("  -c, --config <PATH>     Config file path");
                eprintln!("      --setup             Run first-time setup wizard");
                eprintln!(
                    "      --demo              Launch with dummy data (no signal-cli needed)"
                );
                eprintln!("      --incognito         No local message storage (in-memory only)");
                eprintln!("      --debug             Write debug log (PII redacted)");
                eprintln!("      --debug-full        Write debug log (full, unredacted)");
                eprintln!("      --help              Show this help");
                std::process::exit(0);
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                std::process::exit(1);
            }
        }
    }

    if debug_full {
        debug_log::enable_full();
        debug_log::log("=== siggy debug session started (full/unredacted) ===");
    } else if debug {
        debug_log::enable();
        debug_log::log("=== siggy debug session started (PII redacted) ===");
    }

    // Load config
    let mut config = Config::load(config_path)?;
    if let Some(acct) = account {
        config.account = acct;
    }

    // Set up terminal BEFORE anything else so all errors render in the TUI
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if config.mouse_enabled {
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        )?;
    } else {
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    }
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run the main flow inside a closure so we can always restore the terminal
    let result = run_main_flow(
        &mut terminal,
        &mut config,
        force_setup,
        demo_mode,
        incognito,
    )
    .await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
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
        let demo_config = Config {
            account: "+15551234567".to_string(),
            ..Config::default()
        };
        return run_app(
            terminal,
            MessagingBackend::Demo,
            &demo_config,
            database,
            false,
        )
        .await;
    }

    // Run setup wizard if needed
    let mut setup_handled_linking = false;
    if config.needs_setup() || force_setup {
        match setup::run_setup(terminal, config, force_setup).await? {
            SetupResult::Completed(new_config) => {
                *config = *new_config;
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
        set_dir_permissions(&config.download_dir);
    }

    // Open database (in-memory for incognito mode)
    let database = if incognito {
        db::Database::open_in_memory()?
    } else {
        let db_dir = dirs::data_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("siggy");

        // Auto-migrate from old "signal-tui" data directory
        if !db_dir.exists() {
            let old_db_dir = dirs::data_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("signal-tui");
            if old_db_dir.exists() {
                let _ = std::fs::rename(&old_db_dir, &db_dir);
            }
        }

        std::fs::create_dir_all(&db_dir)?;
        set_dir_permissions(&db_dir);
        let db_path = db_dir.join("siggy.db");

        // Auto-migrate old database filename
        if !db_path.exists() {
            let old_db_path = db_dir.join("signal-tui.db");
            if old_db_path.exists() {
                let _ = std::fs::rename(&old_db_path, &db_path);
            }
        }
        set_file_permissions(&db_path);
        db::Database::open(&db_path)?
    };

    // In incognito mode, redirect attachments to a temp directory
    let incognito_tmp_dir = if incognito {
        let tmp = std::env::temp_dir().join(format!("siggy-incognito-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        set_dir_permissions(&tmp);
        config.download_dir = tmp.clone();
        Some(tmp)
    } else {
        None
    };

    // Spawn signal-cli backend directly (skip the old pre-flight check that spawned
    // a throwaway JVM process). If the account isn't registered, signal-cli will exit
    // quickly and we fall back to the linking flow.
    let mut signal_client = match SignalClient::spawn(config).await {
        Ok(client) => client,
        Err(e) => {
            let msg = format!("{e}");
            show_error_screen(terminal, "Failed to Start signal-cli", &msg).await?;
            return Ok(());
        }
    };

    // Give signal-cli a brief window to fail if the account is unregistered.
    // If the process exits early, check stderr and fall back to linking.
    if !setup_handled_linking
        && !signal_client
            .wait_for_ready(std::time::Duration::from_millis(500))
            .await
    {
        let stderr = signal_client.stderr_output();
        debug_log::logf(format_args!(
            "signal-cli exited early during startup, stderr: {stderr}"
        ));
        signal_client.shutdown().await?;

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

        // Re-spawn after successful linking
        signal_client = match SignalClient::spawn(config).await {
            Ok(client) => client,
            Err(e) => {
                let msg = format!("{e}");
                show_error_screen(terminal, "Failed to Start signal-cli", &msg).await?;
                return Ok(());
            }
        };
    }

    // Run the app
    let result = run_app(
        terminal,
        MessagingBackend::Signal(&mut signal_client),
        config,
        database,
        incognito,
    )
    .await;

    // Shut down signal-cli
    signal_client.shutdown().await?;

    // Clean up incognito temp directory
    if let Some(ref tmp) = incognito_tmp_dir {
        let _ = std::fs::remove_dir_all(tmp);
    }

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

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            return Ok(());
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
        // Sanitize URL: strip control characters to prevent terminal escape injection
        let safe_url: String = link.url.chars().filter(|c| !c.is_control()).collect();
        queue!(
            backend,
            Print(format!("\x1b]8;;{}\x07{}\x1b]8;;\x07", safe_url, link.text))
        )?;
        queue!(backend, ResetColor)?;
    }
    queue!(backend, RestorePosition)?;
    backend.flush()?;
    Ok(())
}

/// Look up or encode and cache a PNG for the given image path and cell dimensions.
/// Returns the base64-encoded PNG data, or `None` if the image can't be loaded.
fn get_or_cache_png(
    cache: &mut HashMap<String, (String, u32, u32)>,
    path: &str,
    cell_cols: u32,
    cell_rows: u32,
) -> Option<String> {
    if let Some(cached) = cache.get(path) {
        return Some(cached.0.clone());
    }
    let data = image_render::encode_native_png(std::path::Path::new(path), cell_cols, cell_rows)?;
    let b64 = data.0.clone();
    cache.insert(path.to_string(), data);
    Some(b64)
}

/// Write native terminal image protocol escape sequences.
///
/// For Kitty: process `kitty_pending_transmits` — transmit image data and create
/// virtual placements. The actual display uses Unicode Placeholder cells embedded
/// in the ratatui buffer by `render_placeholder()`.
///
/// For iTerm2: overlay pre-resized images on top of the halfblock placeholders
/// using cursor-positioned inline image sequences.
fn emit_native_images(backend: &mut CrosstermBackend<io::Stdout>, app: &mut App) -> Result<()> {
    let protocol = app.image.image_protocol;
    if protocol == image_render::ImageProtocol::Halfblock {
        return Ok(());
    }

    use std::io::Write;

    if protocol == image_render::ImageProtocol::Kitty {
        // Kitty Unicode Placeholders: transmit pending images and create virtual placements.
        // The placeholder cells (U+10EEEE) are already in the ratatui buffer.
        let pending = std::mem::take(&mut app.image.kitty_pending_transmits);
        if pending.is_empty() {
            return Ok(());
        }

        for (id, path, cols, rows) in &pending {
            let b64 = match get_or_cache_png(
                &mut app.image.native_image_cache,
                path,
                *cols as u32,
                *rows as u32,
            ) {
                Some(b) => b,
                None => continue,
            };

            // Transmit image data (a=t = transmit only, no display)
            let chunks: Vec<&[u8]> = b64.as_bytes().chunks(4096).collect();
            for (i, chunk) in chunks.iter().enumerate() {
                let m = if i == chunks.len() - 1 { 0 } else { 1 };
                let chunk_str = std::str::from_utf8(chunk).unwrap_or("");
                if i == 0 {
                    write!(
                        backend,
                        "\x1b_Gf=100,a=t,i={id},q=2,m={m};{chunk_str}\x1b\\",
                    )?;
                } else {
                    write!(backend, "\x1b_Gm={m};{chunk_str}\x1b\\")?;
                }
            }

            // Create virtual placement (U=1 enables Unicode Placeholder mode)
            write!(backend, "\x1b_Ga=p,U=1,i={id},c={cols},r={rows},q=2\x1b\\",)?;

            app.image.kitty_transmitted.insert(*id);
        }

        backend.flush()?;
        return Ok(());
    }

    // Sixel: slice the cached full Sixel to the visible region (instant string op).
    if protocol == image_render::ImageProtocol::Sixel {
        if app.has_overlay() || app.image.visible_images.is_empty() {
            app.image.visible_images.clear();
            return Ok(());
        }

        // Dedup: skip Sixel emit when images haven't moved (avoids cursor
        // flash from large Sixel writes on every mouse-move/redraw).
        if app.image.visible_images == app.image.prev_visible_images {
            app.image.visible_images.clear();
            return Ok(());
        }

        let images = std::mem::take(&mut app.image.visible_images);
        queue!(backend, Hide, SavePosition)?;

        for img in &images {
            if let Some(full_sixel) = app.image.sixel_cache.get(&img.path) {
                let sliced = image_render::slice_sixel_bands(
                    full_sixel,
                    app.image.cell_px.1,
                    img.full_height,
                    img.crop_top,
                    img.height,
                );
                if let Some(sixel) = sliced {
                    queue!(backend, MoveTo(img.x, img.y))?;
                    write!(backend, "{sixel}")?;
                }
            }
        }

        queue!(backend, RestorePosition, Show)?;
        app.image.prev_visible_images = images;
        return Ok(());
    }

    // iTerm2: only re-render when images change (dedup avoids flicker).
    if app.image.visible_images == app.image.prev_visible_images {
        return Ok(());
    }

    if app.image.visible_images.is_empty() {
        app.image.prev_visible_images.clear();
        return Ok(());
    }

    let images = std::mem::take(&mut app.image.visible_images);

    queue!(backend, SavePosition)?;

    for img in &images {
        let b64 = match get_or_cache_png(
            &mut app.image.native_image_cache,
            &img.path,
            img.width as u32,
            img.full_height as u32,
        ) {
            Some(b) => b,
            None => continue,
        };

        // Crop when partially scrolled out of view, with caching to avoid
        // re-encoding every frame (which causes flicker in iTerm2).
        let b64 = if img.crop_top > 0 || img.height < img.full_height {
            let crop_key = (img.path.clone(), img.crop_top, img.height);
            if let Some(cached) = app.image.iterm2_crop_cache.get(&crop_key) {
                cached.clone()
            } else {
                let px_h = app
                    .image
                    .native_image_cache
                    .get(&img.path)
                    .map(|c| c.2)
                    .unwrap_or(0);
                let cropped = image_render::crop_png_vertical(
                    &b64,
                    px_h,
                    img.full_height,
                    img.crop_top,
                    img.height,
                )
                .unwrap_or(b64);
                app.image
                    .iterm2_crop_cache
                    .insert(crop_key, cropped.clone());
                cropped
            }
        } else {
            b64
        };

        queue!(backend, MoveTo(img.x, img.y))?;

        write!(
            backend,
            "\x1b]1337;File=inline=1;width={};height={};preserveAspectRatio=0:{b64}\x07",
            img.width, img.height
        )?;
    }

    queue!(backend, RestorePosition)?;
    // No flush - let EndSynchronizedUpdate send everything atomically.

    app.image.prev_visible_images = images;
    Ok(())
}

/// Dispatch a SendRequest to signal-cli.
async fn dispatch_send(signal_client: &mut SignalClient, app: &mut App, req: SendRequest) {
    match req {
        SendRequest::Message {
            recipient,
            body,
            is_group,
            local_ts_ms,
            mentions,
            attachment,
            quote_timestamp,
            quote_author,
            quote_body,
        } => {
            let attachments: Vec<std::path::PathBuf> = attachment.into_iter().collect();
            let quote = match (quote_author, quote_timestamp, quote_body) {
                (Some(author), Some(ts), Some(body_text)) => Some((author, ts, body_text)),
                _ => None,
            };
            let att_refs: Vec<&std::path::Path> = attachments.iter().map(|p| p.as_path()).collect();
            match signal_client
                .send_message(
                    &recipient,
                    &body,
                    is_group,
                    &mentions,
                    &att_refs,
                    quote.as_ref().map(|(a, t, b)| (a.as_str(), *t, b.as_str())),
                )
                .await
            {
                Ok(rpc_id) => {
                    debug_log::logf(format_args!(
                        "send: to={} ts={local_ts_ms}",
                        debug_log::mask_phone(&recipient)
                    ));
                    app.pending
                        .sends
                        .insert(rpc_id.clone(), (recipient.to_string(), local_ts_ms));
                    // Register any paste temp file for deferred deletion. The actual delete is
                    // triggered after send confirmation; this sentinel keeps it alive until then.
                    // Only one paste attachment per send is expected; break after the first match.
                    for path in &attachments {
                        if path.starts_with(&app.paste_temp_path) {
                            let sentinel = Instant::now()
                                + Duration::from_secs(app::PASTE_CLEANUP_SENTINEL_SECS);
                            app.pending_paste_cleanups
                                .insert(rpc_id.clone(), (path.clone(), sentinel));
                            break;
                        }
                    }
                }
                Err(e) => {
                    app.status_message = format!("send error: {e}");
                    // RPC failed to send — delete temp file immediately (signal-cli never saw it)
                    for path in &attachments {
                        if path.starts_with(&app.paste_temp_path) {
                            let _ = std::fs::remove_file(path);
                        }
                    }
                }
            }
        }
        SendRequest::Reaction {
            conv_id,
            emoji,
            is_group,
            target_author,
            target_timestamp,
            remove,
        } => {
            if let Err(e) = signal_client
                .send_reaction(
                    &conv_id,
                    is_group,
                    &emoji,
                    &target_author,
                    target_timestamp,
                    remove,
                )
                .await
            {
                app.status_message = format!("reaction error: {e}");
            }
        }
        SendRequest::Edit {
            recipient,
            body,
            is_group,
            edit_timestamp,
            local_ts_ms,
            mentions,
            quote_timestamp,
            quote_author,
            quote_body,
        } => {
            let quote = match (quote_author, quote_timestamp, quote_body) {
                (Some(author), Some(ts), Some(body_text)) => Some((author, ts, body_text)),
                _ => None,
            };
            match signal_client
                .send_edit_message(
                    &recipient,
                    &body,
                    is_group,
                    edit_timestamp,
                    &mentions,
                    quote.as_ref().map(|(a, t, b)| (a.as_str(), *t, b.as_str())),
                )
                .await
            {
                Ok(rpc_id) => {
                    debug_log::logf(format_args!(
                        "edit: to={} ts={edit_timestamp}",
                        debug_log::mask_phone(&recipient)
                    ));
                    app.pending
                        .sends
                        .insert(rpc_id, (recipient.to_string(), local_ts_ms));
                }
                Err(e) => {
                    app.status_message = format!("edit error: {e}");
                }
            }
        }
        SendRequest::RemoteDelete {
            recipient,
            is_group,
            target_timestamp,
        } => {
            if let Err(e) = signal_client
                .send_remote_delete(&recipient, is_group, target_timestamp)
                .await
            {
                app.status_message = format!("delete error: {e}");
            }
        }
        SendRequest::Typing {
            recipient,
            is_group,
            stop,
        } => {
            let _ = signal_client.send_typing(&recipient, is_group, stop).await;
        }
        SendRequest::ReadReceipt {
            recipient,
            timestamps,
        } => {
            if let Err(e) = signal_client
                .send_read_receipt(&recipient, &timestamps)
                .await
            {
                debug_log::logf(format_args!("read receipt error: {e}"));
            }
        }
        SendRequest::UpdateExpiration {
            conv_id,
            is_group,
            seconds,
        } => {
            let result = if is_group {
                signal_client
                    .send_update_group_expiration(&conv_id, seconds)
                    .await
            } else {
                signal_client
                    .send_update_contact_expiration(&conv_id, seconds)
                    .await
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
        SendRequest::CreateGroup { name } => match signal_client.create_group(&name, &[]).await {
            Err(e) => {
                app.status_message = format!("create group error: {e}");
            }
            _ => {
                app.status_message = format!("Created group \"{}\"", name);
                let _ = signal_client.list_groups().await;
            }
        },
        SendRequest::AddGroupMembers { group_id, members } => {
            match signal_client.add_group_members(&group_id, &members).await {
                Err(e) => {
                    app.status_message = format!("add member error: {e}");
                }
                _ => {
                    let names: Vec<String> = members
                        .iter()
                        .map(|m| {
                            app.store
                                .contact_names
                                .get(m)
                                .cloned()
                                .unwrap_or_else(|| m.clone())
                        })
                        .collect();
                    app.status_message = format!("Added {}", names.join(", "));
                    let _ = signal_client.list_groups().await;
                }
            }
        }
        SendRequest::RemoveGroupMembers { group_id, members } => {
            match signal_client
                .remove_group_members(&group_id, &members)
                .await
            {
                Err(e) => {
                    app.status_message = format!("remove member error: {e}");
                }
                _ => {
                    let names: Vec<String> = members
                        .iter()
                        .map(|m| {
                            app.store
                                .contact_names
                                .get(m)
                                .cloned()
                                .unwrap_or_else(|| m.clone())
                        })
                        .collect();
                    app.status_message = format!("Removed {}", names.join(", "));
                    let _ = signal_client.list_groups().await;
                }
            }
        }
        SendRequest::RenameGroup { group_id, name } => {
            match signal_client.rename_group(&group_id, &name).await {
                Err(e) => {
                    app.status_message = format!("rename group error: {e}");
                }
                _ => {
                    // Update locally for instant visual feedback
                    if let Some(conv) = app.store.conversations.get_mut(&group_id) {
                        conv.name = name.clone();
                    }
                    app.store
                        .contact_names
                        .insert(group_id.clone(), name.clone());
                    app.status_message = format!("Renamed group to \"{}\"", name);
                    let _ = signal_client.list_groups().await;
                }
            }
        }
        SendRequest::LeaveGroup { group_id } => match signal_client.quit_group(&group_id).await {
            Err(e) => {
                app.status_message = format!("leave group error: {e}");
            }
            _ => {
                let name = app
                    .store
                    .conversations
                    .get(&group_id)
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| group_id.clone());
                app.store.conversations.remove(&group_id);
                app.store.conversation_order.retain(|id| id != &group_id);
                app.store.groups.remove(&group_id);
                if app.active_conversation.as_ref() == Some(&group_id) {
                    app.active_conversation = None;
                }
                app.status_message = format!("Left group \"{}\"", name);
            }
        },
        SendRequest::Block {
            recipient,
            is_group,
        } => {
            if let Err(e) = signal_client.block_contact(&recipient, is_group).await {
                app.status_message = format!("block error: {e}");
            }
        }
        SendRequest::Unblock {
            recipient,
            is_group,
        } => {
            if let Err(e) = signal_client.unblock_contact(&recipient, is_group).await {
                app.status_message = format!("unblock error: {e}");
            }
        }
        SendRequest::Pin {
            recipient,
            is_group,
            target_author,
            target_timestamp,
            pin_duration,
        } => {
            if let Err(e) = signal_client
                .send_pin_message(
                    &recipient,
                    is_group,
                    &target_author,
                    target_timestamp,
                    pin_duration,
                )
                .await
            {
                app.status_message = format!("pin error: {e}");
            }
        }
        SendRequest::Unpin {
            recipient,
            is_group,
            target_author,
            target_timestamp,
        } => {
            if let Err(e) = signal_client
                .send_unpin_message(&recipient, is_group, &target_author, target_timestamp)
                .await
            {
                app.status_message = format!("unpin error: {e}");
            }
        }
        SendRequest::PollCreate {
            recipient,
            is_group,
            question,
            options,
            allow_multiple,
            local_ts_ms,
        } => {
            match signal_client
                .send_poll_create(&recipient, is_group, &question, &options, allow_multiple)
                .await
            {
                Ok(rpc_id) => {
                    app.pending.sends.insert(rpc_id, (recipient, local_ts_ms));
                }
                Err(e) => {
                    app.status_message = format!("poll error: {e}");
                }
            }
        }
        SendRequest::PollVote {
            recipient,
            is_group,
            poll_author,
            poll_timestamp,
            option_indexes,
            vote_count,
        } => {
            if let Err(e) = signal_client
                .send_poll_vote(
                    &recipient,
                    is_group,
                    &poll_author,
                    poll_timestamp,
                    &option_indexes,
                    vote_count,
                )
                .await
            {
                app.status_message = format!("vote error: {e}");
            }
        }
        SendRequest::PollTerminate {
            recipient,
            is_group,
            poll_timestamp,
        } => {
            if let Err(e) = signal_client
                .send_poll_terminate(&recipient, is_group, poll_timestamp)
                .await
            {
                app.status_message = format!("end poll error: {e}");
            }
        }
        SendRequest::MessageRequestResponse {
            recipient,
            is_group,
            response_type,
        } => {
            match signal_client
                .send_message_request_response(&recipient, is_group, &response_type)
                .await
            {
                Err(e) => {
                    app.status_message = format!("message request error: {e}");
                }
                _ => {
                    app.status_message = match response_type.as_str() {
                        "accept" => "Message request accepted".to_string(),
                        "delete" => "Message request deleted".to_string(),
                        _ => String::new(),
                    };
                }
            }
        }
        SendRequest::ListIdentities => {
            let _ = signal_client.list_identities().await;
        }
        SendRequest::TrustIdentity {
            recipient,
            safety_number,
        } => {
            match signal_client
                .trust_identity(&recipient, &safety_number)
                .await
            {
                Err(e) => {
                    app.status_message = format!("trust error: {e}");
                }
                _ => {
                    app.status_message = format!(
                        "Verified {}",
                        app.store
                            .contact_names
                            .get(&recipient)
                            .unwrap_or(&recipient)
                    );
                    // Re-fetch identities to update trust levels
                    let _ = signal_client.list_identities().await;
                }
            }
        }
        SendRequest::UpdateProfile {
            given_name,
            family_name,
            about,
            about_emoji,
        } => {
            match signal_client
                .update_profile(&given_name, &family_name, &about, &about_emoji)
                .await
            {
                Err(e) => {
                    app.status_message = format!("profile error: {e}");
                }
                _ => {
                    app.status_message = "Profile updated".to_string();
                }
            }
        }
    }
}

enum MessagingBackend<'a> {
    Signal(&'a mut SignalClient),
    Demo,
}

impl MessagingBackend<'_> {
    async fn dispatch(&mut self, app: &mut App, req: SendRequest) {
        if let MessagingBackend::Signal(sc) = self {
            dispatch_send(sc, app, req).await;
        }
    }

    fn drain_events(&mut self, app: &mut App) -> bool {
        let MessagingBackend::Signal(sc) = self else {
            return false;
        };
        let mut changed = false;
        loop {
            match sc.event_rx.try_recv() {
                Ok(ev) => {
                    app.handle_signal_event(ev);
                    changed = true;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    if app.connection_error.is_none() {
                        let stderr = sc.stderr_output();
                        let exit_info = sc.try_child_exit();
                        let msg = if let Some(last_line) =
                            stderr.lines().last().filter(|l| !l.is_empty())
                        {
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
                Err(_) => break,
            }
        }
        changed
    }
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut backend: MessagingBackend<'_>,
    config: &Config,
    db: db::Database,
    incognito: bool,
) -> Result<()> {
    let mut app = App::new(config.account.clone(), db);
    app.notifications.notify_direct = config.notify_direct;
    app.notifications.notify_group = config.notify_group;
    app.notifications.desktop_notifications = config.desktop_notifications;
    app.notifications.notification_preview = config.notification_preview.clone();
    app.notifications.clipboard_clear_seconds = config.clipboard_clear_seconds;
    app.image.image_mode = config.image_mode.clone();
    app.image.show_link_previews = config.show_link_previews;
    app.incognito = incognito;
    app.date_separators = config.date_separators;
    app.show_receipts = config.show_receipts;
    app.color_receipts = config.color_receipts;
    app.nerd_fonts = config.nerd_fonts;
    app.reactions.emoji_to_text = config.emoji_to_text;
    app.reactions.show_reactions = config.show_reactions;
    app.reactions.verbose = config.reaction_verbose;
    app.send_read_receipts = config.send_read_receipts;
    app.mouse_enabled = config.mouse_enabled;
    app.sidebar_on_right = config.sidebar_on_right;
    app.sidebar_width = config.sidebar_width.clamp(14, 40);
    if config.cell_pixel_width > 0 && config.cell_pixel_height > 0 {
        app.image.cell_px = (config.cell_pixel_width, config.cell_pixel_height);
    }
    app.theme_picker.available_themes = theme::all_themes();
    app.theme = theme::find_theme(&config.theme);
    let mut kb = keybindings::find_profile(&config.keybinding_profile);
    let overrides = keybindings::load_overrides();
    kb.apply_overrides(&overrides);
    app.keybindings = kb;
    app.keybindings_overlay.available_profiles = keybindings::all_profile_names();
    app.settings_profiles.name = config.settings_profile.clone();
    app.settings_profiles.available = settings_profile::all_settings_profiles();
    app.load_from_db()?;
    app.expiring_msg_count = app
        .store
        .conversations
        .values()
        .flat_map(|c| &c.messages)
        .filter(|m| m.expires_in_seconds > 0)
        .count();
    if let MessagingBackend::Signal(sc) = &mut backend {
        app.set_connected();

        // Purge messages that expired while the app was closed
        app.sweep_expired_messages();

        // Ask primary device to sync contacts/groups, then fetch them (best-effort)
        app.startup_status = "Syncing with primary device...".to_string();
        let _ = sc.send_sync_request().await;
        app.startup_status = "Loading contacts...".to_string();
        let _ = sc.list_contacts().await;
        app.startup_status = "Loading groups...".to_string();
        let _ = sc.list_groups().await;
        app.startup_status = "Loading identities...".to_string();
        let _ = sc.list_identities().await;
    } else {
        app.is_demo = true;
        app.connected = true;
        app.loading = false;
        app.status_message = "connected | demo mode".to_string();
        app.populate_demo_data(chrono::Utc::now().date_naive());
    }

    let mut last_expiry_sweep = Instant::now();
    let mut last_sync_redraw = Instant::now();
    let mut needs_redraw = true;

    // Re-enable terminal modes — on Windows, spawning cmd.exe subprocesses
    // (signal-cli.bat, check_account_registered) can reset console input mode flags.
    if config.mouse_enabled {
        execute!(
            terminal.backend_mut(),
            EnableMouseCapture,
            EnableBracketedPaste
        )?;
    } else {
        execute!(terminal.backend_mut(), EnableBracketedPaste)?;
    }

    loop {
        // Only redraw when state has changed (avoids resetting cursor blink timer every 50ms)
        if needs_redraw {
            let native = app.image.image_mode == "native";
            let sixel_mode =
                native && app.image.image_protocol == image_render::ImageProtocol::Sixel;

            // Always start sync update for atomic rendering (prevents cursor flicker).
            queue!(terminal.backend_mut(), BeginSynchronizedUpdate)?;

            // Force full redraw when active conversation changes (clears native image artifacts)
            if native && app.active_conversation != app.prev_active_conversation {
                app.prev_active_conversation = app.active_conversation.clone();
                terminal.clear()?;
            }
            // Sixel: force full redraw when scroll changes so ratatui resends
            // ALL cells. The text output (inside sync) overwrites the terminal
            // buffer, then after EndSync our Sixel overlays at the new positions.
            // Without this, stale Sixel pixels persist at old image positions
            // because ratatui's diff only sends changed cells.
            if sixel_mode && app.scroll_offset != app.image.sixel_prev_scroll {
                app.image.sixel_prev_scroll = app.scroll_offset;
                app.image.prev_visible_images.clear();
                terminal.clear()?;
            }
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
            // Post-draw work that needs cursor hidden: OSC8 links use MoveTo,
            // and non-Sixel native images write escape sequences. Sixel emit
            // happens outside sync and handles its own cursor.
            let has_post_draw = !app.image.link_regions.is_empty() || (native && !sixel_mode);
            if has_post_draw && app.mode == InputMode::Insert {
                queue!(terminal.backend_mut(), Hide)?;
            }
            emit_osc8_links(
                terminal.backend_mut(),
                &app.image.link_regions,
                app.theme.link,
            )?;
            if native && !sixel_mode {
                emit_native_images(terminal.backend_mut(), &mut app)?;
            }
            if has_post_draw && app.mode == InputMode::Insert {
                queue!(terminal.backend_mut(), Show)?;
            }
            execute!(terminal.backend_mut(), EndSynchronizedUpdate)?;
            // Sixel: emit AFTER sync update ends. WT composites text ON TOP
            // of Sixel within sync, so text can't clear stale pixels. Outside
            // sync, the text from ratatui's diff has already been processed,
            // and our Sixel overlays cleanly on top. SavePosition/RestorePosition
            // in emit keeps cursor at the input bar (no Hide/Show needed).
            if sixel_mode {
                use std::io::Write;
                emit_native_images(terminal.backend_mut(), &mut app)?;
                terminal.backend_mut().flush()?;
            }
            needs_redraw = false;
        }

        // Background image rendering: drain completed renders and spawn new ones
        if app.ensure_active_images() {
            needs_redraw = true;
        }

        // Animate the loading spinner
        if app.loading {
            app.spinner_tick = app.spinner_tick.wrapping_add(1);
            needs_redraw = true;
        }

        // Load older messages when scrolled to the top
        if app.at_scroll_top {
            app.load_more_messages();
            needs_redraw = true;
        }

        // Poll for events with a short timeout so we stay responsive to signal events
        let has_terminal_event = event::poll(POLL_TIMEOUT)?;

        if has_terminal_event {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    needs_redraw = true;
                    // Keybinding capture mode intercepts ALL keys before anything else
                    if app.keybindings_overlay.capturing {
                        app.handle_keybinding_capture(key.modifiers, key.code);
                    } else if !app.handle_global_key(key.modifiers, key.code) {
                        let (overlay_handled, send_request) = app.handle_overlay_key(key.code);
                        if let Some(req) = send_request {
                            backend.dispatch(&mut app, req).await;
                        }
                        if !overlay_handled {
                            let send_request = match app.mode {
                                InputMode::Normal => app.handle_normal_key(key.modifiers, key.code),
                                InputMode::Insert => app.handle_insert_key(key.modifiers, key.code),
                            };
                            if let Some(req) = send_request {
                                backend.dispatch(&mut app, req).await;
                            }
                        }
                    }
                }
                Event::Mouse(mouse) => {
                    // Only redraw for clicks and scroll, not bare mouse moves
                    if !matches!(mouse.kind, event::MouseEventKind::Moved) {
                        needs_redraw = true;
                    }
                    if let Some(req) = app.handle_mouse_event(mouse) {
                        backend.dispatch(&mut app, req).await;
                    }
                }
                Event::Paste(text) => {
                    needs_redraw = true;
                    if let Some(req) = app.handle_paste(text) {
                        backend.dispatch(&mut app, req).await;
                    }
                }
                Event::Resize(..) => {
                    needs_redraw = true;
                    app.clear_kitty_state();
                }
                _ => {}
            }
        }

        // Drain signal events (non-blocking), detect disconnect
        if backend.drain_events(&mut app) {
            if app.sync.active {
                // During sync: throttle redraws to 500ms to keep UI responsive
                if last_sync_redraw.elapsed() >= std::time::Duration::from_millis(500) {
                    needs_redraw = true;
                    last_sync_redraw = Instant::now();
                }
            } else {
                needs_redraw = true;
            }
        }

        // Check if initial sync burst has ended
        if app.sync.active && app.sync.should_end() {
            app.end_sync();
            needs_redraw = true;
        }

        // Dispatch queued read receipts
        for (recipient, timestamps) in std::mem::take(&mut app.pending.read_receipts) {
            backend
                .dispatch(
                    &mut app,
                    SendRequest::ReadReceipt {
                        recipient,
                        timestamps,
                    },
                )
                .await;
        }

        // Expire stale typing indicators
        if app.typing.cleanup() {
            needs_redraw = true;
        }

        // Check if our outgoing typing indicator has timed out
        if let Some(typing_stop) = app.check_typing_timeout() {
            backend.dispatch(&mut app, typing_stop).await;
        }
        // Drain pending typing stop from conversation switches
        if let Some(typing_stop) = app.pending.typing_stop.take() {
            backend.dispatch(&mut app, typing_stop).await;
        }

        // Periodic sweep of expired disappearing messages and timed mutes (every 10s)
        if last_expiry_sweep.elapsed() >= Duration::from_secs(10) {
            app.sweep_expired_messages();
            app.sweep_expired_mutes();
            last_expiry_sweep = Instant::now();
            needs_redraw = true;
        }

        // Terminal bell on new messages in background conversations
        if app.notifications.pending_bell {
            app.notifications.pending_bell = false;
            execute!(terminal.backend_mut(), Print("\x07"))?;
        }

        // Auto-clear clipboard after timeout
        app.check_clipboard_clear();

        // Delete paste temp files that have passed their 10s post-send delay
        app.cleanup_paste_files();

        // Dynamic mouse capture toggle from settings
        if let Some(enabled) = app.pending_mouse_toggle.take() {
            if enabled {
                execute!(terminal.backend_mut(), EnableMouseCapture)?;
            } else {
                execute!(terminal.backend_mut(), DisableMouseCapture)?;
            }
        }

        // Update terminal title with unread count
        let unread = app.store.total_unread();
        let title = if unread > 0 {
            format!("siggy ({unread})")
        } else {
            "siggy".to_string()
        };
        execute!(
            terminal.backend_mut(),
            crossterm::terminal::SetTitle(&title)
        )?;

        if app.should_quit {
            break;
        }
    }

    // Restore terminal title on exit
    execute!(terminal.backend_mut(), crossterm::terminal::SetTitle("")).ok();

    Ok(())
}
