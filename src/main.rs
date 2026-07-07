//! Binary entry point and event loop.
//!
//! Polls the keyboard at 50 ms, drains [`signal::types::SignalEvent`]s into
//! the [`app::App`] state, and renders each frame via [`ui::draw`].
//! Orchestrates the first-run flow: setup wizard -> device linking -> app
//! startup.

// Backend features are mutually exclusive (#640 U7, plan R2/KTD-1): the
// boundary is compile-time selected and exactly one engine exists per binary.
#[cfg(all(feature = "signal-cli-backend", feature = "native-backend"))]
compile_error!(
    "features `signal-cli-backend` and `native-backend` are mutually exclusive; enable exactly one"
);
#[cfg(not(any(feature = "signal-cli-backend", feature = "native-backend")))]
compile_error!(
    "one backend feature is required: `signal-cli-backend` (default) or `native-backend`"
);

mod app;
mod audio;
mod autocomplete;
mod backend;
mod compose;
mod config;
mod conversation_store;
mod db;
mod debug_log;
mod demo;
mod domain;
mod export;
mod fs_migrate;
mod handlers;
mod image_render;
mod input;
mod keybindings;
mod link;
mod list_overlay;
mod mute;
mod preview;
mod settings_profile;
mod setup;
mod signal;
mod theme;
mod trigger;
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

use app::{App, InputMode, OverlayKind, SendRequest};
use config::Config;
use setup::SetupResult;
use signal::client::SignalClient;

/// Keyboard polling interval for the main event loop.
const POLL_TIMEOUT: Duration = Duration::from_millis(50);

/// How long to wait for the startup contact sync before giving up on the
/// loading spinner. If the contact list never comes back (e.g. signal-cli is
/// not properly linked), the app would otherwise spin on "Loading..." forever
/// with no feedback. After this grace period we clear the loading state, show a
/// diagnostic hint, and let the user reach the rest of the app. Generous enough
/// not to fire on a slow-but-healthy first sync.
const STARTUP_SYNC_TIMEOUT: Duration = Duration::from_secs(20);

/// How many times to retry spawning signal-cli after the child dies before
/// giving up and leaving the disconnect error on screen (#497).
const MAX_RECONNECT_ATTEMPTS: u32 = 6;

/// Exponential backoff before the next signal-cli reconnect attempt, capped at
/// 30s. `attempt` is 1-based (the delay applied *after* attempt N fails, before
/// attempt N+1). The first attempt fires immediately on disconnect, so this is
/// only consulted after a failure.
fn reconnect_backoff(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.min(6)).min(30))
}

/// Translate signal-cli stderr into a clearer message for the non-interactive
/// CLI commands when it indicates a well-known fatal condition. signal-cli
/// allows only one process per account, so running `--send`/`--receive` while
/// the siggy TUI is open makes signal-cli block on the account lock and the
/// command times out with an otherwise opaque error. Returns `None` when
/// nothing recognised, so the caller keeps its generic message.
fn cli_stderr_hint(stderr: &str) -> Option<&'static str> {
    let e = stderr.to_ascii_lowercase();
    if e.contains("is in use by another instance") || e.contains("config file is in use") {
        Some("another siggy or signal-cli instance is using this account - close it and try again")
    } else if e.contains("is not registered") {
        Some("this account is not registered - run `siggy --setup` to link a device")
    } else {
        None
    }
}

/// Classify a `--send` recipient: phone numbers and usernames (`@name.123`
/// or `u:name.123`, normalized to signal-cli's `u:` form) are 1:1 targets;
/// anything else is treated as a group id (#257, #612).
fn oneshot_target(recipient: &str) -> (String, bool) {
    if let Some(handle) = recipient
        .strip_prefix('@')
        .or_else(|| recipient.strip_prefix("u:"))
    {
        // Lowercase like the interactive /join path (usernames are
        // case-insensitive; nicknames are canonically lowercase)
        (format!("u:{}", handle.to_lowercase()), false)
    } else if recipient.starts_with('+') {
        (recipient.to_string(), false)
    } else {
        (recipient.to_string(), true)
    }
}

/// Send a single message non-interactively, await confirmation, and return
/// whether it was accepted (#257). Spawns signal-cli, sends, then waits up to
/// 30s for the correlated `SendTimestamp` (ok) or `SendFailed`.
async fn run_send_oneshot(config: &Config, recipient: &str, body: &str) -> Result<bool> {
    let mut client = SignalClient::spawn(config).await?;
    let (target, is_group) = oneshot_target(recipient);
    let token = client
        .send_message(&target, body, is_group, &[], &[], &[], None, None)
        .await?;

    let outcome = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match client.event_rx.recv().await {
                Some(signal::types::SignalEvent::SendTimestamp { token: t, .. }) if t == token => {
                    return Some(true);
                }
                Some(signal::types::SignalEvent::SendFailed { token: t }) if t == token => {
                    return Some(false);
                }
                Some(_) => continue,
                None => return None, // channel closed: child exited
            }
        }
    })
    .await;

    let _ = client.shutdown().await;
    let stderr = client.stderr_output();
    match outcome {
        Ok(Some(ok)) => Ok(ok),
        Ok(None) => Err(match cli_stderr_hint(&stderr) {
            Some(hint) => anyhow::anyhow!("{hint}"),
            None => anyhow::anyhow!("signal-cli disconnected before confirming the send"),
        }),
        Err(_) => Err(match cli_stderr_hint(&stderr) {
            Some(hint) => anyhow::anyhow!("{hint}"),
            None => anyhow::anyhow!("timed out waiting for send confirmation"),
        }),
    }
}

/// Resolve the message database path for `config` (#260): the custom `db_path`
/// (absolute as-is, relative under the data dir) or the default `siggy.db`. Does
/// not perform the legacy signal-tui.db rename (that is a startup-only concern).
fn resolve_db_path(config: &Config) -> std::path::PathBuf {
    let db_dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("siggy");
    match config.db_path.as_deref().filter(|p| !p.is_empty()) {
        Some(custom) => {
            let p = std::path::Path::new(custom);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                db_dir.join(p)
            }
        }
        None => db_dir.join("siggy.db"),
    }
}

/// Stream incoming messages to stdout, one tab-separated line each
/// (`timestamp_ms`, sender, group-id-or-empty, body), until signal-cli
/// disconnects or the user interrupts (Ctrl-C) (#257). Outgoing/sync and
/// body-less (attachment-only) messages are skipped; the body has control
/// characters stripped so each message stays a single clean line.
async fn run_receive_stream(config: &Config) -> Result<()> {
    use std::io::Write;
    let mut client = SignalClient::spawn(config).await?;
    loop {
        match client.event_rx.recv().await {
            Some(signal::types::SignalEvent::MessageReceived(msg)) => {
                if msg.is_outgoing {
                    continue;
                }
                if let Some(body) = msg.body.as_deref().filter(|b| !b.is_empty()) {
                    let clean = debug_log::strip_control_chars(body, false);
                    let group = msg.group_id.as_deref().unwrap_or("");
                    println!(
                        "{}\t{}\t{}\t{}",
                        msg.timestamp.timestamp_millis(),
                        msg.source,
                        group,
                        clean
                    );
                    let _ = std::io::stdout().flush();
                }
            }
            Some(signal::types::SignalEvent::Disconnected) | None => break,
            Some(_) => {}
        }
    }
    let _ = client.shutdown().await;
    // If signal-cli disconnected for a recognised reason (account lock held by
    // another instance, not registered), report it instead of exiting cleanly.
    match cli_stderr_hint(&client.stderr_output()) {
        Some(hint) => Err(anyhow::anyhow!("{hint}")),
        None => Ok(()),
    }
}

/// Headless trigger mode (#615): evaluate `triggers.toml` rules over the
/// incoming stream and execute their actions, logging one line per firing.
/// For 1:1 chats the conversation "name" is the phone number here (no
/// contact list without the TUI), so `conversation` filters should use ids.
async fn run_watch(config: &Config) -> Result<()> {
    use std::io::Write;
    let mut engine = trigger::TriggerEngine::load_default();
    for warning in &engine.warnings {
        eprintln!("warning: {warning}");
    }
    if engine.rule_count() == 0 {
        let path = trigger::TriggerEngine::rules_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "triggers.toml".to_string());
        anyhow::bail!("no trigger rules loaded; create {path}");
    }
    println!("watching with {} rule(s)", engine.rule_count());

    let mut client = SignalClient::spawn(config).await?;
    loop {
        match client.event_rx.recv().await {
            Some(signal::types::SignalEvent::MessageReceived(msg)) => {
                let Some(body) = msg.body.as_deref().filter(|b| !b.is_empty()) else {
                    continue;
                };
                let conv_id = msg.group_id.clone().unwrap_or_else(|| msg.source.clone());
                let is_group = msg.group_id.is_some();
                let ctx = trigger::MessageContext {
                    body,
                    sender_id: &msg.source,
                    sender_name: msg.source_name.as_deref(),
                    conv_id: &conv_id,
                    conv_name: msg.group_name.as_deref().unwrap_or(&conv_id),
                    is_group,
                    is_outgoing: msg.is_outgoing,
                    timestamp_ms: msg.timestamp.timestamp_millis(),
                };
                for action in engine.evaluate(&ctx, Instant::now()) {
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    match action {
                        trigger::TriggerAction::Reply {
                            conv_id,
                            is_group,
                            text,
                        } => match client
                            .send_message(&conv_id, &text, is_group, &[], &[], &[], None, None)
                            .await
                        {
                            Ok(_) => println!("{now_ms}\treply\t{conv_id}\t{text}"),
                            Err(e) => eprintln!("reply to {conv_id} failed: {e}"),
                        },
                        trigger::TriggerAction::Run { argv, stdin_json } => {
                            match trigger::execute_run(&argv, stdin_json) {
                                Ok(()) => println!("{now_ms}\trun\t{}", argv.join(" ")),
                                Err(e) => eprintln!("run {} failed: {e}", argv.join(" ")),
                            }
                        }
                    }
                }
                let _ = std::io::stdout().flush();
            }
            Some(signal::types::SignalEvent::Disconnected) | None => break,
            Some(_) => {}
        }
    }
    let _ = client.shutdown().await;
    match cli_stderr_hint(&client.stderr_output()) {
        Some(hint) => Err(anyhow::anyhow!("{hint}")),
        None => Ok(()),
    }
}

/// Whether the session should auto-lock now (#438). `timeout_mins == 0` disables
/// it; otherwise lock once keyboard `idle` reaches that many minutes and we are
/// not already locked. Only keypresses reset idle (the caller excludes mouse
/// motion), so a wireless mouse on a desk can't hold the lock open forever.
fn should_auto_lock(timeout_mins: u64, idle: Duration, already_locked: bool) -> bool {
    timeout_mins > 0 && !already_locked && idle >= Duration::from_secs(timeout_mins * 60)
}

/// Whether the startup contact sync has run past its grace period without
/// completing (`loading` still true). Pure, like `should_auto_lock`, so the
/// watchdog decision can be unit-tested without driving the event loop.
fn startup_sync_timed_out(loading: bool, elapsed: Duration) -> bool {
    loading && elapsed >= STARTUP_SYNC_TIMEOUT
}

/// Set restrictive permissions (0600) on a sensitive file (Unix only).
#[cfg(unix)]
fn set_file_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

// On Windows there is no portable chmod equivalent, so siggy relies on the
// default per-user ACLs that Windows applies to the profile directories these
// files live under (%APPDATA%, %USERPROFILE%, %LOCALAPPDATA%): other standard
// users cannot read them, though local administrators still can. Setting an
// owner-only DACL would mean linking the Win32 security APIs and could lock a
// user out of their own files if SID resolution failed, so the reliance is
// documented in docs/src/user-guide/security.md instead (#504).
#[cfg(not(unix))]
fn set_file_permissions(_path: &std::path::Path) {}

/// Set restrictive permissions (0700) on a sensitive directory (Unix only).
#[cfg(unix)]
fn set_dir_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

// No-op on Windows for the same reason as `set_file_permissions` above.
#[cfg(not(unix))]
fn set_dir_permissions(_path: &std::path::Path) {}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI args
    let args: Vec<String> = std::env::args().collect();
    let mut config_path: Option<&str> = None;
    let mut account: Option<String> = None;
    let mut force_setup = false;
    let mut demo_mode = false;
    let mut incognito = false;
    let mut debug = false;
    let mut debug_full = false;
    let mut reset_lock = false;
    let mut reset_account = false;
    let mut check = false;
    let mut list = false;
    let mut receive = false;
    let mut watch = false;
    let mut send_args: Option<(String, String)> = None;

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
            "--reset-lock" => {
                reset_lock = true;
                i += 1;
            }
            "--reset-account" => {
                reset_account = true;
                i += 1;
            }
            "--check" => {
                check = true;
                i += 1;
            }
            "--list" => {
                list = true;
                i += 1;
            }
            "--receive" => {
                receive = true;
                i += 1;
            }
            "--watch" => {
                watch = true;
                i += 1;
            }
            "--send" => {
                if i + 2 < args.len() {
                    send_args = Some((args[i + 1].clone(), args[i + 2].clone()));
                    i += 3;
                } else {
                    eprintln!("--send requires <recipient> <message>");
                    std::process::exit(2);
                }
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
                eprintln!("      --reset-lock        Delete the session-lock passphrase and exit");
                eprintln!(
                    "      --reset-account     Clear local signal-cli account data (for a clean relink) and exit"
                );
                eprintln!(
                    "      --check             Print a setup health report and exit (0 = ready)"
                );
                eprintln!("      --send <TO> <MSG>   Send one message non-interactively and exit");
                eprintln!(
                    "      --list              List cached conversations (tab-separated) and exit"
                );
                eprintln!(
                    "      --receive           Stream incoming messages (tab-separated) until interrupted"
                );
                eprintln!("  -V, --version           Print version and exit");
                eprintln!("      --help              Show this help");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                // Plain "siggy x.y.z" on stdout for scripts/CI (#257).
                println!("siggy {}", env!("CARGO_PKG_VERSION"));
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
    let resolved_config_path = match config_path {
        Some(p) => std::path::PathBuf::from(p),
        None => Config::default_config_path(),
    };

    if reset_lock {
        let hash_path = domain::lock_hash_path(&resolved_config_path);
        if hash_path.exists() {
            std::fs::remove_file(&hash_path)?;
            println!("Removed lock hash: {}", hash_path.display());
        } else {
            println!(
                "No lock hash to remove (looked for {})",
                hash_path.display()
            );
        }
        return Ok(());
    }

    let mut config = Config::load(config_path)?;
    if let Some(acct) = account {
        config.account = acct;
    }

    // Non-interactive counterpart to the relink guard (#603/#607): clear stale
    // local signal-cli account data so a subsequent `--setup` can relink from a
    // clean slate. Exits without launching the TUI. Note: this only removes
    // local data; a server-side conflict also needs old devices removed on the
    // phone.
    if reset_account {
        if config.account.is_empty() {
            eprintln!("no account configured; nothing to reset");
            std::process::exit(1);
        }
        if !setup::account_exists_locally(&config.account) {
            println!("No local account data found for {}.", config.account);
            std::process::exit(0);
        }
        match setup::delete_local_account_data(&config).await {
            Ok(()) => {
                println!(
                    "Cleared local signal-cli account data for {}. Run `siggy --setup` to relink.",
                    config.account
                );
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("failed to clear account data: {e}");
                std::process::exit(1);
            }
        }
    }

    // Non-interactive setup health report for scripts/CI (#257). Plain stdout,
    // no TUI; exit 0 only when signal-cli is detected and an account is set.
    if check {
        let (found, location, _resolved) = setup::check_signal_cli(&config.signal_cli_path).await;
        let account_ok = !config.account.is_empty();
        println!("siggy {}", env!("CARGO_PKG_VERSION"));
        println!("config:       {}", resolved_config_path.display());
        println!(
            "account:      {}",
            if account_ok {
                config.account.as_str()
            } else {
                "(not set)"
            }
        );
        println!(
            "signal-cli:   {}",
            if found {
                location
            } else {
                format!("NOT FOUND (configured: {})", config.signal_cli_path)
            }
        );
        println!("download dir: {}", config.download_dir.display());
        let ready = found && account_ok;
        println!(
            "status:       {}",
            if ready { "ready" } else { "not ready" }
        );
        std::process::exit(if ready { 0 } else { 1 });
    }

    // Non-interactive conversation list for scripts (#257). Reads the cached DB
    // (no signal-cli spawn) and prints tab-separated rows: unread, type, name, id
    // -- no header, so it pipes cleanly into grep/cut/awk.
    if list {
        let db_path = resolve_db_path(&config);
        if !db_path.exists() {
            eprintln!(
                "no database at {} (run siggy once to create it)",
                db_path.display()
            );
            std::process::exit(1);
        }
        let db = db::Database::open(&db_path)?;
        for c in db.load_conversations(1)? {
            let kind = if c.is_group { "group" } else { "dm" };
            println!("{}\t{}\t{}\t{}", c.unread, kind, c.name, c.id);
        }
        std::process::exit(0);
    }

    // Non-interactive incoming-message stream for scripts (#257). Runs until
    // signal-cli disconnects or Ctrl-C; no TUI.
    if receive {
        match run_receive_stream(&config).await {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("receive failed: {e}");
                std::process::exit(1);
            }
        }
    }

    // Headless trigger mode (#615). Mutually exclusive with the TUI on the
    // same account: signal-cli allows one process per account.
    if watch {
        match run_watch(&config).await {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("watch failed: {e}");
                std::process::exit(1);
            }
        }
    }

    // Non-interactive one-shot send for scripts/cron (#257). No TUI; exit 0 on
    // confirmed send, 1 on rejection/timeout/error.
    if let Some((recipient, body)) = send_args {
        match run_send_oneshot(&config, &recipient, &body).await {
            Ok(true) => {
                println!("sent");
                std::process::exit(0);
            }
            Ok(false) => {
                eprintln!("send failed: signal-cli rejected the message");
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("send failed: {e}");
                std::process::exit(1);
            }
        }
    }

    // Disable the default Windows Ctrl+C handler for the TUI only — crossterm
    // captures it as a key event in raw mode, so the OS handler would just cause
    // a noisy exit. Done here (not at startup) so non-interactive commands above
    // (--receive etc.) stay interruptible with Ctrl-C.
    #[cfg(windows)]
    unsafe {
        unsafe extern "system" {
            fn SetConsoleCtrlHandler(handler: usize, add: i32) -> i32;
        }
        SetConsoleCtrlHandler(0, 1);
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
        &resolved_config_path,
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
    config_path: &std::path::Path,
) -> Result<()> {
    if demo_mode {
        let database = db::Database::open_in_memory()?;
        let demo_config = Config {
            account: "+15551234567".to_string(),
            ..Config::default()
        };
        return run_app(
            terminal,
            backend::DemoBackend,
            &demo_config,
            database,
            false,
            config_path,
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
        let data_root = dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        let db_dir = data_root.join("siggy");
        let old_db_dir = data_root.join("signal-tui");
        fs_migrate::migrate_path(&old_db_dir, &db_dir);

        std::fs::create_dir_all(&db_dir)?;
        set_dir_permissions(&db_dir);
        let db_path = resolve_db_path(config);
        if config
            .db_path
            .as_deref()
            .filter(|p| !p.is_empty())
            .is_some()
        {
            // Custom per-account db (#260) may live outside db_dir; ensure its
            // parent exists. No legacy rename - that only applies to the default.
            if let Some(parent) = db_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
        } else {
            fs_migrate::migrate_path(&db_dir.join("signal-tui.db"), &db_path);
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
    // ActiveBackend is the cfg-selected engine (signal-cli today, see
    // src/backend/). It borrows the client: run_main_flow keeps ownership
    // because shutdown happens after run_app returns.
    let result = run_app(
        terminal,
        backend::ActiveBackend::new(&mut signal_client),
        config,
        database,
        incognito,
        config_path,
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

/// Write a single native-image escape sequence, wrapping it in tmux's DCS
/// passthrough envelope when `in_tmux` is true. Outside tmux this is a plain
/// pass-through (byte-for-byte equivalent to `write!(backend, "{seq}")`).
fn emit_passthrough_seq(
    backend: &mut CrosstermBackend<io::Stdout>,
    seq: &str,
    in_tmux: bool,
) -> io::Result<()> {
    use std::io::Write;
    if in_tmux {
        backend.write_all(image_render::wrap_for_tmux(seq).as_bytes())
    } else {
        backend.write_all(seq.as_bytes())
    }
}

/// Overwrite previously drawn Sixel image rectangles with spaces.
///
/// This is much cheaper than clearing the whole terminal and avoids re-sending
/// current image rectangles before every Sixel payload.
fn erase_sixel_rects(
    backend: &mut CrosstermBackend<io::Stdout>,
    images: &[app::VisibleImage],
) -> Result<()> {
    if images.is_empty() {
        return Ok(());
    }
    queue!(backend, Hide, SavePosition)?;
    for img in images {
        if img.width == 0 || img.height == 0 {
            continue;
        }
        let blank = " ".repeat(img.width as usize);
        for row in 0..img.height {
            queue!(backend, MoveTo(img.x, img.y + row), Print(blank.as_str()))?;
        }
    }
    queue!(backend, RestorePosition, Show)?;
    Ok(())
}

/// Write native terminal image protocol escape sequences.
///
/// For Kitty: process `kitty_pending_transmits` — transmit image data and create
/// virtual placements. The actual display uses Unicode Placeholder cells embedded
/// in the ratatui buffer by `render_placeholder()`.
///
/// For iTerm2: overlay pre-resized images on top of the halfblock placeholders
/// using cursor-positioned inline image sequences.
///
/// When running inside tmux (`$TMUX` set), every Kitty (`\x1b_G…\x1b\\`) and
/// iTerm2 (`\x1b]1337;…\x07`) escape is wrapped in tmux's DCS passthrough
/// envelope so the outer terminal can still see it. Requires `tmux 3.3+` with
/// `set -g allow-passthrough on` (or `all`).
fn emit_native_images(backend: &mut CrosstermBackend<io::Stdout>, app: &mut App) -> Result<()> {
    let protocol = app.image.image_protocol;
    if protocol == image_render::ImageProtocol::Halfblock {
        return Ok(());
    }

    use std::io::Write;
    let tmux = image_render::in_tmux();

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
                let seq = if i == 0 {
                    format!("\x1b_Gf=100,a=t,i={id},q=2,m={m};{chunk_str}\x1b\\")
                } else {
                    format!("\x1b_Gm={m};{chunk_str}\x1b\\")
                };
                emit_passthrough_seq(backend, &seq, tmux)?;
            }

            // Create virtual placement (U=1 enables Unicode Placeholder mode)
            let placement = format!("\x1b_Ga=p,U=1,i={id},c={cols},r={rows},q=2\x1b\\");
            emit_passthrough_seq(backend, &placement, tmux)?;

            app.image.kitty_transmitted.insert(*id);
        }

        backend.flush()?;
        return Ok(());
    }

    // Sixel: slice the cached full Sixel to the visible region (instant string op).
    if protocol == image_render::ImageProtocol::Sixel {
        if app.has_overlay() || app.image.visible_images.is_empty() {
            app.image.visible_images.clear();
            app.image.prev_visible_images.clear();
            return Ok(());
        }

        if app.image.visible_images == app.image.prev_visible_images {
            app.image.visible_images.clear();
            return Ok(());
        }

        let images = std::mem::take(&mut app.image.visible_images);
        let all_cached = images
            .iter()
            .all(|img| app.image.sixel_cache.contains_key(&img.path));
        queue!(backend, Hide, SavePosition)?;

        for img in &images {
            if img.width == 0 || img.height == 0 {
                continue;
            }
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
        if all_cached {
            app.image.prev_visible_images = images;
        } else {
            app.image.prev_visible_images.clear();
        }
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

        let seq = format!(
            "\x1b]1337;File=inline=1;width={};height={};preserveAspectRatio=0:{b64}\x07",
            img.width, img.height
        );
        emit_passthrough_seq(backend, &seq, tmux)?;
    }

    queue!(backend, RestorePosition)?;
    // No flush - let EndSynchronizedUpdate send everything atomically.

    app.image.prev_visible_images = images;
    Ok(())
}

async fn run_app<B: backend::Backend>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut backend: B,
    config: &Config,
    db: db::Database,
    incognito: bool,
    config_path: &std::path::Path,
) -> Result<()> {
    let mut app = App::new(config.account.clone(), db, config_path);
    // Table-driven boolean toggles (notifications, display, messages,
    // interface) load through the shared SETTINGS table, paired with the save
    // loop in save_settings (#498).
    app.apply_settings_from_config(config);
    // Non-toggle scalar settings, copied directly.
    app.notifications.notification_preview = config.notification_preview;
    app.notifications.clipboard_clear_seconds = config.clipboard_clear_seconds;
    app.image.image_mode = config.image_mode.unwrap_or_default();
    app.image.image_max_width = config.image_max_width.clamp(1, 240);
    app.image.preview_image_max_width = config.preview_image_max_width.clamp(1, 240);
    app.image.image_max_height = config.image_max_height.clamp(1, 120);
    app.image.sixel_encode = image_render::SixelEncodeSettings {
        max_colors: config.sixel_max_colors.clamp(2, 256),
        diffusion: config.sixel_diffusion.clamp(0.0, 1.0),
    };
    app.incognito = incognito;
    app.media.download_dir = config.download_dir.clone();
    app.media.audio_player = config.audio_player.clone();
    // Message triggers (#615): loaded here (not in App::new) so tests and
    // demo mode never pick up the user's triggers.toml.
    app.triggers = trigger::TriggerEngine::load_default();
    for warning in &app.triggers.warnings {
        debug_log::logf(format_args!("triggers.toml: {warning}"));
    }
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
    // Per-backend startup: mark connected and kick off the initial sync
    // (signal-cli), or populate the demo fixtures (see src/backend/).
    backend.startup(&mut app).await;

    let mut last_expiry_sweep = Instant::now();
    let mut last_sync_redraw = Instant::now();
    // Initialise far enough in the past that the spinner ticks on the very
    // first loop iteration. Without this the spinner sits on frame 0 for the
    // first 80ms after entering run_app, which on slow hardware looks like a
    // freeze before any animation starts (#426).
    let mut last_spinner_tick = Instant::now()
        .checked_sub(Duration::from_millis(80))
        .unwrap_or_else(Instant::now);
    let mut needs_redraw = true;
    let mut last_title: Option<String> = None;
    // Supervised signal-cli reconnect state (#497). `next_reconnect_at` is the
    // earliest time the next spawn attempt may run; None means "try now".
    let mut reconnect_attempts: u32 = 0;
    let mut next_reconnect_at: Option<Instant> = None;
    // Auto-lock idle tracking (#438): reset on every keypress (not mouse).
    let mut last_activity = Instant::now();
    // Startup-sync watchdog: when the loop begins, `app.loading` is still true
    // until the contact list arrives. If it never does, the timeout below clears
    // the spinner so the app does not appear hung (see STARTUP_SYNC_TIMEOUT).
    let loop_started = Instant::now();
    // Loop-rate diagnostics: only counted/logged when --debug is on. Helps
    // locate non-converging redraw triggers (see issue #408). Cheap when off.
    let mut diag_last = Instant::now();
    let mut diag_iter = 0u32;
    let mut diag_render = 0u32;
    let mut diag_term_event = 0u32;
    let mut diag_signal_event = 0u32;

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
        diag_iter = diag_iter.wrapping_add(1);
        // Only redraw when state has changed (avoids resetting cursor blink timer every 50ms)
        if needs_redraw {
            diag_render = diag_render.wrapping_add(1);
            let native = app.image.image_mode == crate::domain::ImageMode::Native;
            let sixel_mode =
                native && app.image.image_protocol == image_render::ImageProtocol::Sixel;

            // Always start sync update for atomic rendering (prevents cursor flicker).
            queue!(terminal.backend_mut(), BeginSynchronizedUpdate)?;

            // Force full redraw when active conversation changes (clears native image artifacts)
            let conversation_changed =
                native && app.active_conversation != app.prev_active_conversation;
            let scroll_changed = sixel_mode && app.scroll.offset != app.image.sixel_prev_scroll;
            let clear_previous_sixels =
                sixel_mode && (conversation_changed || scroll_changed || app.has_overlay());

            if conversation_changed {
                app.prev_active_conversation = app.active_conversation.clone();
                app.image.visible_images.clear();
                terminal.clear()?;
            }
            // Sixel: force full redraw when scroll changes so ratatui resends
            // ALL cells. The text output (inside sync) overwrites the terminal
            // buffer, then after EndSync our Sixel overlays at the new positions.
            // Without this, stale Sixel pixels persist at old image positions
            // because ratatui's diff only sends changed cells.
            if scroll_changed {
                app.image.sixel_prev_scroll = app.scroll.offset;
                app.image.visible_images.clear();
                terminal.clear()?;
            }
            if clear_previous_sixels {
                erase_sixel_rects(terminal.backend_mut(), &app.image.prev_visible_images)?;
                app.image.prev_visible_images.clear();
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
        // Background /preview fetch (#267): apply the result when it lands.
        if app.poll_preview_fetch() {
            needs_redraw = true;
        }
        // Voice playback progress (#618): reap the player / refresh the line.
        if app.tick_voice_playback() {
            needs_redraw = true;
        }
        // Keep the encode caches bounded (#492). Cheap O(1) length check per tick.
        app.image.enforce_cache_caps();

        // Debounced message search: keystrokes mark the query dirty; the DB
        // scan runs here once typing pauses (#491).
        if app.is_overlay(OverlayKind::Search)
            && app
                .search
                .run_if_due(app.active_conversation.as_deref(), &app.db)
        {
            needs_redraw = true;
        }

        // Animate the loading spinner on a wall-clock cadence so its speed
        // is decoupled from event-loop iteration rate. The drain loop above
        // collapses bursts of input into a single outer iteration, which
        // would otherwise tie the spinner to "events arrived" rather than
        // "time passed". 80ms = 12.5fps, brisk but not frantic.
        //
        // The counter advances whenever loading=true regardless of sync state
        // (see #426 -- otherwise the spinner appears frozen during the initial
        // sync burst). The redraw, however, only fires outside sync so the
        // 500ms sync redraw throttle in drain_events still applies (see #326).
        // During sync the spinner therefore animates at ~2fps (one frame per
        // throttled redraw), and at 12.5fps after sync ends.
        if app.loading && last_spinner_tick.elapsed() >= Duration::from_millis(80) {
            app.spinner_tick = app.spinner_tick.wrapping_add(1);
            last_spinner_tick = Instant::now();
            if app.should_tick_spinner() {
                needs_redraw = true;
            }
        }

        // Startup-sync watchdog: if the contact list never came back, stop
        // spinning on "Loading..." forever and surface a diagnostic hint so the
        // user knows to check their link instead of staring at a frozen screen.
        // Fires once (clearing `loading` makes the guard false); a late contact
        // list still clears the state again harmlessly.
        if startup_sync_timed_out(app.loading, loop_started.elapsed()) {
            app.loading = false;
            app.startup_status.clear();
            app.status_message =
                "Sync is taking longer than expected. Run siggy --check to verify your link."
                    .to_string();
            needs_redraw = true;
        }

        // Extend the scrollback when scrolled to the top: widen the render
        // window over loaded messages, paging from the DB only when memory
        // is exhausted (#488).
        if app.scroll.at_top {
            app.extend_scrollback();
            needs_redraw = true;
        }

        // Poll for events with a short timeout so we stay responsive to signal events.
        // Drain all queued terminal events in one pass so a flood of mouse-move events
        // (with EnableMouseCapture, terminals send one per pixel of motion) doesn't
        // multiply the per-tick bookkeeping below into a busy loop. See issue #408.
        let mut had_terminal_event = event::poll(POLL_TIMEOUT)?;
        while had_terminal_event {
            diag_term_event = diag_term_event.wrapping_add(1);
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    needs_redraw = true;
                    last_activity = Instant::now();
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
            had_terminal_event = event::poll(Duration::ZERO)?;
        }

        // Auto-lock after configured keyboard inactivity (#438).
        if should_auto_lock(
            config.lock_timeout,
            last_activity.elapsed(),
            app.lock.is_locked(),
        ) {
            app.lock_now();
            needs_redraw = true;
        }

        // Drain signal events (non-blocking), detect disconnect
        if backend.drain_events(&mut app) {
            diag_signal_event = diag_signal_event.wrapping_add(1);
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

        // Supervised reconnect: only while disconnected, only for the Signal
        // backend, and only up to the attempt cap. The first attempt fires
        // immediately; subsequent ones wait for the backoff (#497).
        if app.connection_error.is_some()
            && backend.supports_reconnect()
            && reconnect_attempts < MAX_RECONNECT_ATTEMPTS
        {
            let now = Instant::now();
            let due = next_reconnect_at.map(|t| now >= t).unwrap_or(true);
            if due {
                reconnect_attempts += 1;
                app.status_message = format!(
                    "Reconnecting to signal-cli (attempt {reconnect_attempts}/{MAX_RECONNECT_ATTEMPTS})..."
                );
                needs_redraw = true;
                if backend.try_reconnect(config).await {
                    reconnect_attempts = 0;
                    next_reconnect_at = None;
                    app.connection_error = None;
                    app.set_connected();
                    // Re-run the startup sync against the fresh child (best-effort).
                    backend.resync_after_reconnect().await;
                } else if reconnect_attempts >= MAX_RECONNECT_ATTEMPTS {
                    app.status_message =
                        "signal-cli disconnected; reconnect failed. Restart siggy.".to_string();
                } else {
                    let delay = reconnect_backoff(reconnect_attempts);
                    next_reconnect_at = Some(now + delay);
                    app.status_message = format!(
                        "signal-cli reconnect failed; retrying in {}s",
                        delay.as_secs()
                    );
                }
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

        // Dispatch auto-replies queued by message triggers (#615)
        for req in std::mem::take(&mut app.pending.trigger_sends) {
            backend.dispatch(&mut app, req).await;
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

        // Terminal bell on new messages in background conversations. Suppress
        // entirely while the session is locked -- the bell would advertise
        // activity even though the screen is supposed to be opaque.
        if app.notifications.pending_bell {
            app.notifications.pending_bell = false;
            if !app.lock.is_locked() {
                execute!(terminal.backend_mut(), Print("\x07"))?;
            }
        }

        // Auto-clear clipboard after timeout
        app.check_clipboard_clear();

        // Delete paste temp files that have passed their 10s post-send delay
        app.cleanup_paste_files();

        // Dynamic mouse capture toggle from settings
        if let Some(enabled) = app.mouse.pending_toggle.take() {
            if enabled {
                execute!(terminal.backend_mut(), EnableMouseCapture)?;
            } else {
                execute!(terminal.backend_mut(), DisableMouseCapture)?;
            }
        }

        // Update terminal title with unread count, but only when it actually changes.
        // Emitting SetTitle every tick (~20Hz) is a write+flush per iteration the terminal
        // has to parse, which compounds with mouse-move event storms. See issue #408.
        let unread = app.store.total_unread();
        let title = if app.lock.is_locked() {
            "siggy".to_string()
        } else if unread > 0 {
            format!("siggy ({unread})")
        } else {
            "siggy".to_string()
        };
        if last_title.as_deref() != Some(title.as_str()) {
            execute!(
                terminal.backend_mut(),
                crossterm::terminal::SetTitle(&title)
            )?;
            last_title = Some(title);
        }

        // Periodic loop-rate digest under --debug. Helps diagnose unconverged
        // redraw triggers in real-world setups (issue #408). No-op when off.
        if diag_last.elapsed() >= Duration::from_secs(5) {
            debug_log::logf(format_args!(
                "loop 5s: iter={diag_iter} render={diag_render} term_ev={diag_term_event} sig_ev={diag_signal_event} bg_in_flight={} active_conv={}",
                app.image.image_render_in_flight.len(),
                app.active_conversation
                    .as_deref()
                    .map(|_| "yes")
                    .unwrap_or("no"),
            ));
            diag_last = Instant::now();
            diag_iter = 0;
            diag_render = 0;
            diag_term_event = 0;
            diag_signal_event = 0;
        }

        if app.should_quit {
            break;
        }
    }

    // Stop any voice playback so the player does not outlive the app (#618).
    app.stop_voice_playback();

    // Restore terminal title on exit
    execute!(terminal.backend_mut(), crossterm::terminal::SetTitle("")).ok();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oneshot_target_classifies_recipients() {
        // Phone number: 1:1, passed through
        assert_eq!(
            oneshot_target("+15551234567"),
            ("+15551234567".into(), false)
        );
        // Username forms: 1:1, normalized to signal-cli's u: prefix and
        // lowercased like the interactive /join path (#612)
        assert_eq!(oneshot_target("@alice.42"), ("u:alice.42".into(), false));
        assert_eq!(oneshot_target("@Alice.42"), ("u:alice.42".into(), false));
        assert_eq!(oneshot_target("u:Alice.42"), ("u:alice.42".into(), false));
        // Anything else: group id
        assert_eq!(oneshot_target("grpid=="), ("grpid==".into(), true));
    }

    #[test]
    fn should_auto_lock_respects_timeout_and_state() {
        let min = Duration::from_secs(60);
        // Disabled: never locks, however idle.
        assert!(!should_auto_lock(0, Duration::from_secs(99999), false));
        // Enabled, idle past the threshold, not yet locked -> lock.
        assert!(should_auto_lock(1, min, false));
        assert!(should_auto_lock(5, 5 * min, false));
        // Idle below the threshold -> don't lock.
        assert!(!should_auto_lock(
            5,
            5 * min - Duration::from_secs(1),
            false
        ));
        // Already locked -> no-op (don't re-lock).
        assert!(!should_auto_lock(1, 10 * min, true));
    }

    #[test]
    fn cli_stderr_hint_recognises_known_fatal_conditions() {
        // The lock-contention line signal-cli prints when the TUI is open.
        assert_eq!(
            cli_stderr_hint(
                "INFO  SignalAccount - Config file is in use by another instance, waiting…"
            ),
            Some(
                "another siggy or signal-cli instance is using this account - close it and try again"
            )
        );
        // Unregistered account.
        assert_eq!(
            cli_stderr_hint("User +447588442858 is not registered."),
            Some("this account is not registered - run `siggy --setup` to link a device")
        );
        // Unrecognised stderr -> no hint (caller keeps its generic message).
        assert_eq!(cli_stderr_hint("some unrelated warning"), None);
        assert_eq!(cli_stderr_hint(""), None);
    }

    #[test]
    fn startup_sync_timed_out_only_fires_while_loading_past_grace() {
        // Not loading -> never times out, however long it has been.
        assert!(!startup_sync_timed_out(false, STARTUP_SYNC_TIMEOUT * 2));
        // Loading but still within the grace period -> keep waiting.
        assert!(!startup_sync_timed_out(
            true,
            STARTUP_SYNC_TIMEOUT - Duration::from_secs(1)
        ));
        // Loading past the grace period -> time out.
        assert!(startup_sync_timed_out(true, STARTUP_SYNC_TIMEOUT));
        assert!(startup_sync_timed_out(true, STARTUP_SYNC_TIMEOUT * 2));
    }

    #[test]
    fn reconnect_backoff_is_exponential_and_capped() {
        assert_eq!(reconnect_backoff(1), Duration::from_secs(2));
        assert_eq!(reconnect_backoff(2), Duration::from_secs(4));
        assert_eq!(reconnect_backoff(3), Duration::from_secs(8));
        assert_eq!(reconnect_backoff(4), Duration::from_secs(16));
        // Capped at 30s, and never overflows the shift for large attempts.
        assert_eq!(reconnect_backoff(5), Duration::from_secs(30));
        assert_eq!(reconnect_backoff(6), Duration::from_secs(30));
        assert_eq!(reconnect_backoff(100), Duration::from_secs(30));
    }
}
