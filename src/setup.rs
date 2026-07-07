//! First-run wizard.
//!
//! Multi-step flow that detects signal-cli on PATH, prompts for the user's
//! E.164 phone, and hands off to [`crate::link`] for QR-code device
//! linking. Persists the resolved signal-cli path back to [`crate::config`]
//! so subsequent launches skip detection.

use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Flex, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
};
use tokio::process::Command;

use crate::backend;
use crate::config::Config;
use crate::link;
use crate::signal::types::LinkState;

pub enum SetupResult {
    /// Wizard finished successfully, use this config.
    Completed(Box<Config>),
    /// User had a valid config, no setup needed.
    Skipped,
    /// User cancelled during setup.
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    SignalCli,
    Account,
    Linking,
    Preferences,
    Done,
}

/// Ordered wizard steps for the compiled-in backend (plan U5 gating hook,
/// #640). The binary-detection step exists only for engines that drive an
/// external binary (`needs_binary` = [`backend::NEEDS_CLI_BINARY`]); U10's
/// native backend flips that flag and the wizard starts at the account step.
/// Today only the first step feeds the imperative loop below - the in-loop
/// transitions stay hardcoded until a second backend actually exists.
fn wizard_steps(needs_binary: bool) -> Vec<Step> {
    let mut steps = Vec::with_capacity(5);
    if needs_binary {
        steps.push(Step::SignalCli);
    }
    steps.extend([Step::Account, Step::Linking, Step::Preferences, Step::Done]);
    steps
}

pub async fn run_setup(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: &Config,
    force: bool,
) -> Result<SetupResult> {
    if !force && !config.needs_setup() {
        return Ok(SetupResult::Skipped);
    }

    let mut working_config = config.clone();
    let mut step = *wizard_steps(backend::NEEDS_CLI_BINARY)
        .first()
        .expect("wizard step list is never empty");
    let mut signal_cli_path = working_config.signal_cli_path.clone();
    let mut phone_input = String::new();
    let mut phone_cursor: usize = 0;
    let mut phone_error: Option<String> = None;
    let mut signal_cli_found = false;
    let mut signal_cli_location = String::new();
    let mut custom_path_mode = false;
    let mut custom_path_input = String::new();
    let mut custom_path_cursor: usize = 0;
    // Relink pre-flight (#603): only prompt about pre-existing local account
    // data once, not on every link retry.
    let mut reset_checked = false;

    loop {
        match step {
            Step::SignalCli => {
                // Check for signal-cli
                if !signal_cli_found {
                    let (found, location, resolved) = check_signal_cli(&signal_cli_path).await;
                    signal_cli_found = found;
                    signal_cli_location = location;
                    if found {
                        // Persist the invocation that actually worked — `where`/`which`
                        // may have resolved a .bat/.cmd that Rust's Command couldn't
                        // find by bare name.
                        signal_cli_path = resolved;
                    }
                }

                terminal.draw(|frame| {
                    draw_signal_cli_step(
                        frame,
                        signal_cli_found,
                        &signal_cli_location,
                        custom_path_mode,
                        &custom_path_input,
                        custom_path_cursor,
                    );
                })?;

                if signal_cli_found && !custom_path_mode {
                    // Auto-advance after a brief pause
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    working_config.signal_cli_path = signal_cli_path.clone();
                    step = Step::Account;
                    continue;
                }

                // Wait for user input
                if event::poll(Duration::from_millis(50))?
                    && let Event::Key(key) = event::read()?
                {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    match handle_signal_cli_key(
                        custom_path_mode,
                        &mut custom_path_input,
                        &mut custom_path_cursor,
                        key.modifiers,
                        key.code,
                    ) {
                        SignalCliStepOutcome::Continue => {}
                        SignalCliStepOutcome::Cancel => {
                            return Ok(SetupResult::Cancelled);
                        }
                        SignalCliStepOutcome::Retry => {
                            // Re-check signal-cli on the next loop.
                            signal_cli_found = false;
                        }
                        SignalCliStepOutcome::EnterCustomPath => {
                            // Buffer already cleared by the handler.
                            custom_path_mode = true;
                        }
                        SignalCliStepOutcome::ExitCustomPath => {
                            custom_path_mode = false;
                        }
                        SignalCliStepOutcome::SetPath(path) => {
                            signal_cli_path = path;
                            signal_cli_found = false;
                            custom_path_mode = false;
                        }
                    }
                }
            }

            Step::Account => {
                terminal.draw(|frame| {
                    draw_account_step(frame, &phone_input, phone_cursor, phone_error.as_deref());
                })?;

                if event::poll(Duration::from_millis(50))?
                    && let Event::Key(key) = event::read()?
                {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    match handle_account_key(
                        &mut phone_input,
                        &mut phone_cursor,
                        &mut phone_error,
                        key.modifiers,
                        key.code,
                    ) {
                        AccountStepOutcome::Continue => {}
                        AccountStepOutcome::Cancel => {
                            return Ok(SetupResult::Cancelled);
                        }
                        AccountStepOutcome::Back => {
                            // Phone state is cleared by the handler; reset the
                            // signal-cli step flags so it re-checks on return.
                            step = Step::SignalCli;
                            signal_cli_found = false;
                            custom_path_mode = false;
                        }
                        AccountStepOutcome::Submit(account) => {
                            working_config.account = account;
                            step = Step::Linking;
                        }
                    }
                }
            }

            Step::Linking => {
                // Check if already registered - through the adapter's
                // link-state probe (#640 U5, flow gap G1) rather than a raw
                // listContacts exit-code read. Same probe, same semantics
                // (a failed probe reads as unlinked).
                let registered = backend::signal_cli::SignalCliBackend::link_state(&working_config)
                    .await
                    == LinkState::Linked;
                if registered {
                    // Already registered, skip linking
                    terminal.draw(|frame| {
                        draw_registered_screen(frame, &working_config.account);
                    })?;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    step = Step::Preferences;
                    continue;
                }

                // Pre-flight (#603): if signal-cli already has local data for
                // this number, a relink can fail with a server conflict. Offer
                // to clear it first. Runs once (not on link retries), and only
                // when local data actually exists.
                if !reset_checked {
                    reset_checked = true;
                    if account_exists_locally(&working_config.account) {
                        terminal.draw(|frame| {
                            draw_relink_confirm(frame, &working_config.account);
                        })?;
                        let mut go_back = false;
                        loop {
                            if event::poll(Duration::from_millis(50))?
                                && let Event::Key(key) = event::read()?
                            {
                                if key.kind != KeyEventKind::Press {
                                    continue;
                                }
                                match key.code {
                                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                                        terminal.draw(|frame| {
                                            draw_status_message(
                                                frame,
                                                "Clearing existing account data...",
                                            );
                                        })?;
                                        // Best-effort: if the reset fails, fall
                                        // through to linking anyway and let the
                                        // normal error path report it.
                                        let _ = delete_local_account_data(&working_config).await;
                                        break;
                                    }
                                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Enter => {
                                        break;
                                    }
                                    KeyCode::Esc => {
                                        go_back = true;
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        if go_back {
                            step = Step::Account;
                            continue;
                        }
                    }
                }

                // Run linking flow
                match link::run_linking_flow(terminal, &working_config).await {
                    Ok(link::LinkResult::Success) => {
                        step = Step::Preferences;
                    }
                    Ok(link::LinkResult::Cancelled) => {
                        step = Step::Account;
                    }
                    Err(e) => {
                        let msg = format!("{e}");
                        {
                            // Show error, let user retry or go back
                            terminal.draw(|frame| {
                                draw_link_error(frame, &msg);
                            })?;
                            loop {
                                if event::poll(Duration::from_millis(50))?
                                    && let Event::Key(key) = event::read()?
                                {
                                    if key.kind != KeyEventKind::Press {
                                        continue;
                                    }
                                    match key.code {
                                        KeyCode::Enter => {
                                            // Retry linking
                                            break;
                                        }
                                        KeyCode::Esc => {
                                            step = Step::Account;
                                            break;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Step::Preferences => {
                terminal.draw(|frame| {
                    draw_preferences_step(frame, &working_config);
                })?;

                if event::poll(Duration::from_millis(50))?
                    && let Event::Key(key) = event::read()?
                {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    match (key.modifiers, key.code) {
                        (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                            return Ok(SetupResult::Cancelled);
                        }
                        (_, KeyCode::Char('1')) => {
                            working_config.notify_direct = !working_config.notify_direct;
                        }
                        (_, KeyCode::Char('2')) => {
                            working_config.notify_group = !working_config.notify_group;
                        }
                        (_, KeyCode::Enter) => {
                            step = Step::Done;
                        }
                        (_, KeyCode::Esc) => {
                            // Skip preferences and proceed with defaults
                            step = Step::Done;
                        }
                        _ => {}
                    }
                }
            }

            Step::Done => {
                // Save config and finish
                working_config.save()?;

                terminal.draw(|frame| {
                    draw_done_screen(frame);
                })?;
                tokio::time::sleep(Duration::from_millis(1500)).await;

                return Ok(SetupResult::Completed(Box::new(working_config)));
            }
        }
    }
}

/// Check whether signal-cli can be invoked at `path`.
///
/// Returns `(found, display_location, resolved_path)`:
/// - `found`: whether a working invocation was discovered
/// - `display_location`: a pretty string for the wizard UI
/// - `resolved_path`: the invocation the caller should store in config
///
/// On Windows, `Command::new("foo")` only searches for `foo.exe` in PATH, while
/// `where foo` also matches `.bat`/`.cmd` via PATHEXT. That asymmetry used to
/// let the wizard report success via the `where` fallback and then fail at the
/// linking step when it re-tried the unspawnable bare name. We now verify that
/// whatever path we return can actually be spawned.
pub(crate) async fn check_signal_cli(path: &str) -> (bool, String, String) {
    // Try the path as given.
    if let Some(display) = try_spawn_version(path).await {
        return (true, display, path.to_string());
    }

    // Windows: try batch-file extensions before falling back to `where`. Rust's
    // Command invokes `.bat`/`.cmd` via cmd.exe when given the explicit extension,
    // but it does not search PATHEXT for them by bare name. Scoop and manual
    // installs commonly place `signal-cli.bat` in PATH without a matching `.exe`.
    // `.exe` is intentionally omitted: the bare-name probe above already finds
    // `.exe` via PATH, and `.ps1` is omitted because Rust's Command cannot spawn
    // PowerShell scripts directly.
    #[cfg(windows)]
    for ext in [".bat", ".cmd"] {
        let candidate = format!("{path}{ext}");
        if let Some(display) = try_spawn_version(&candidate).await {
            return (true, display, candidate);
        }
    }

    // Fallback: use `where`/`which` to resolve a full path, then verify it works.
    let which_cmd = if cfg!(windows) { "where" } else { "which" };
    if let Ok(output) = Command::new(which_cmd)
        .arg(path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        && output.status.success()
    {
        let raw = String::from_utf8_lossy(&output.stdout);
        for candidate in raw.lines() {
            let candidate = candidate.trim();
            if candidate.is_empty() {
                continue;
            }
            if let Some(display) = try_spawn_version(candidate).await {
                return (true, display, candidate.to_string());
            }
        }
    }

    (false, String::new(), path.to_string())
}

/// Try to run `<path> --version`. Returns a pretty display string on success.
async fn try_spawn_version(path: &str) -> Option<String> {
    let output = Command::new(path)
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version.is_empty() {
        Some(path.to_string())
    } else {
        Some(format!("{path} ({version})"))
    }
}

fn validate_phone(input: &str) -> Result<(), String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Phone number cannot be empty".to_string());
    }
    if !trimmed.starts_with('+') {
        return Err("Must start with + (E.164 format)".to_string());
    }
    if trimmed.len() < 8 {
        return Err("Phone number too short".to_string());
    }
    if !trimmed[1..].chars().all(|c| c.is_ascii_digit()) {
        return Err("Only digits allowed after +".to_string());
    }
    Ok(())
}

/// What a key press on the signal-cli detection step decided (#503).
#[derive(Debug, PartialEq, Eq)]
enum SignalCliStepOutcome {
    /// Stay on the step (custom-path text edited, or an ignored key).
    Continue,
    /// Ctrl-C, or Esc outside custom-path mode: cancel setup.
    Cancel,
    /// Enter outside custom-path mode: re-check for signal-cli.
    Retry,
    /// `p`: enter custom-path entry mode (buffer cleared by the handler).
    EnterCustomPath,
    /// Esc inside custom-path mode: leave it without applying.
    ExitCustomPath,
    /// Enter inside custom-path mode with a non-empty path (carries it).
    SetPath(String),
}

/// Apply a key press to the signal-cli step. `custom_path_mode` selects between
/// path-entry editing and the top-level keys. Pure: mutates only the custom-path
/// buffers and returns the transition decision (#503).
fn handle_signal_cli_key(
    custom_path_mode: bool,
    custom_path_input: &mut String,
    custom_path_cursor: &mut usize,
    modifiers: KeyModifiers,
    code: KeyCode,
) -> SignalCliStepOutcome {
    if modifiers == KeyModifiers::CONTROL && code == KeyCode::Char('c') {
        return SignalCliStepOutcome::Cancel;
    }
    if code == KeyCode::Esc {
        return if custom_path_mode {
            SignalCliStepOutcome::ExitCustomPath
        } else {
            SignalCliStepOutcome::Cancel
        };
    }
    if custom_path_mode {
        match code {
            KeyCode::Enter if !custom_path_input.is_empty() => {
                SignalCliStepOutcome::SetPath(custom_path_input.clone())
            }
            KeyCode::Backspace if *custom_path_cursor > 0 => {
                *custom_path_cursor -= 1;
                custom_path_input.remove(*custom_path_cursor);
                SignalCliStepOutcome::Continue
            }
            KeyCode::Left => {
                *custom_path_cursor = custom_path_cursor.saturating_sub(1);
                SignalCliStepOutcome::Continue
            }
            KeyCode::Right if *custom_path_cursor < custom_path_input.len() => {
                *custom_path_cursor += 1;
                SignalCliStepOutcome::Continue
            }
            KeyCode::Char(c) => {
                custom_path_input.insert(*custom_path_cursor, c);
                *custom_path_cursor += 1;
                SignalCliStepOutcome::Continue
            }
            _ => SignalCliStepOutcome::Continue,
        }
    } else {
        match code {
            KeyCode::Enter => SignalCliStepOutcome::Retry,
            KeyCode::Char('p') => {
                custom_path_input.clear();
                *custom_path_cursor = 0;
                SignalCliStepOutcome::EnterCustomPath
            }
            _ => SignalCliStepOutcome::Continue,
        }
    }
}

/// What a key press on the Account (phone-entry) step decided. Lets the pure
/// input/transition logic be unit-tested without driving a real terminal (#503).
#[derive(Debug, PartialEq, Eq)]
enum AccountStepOutcome {
    /// Stay on the step (text edited, cursor moved, or invalid submit).
    Continue,
    /// Ctrl-C: cancel setup entirely.
    Cancel,
    /// Esc: go back to the signal-cli step.
    Back,
    /// Enter with a valid phone number (carries the validated input).
    Submit(String),
}

/// Apply a key press to the Account step's phone-entry state. Pure: mutates only
/// the passed-in buffers and returns the transition decision. On `Back` it
/// clears the phone state itself; the caller resets the signal-cli step flags.
fn handle_account_key(
    phone_input: &mut String,
    phone_cursor: &mut usize,
    phone_error: &mut Option<String>,
    modifiers: KeyModifiers,
    code: KeyCode,
) -> AccountStepOutcome {
    match (modifiers, code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => AccountStepOutcome::Cancel,
        (_, KeyCode::Esc) => {
            phone_input.clear();
            *phone_cursor = 0;
            *phone_error = None;
            AccountStepOutcome::Back
        }
        (_, KeyCode::Enter) => match validate_phone(phone_input) {
            Ok(()) => {
                *phone_error = None;
                AccountStepOutcome::Submit(phone_input.clone())
            }
            Err(msg) => {
                *phone_error = Some(msg);
                AccountStepOutcome::Continue
            }
        },
        (_, KeyCode::Backspace) => {
            if *phone_cursor > 0 {
                *phone_cursor -= 1;
                phone_input.remove(*phone_cursor);
            }
            *phone_error = None;
            AccountStepOutcome::Continue
        }
        (_, KeyCode::Left) => {
            *phone_cursor = phone_cursor.saturating_sub(1);
            AccountStepOutcome::Continue
        }
        (_, KeyCode::Right) if *phone_cursor < phone_input.len() => {
            *phone_cursor += 1;
            AccountStepOutcome::Continue
        }
        (_, KeyCode::Char(c)) => {
            phone_input.insert(*phone_cursor, c);
            *phone_cursor += 1;
            *phone_error = None;
            AccountStepOutcome::Continue
        }
        _ => AccountStepOutcome::Continue,
    }
}

fn step_label(current: Step) -> &'static str {
    match current {
        Step::SignalCli => "Step 1 of 4",
        Step::Account => "Step 2 of 4",
        Step::Linking => "Step 3 of 4",
        Step::Preferences => "Step 4 of 4",
        Step::Done => "Complete",
    }
}

fn draw_signal_cli_step(
    frame: &mut ratatui::Frame,
    found: bool,
    location: &str,
    custom_path_mode: bool,
    custom_path_input: &str,
    custom_path_cursor: usize,
) {
    let area = frame.area();

    let [_, content_area, _] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(18),
        Constraint::Min(1),
    ])
    .flex(Flex::Center)
    .areas(area);

    let [content] = Layout::horizontal([Constraint::Percentage(60)])
        .flex(Flex::Center)
        .areas(content_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Setup ")
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(content);
    frame.render_widget(block, content);

    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Welcome to siggy!",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Let's get you set up.",
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}: Signal-CLI", step_label(Step::SignalCli)),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
    ];

    let mut input_line_idx: Option<usize> = None;

    if found {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                "V ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("Found signal-cli at {location}"),
                Style::default().fg(Color::Green),
            ),
        ]));
    } else if custom_path_mode {
        lines.push(Line::from(Span::styled(
            "  Enter path to signal-cli:",
            Style::default().fg(Color::Yellow),
        )));
        lines.push(Line::from(""));
        input_line_idx = Some(lines.len());
        lines.push(Line::from(vec![
            Span::styled("  > ", Style::default().fg(Color::Cyan)),
            Span::raw(custom_path_input),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Enter to confirm | Esc to go back",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                "X ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled("signal-cli not found", Style::default().fg(Color::Red)),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Install: https://github.com/AsamK/signal-cli",
            Style::default().fg(Color::Gray),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Enter to retry | p for custom path | Esc to quit",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);

    if let Some(idx) = input_line_idx {
        let cursor_x = inner.x + 4 + custom_path_cursor as u16;
        let cursor_y = inner.y + idx as u16;
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

fn draw_account_step(
    frame: &mut ratatui::Frame,
    phone_input: &str,
    phone_cursor: usize,
    error: Option<&str>,
) {
    let area = frame.area();

    let [_, content_area, _] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(16),
        Constraint::Min(1),
    ])
    .flex(Flex::Center)
    .areas(area);

    let [content] = Layout::horizontal([Constraint::Percentage(60)])
        .flex(Flex::Center)
        .areas(content_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Setup ")
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(content);
    frame.render_widget(block, content);

    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}: Phone Number", step_label(Step::Account)),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Enter your Signal phone number (E.164 format):",
            Style::default().fg(Color::Gray),
        )),
        Line::from(Span::styled(
            "  e.g. +15551234567",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
    ];

    let input_line_idx = lines.len();
    lines.push(Line::from(vec![
        Span::styled("  > ", Style::default().fg(Color::Cyan)),
        Span::raw(phone_input),
    ]));

    if let Some(err) = error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  {err}"),
            Style::default().fg(Color::Red),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Enter to confirm | Esc to go back",
        Style::default().fg(Color::DarkGray),
    )));

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);

    // Position cursor
    let cursor_x = inner.x + 4 + phone_cursor as u16;
    let cursor_y = inner.y + input_line_idx as u16;
    frame.set_cursor_position((cursor_x, cursor_y));
}

/// Path to signal-cli's `accounts.json` index, replicating signal-cli's own
/// default-location logic (`$XDG_DATA_HOME/signal-cli` or, failing that,
/// `~/.local/share/signal-cli`). siggy never passes `--config` to signal-cli,
/// so signal-cli always uses this default on every platform (including Windows,
/// where it still uses `~/.local/share`, not `%APPDATA%`). Returns `None` only
/// if the home directory cannot be resolved (#603).
fn signal_cli_accounts_path() -> Option<std::path::PathBuf> {
    let base = match std::env::var_os("XDG_DATA_HOME") {
        Some(x) if !x.is_empty() => std::path::PathBuf::from(x),
        _ => dirs::home_dir()?.join(".local").join("share"),
    };
    Some(base.join("signal-cli").join("data").join("accounts.json"))
}

/// Whether signal-cli's `accounts.json` already lists `account`. Pure so it can
/// be unit-tested without touching the filesystem; returns false for malformed
/// JSON so a parse failure never blocks linking (#603).
fn accounts_json_contains(json: &str, account: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .as_ref()
        .and_then(|v| v.get("accounts"))
        .and_then(|a| a.as_array())
        .is_some_and(|accounts| {
            accounts
                .iter()
                .any(|a| a.get("number").and_then(|n| n.as_str()) == Some(account))
        })
}

/// Whether `account` already has local signal-cli account data. Fails open: any
/// error (no home dir, missing/unreadable/malformed file) returns false so the
/// relink guard can only ever add a prompt, never block setup (#603).
pub(crate) fn account_exists_locally(account: &str) -> bool {
    signal_cli_accounts_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|contents| accounts_json_contains(&contents, account))
}

/// Run `signal-cli deleteLocalAccountData` to clear stale local data for a
/// number before relinking. `--ignore-registered` lets it proceed even when
/// signal-cli still considers the (now unlinked) account registered. Does not
/// touch the Signal servers, so it is safe for the local-conflict case (#603).
pub(crate) async fn delete_local_account_data(config: &Config) -> Result<()> {
    let status = Command::new(&config.signal_cli_path)
        .arg("-a")
        .arg(&config.account)
        .arg("deleteLocalAccountData")
        .arg("--ignore-registered")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!(
            "signal-cli deleteLocalAccountData exited with {:?}",
            status.code()
        )
    }
}

fn draw_registered_screen(frame: &mut ratatui::Frame, account: &str) {
    let area = frame.area();

    let [_, content_area, _] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(8),
        Constraint::Min(1),
    ])
    .flex(Flex::Center)
    .areas(area);

    let [content] = Layout::horizontal([Constraint::Percentage(60)])
        .flex(Flex::Center)
        .areas(content_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Green));
    let inner = block.inner(content);
    frame.render_widget(block, content);

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  V ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("Account {account} is already registered"),
                Style::default().fg(Color::Green),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Skipping device linking...",
            Style::default().fg(Color::Gray),
        )),
    ];

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

/// A centered single-line status message (e.g. shown briefly while a
/// background step runs).
fn draw_status_message(frame: &mut ratatui::Frame, message: &str) {
    let area = frame.area();
    let msg = Paragraph::new(format!("  {message}"))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Yellow));
    let [_, line, _] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .flex(Flex::Center)
    .areas(area);
    frame.render_widget(msg, line);
}

/// Confirm screen shown before linking when local signal-cli data already
/// exists for this number (#603). Offers to clear it first, since a stale local
/// registration is a common cause of a relink conflict.
fn draw_relink_confirm(frame: &mut ratatui::Frame, account: &str) {
    let area = frame.area();

    let [_, content_area, _] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(12),
        Constraint::Min(1),
    ])
    .flex(Flex::Center)
    .areas(area);

    let [content] = Layout::horizontal([Constraint::Percentage(65)])
        .flex(Flex::Center)
        .areas(content_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow))
        .title(" Existing account data ")
        .title_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(content);
    frame.render_widget(block, content);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  signal-cli already has local data for {account}."),
            Style::default().fg(Color::Yellow),
        )),
        Line::from(Span::styled(
            "  Relinking on top of it can fail with a server conflict.",
            Style::default().fg(Color::Yellow),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Reset the local data and continue linking?",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  If linking still fails afterwards, remove old devices on your",
            Style::default().fg(Color::Gray),
        )),
        Line::from(Span::styled(
            "  phone: Signal > Settings > Linked Devices.",
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  y reset and continue | n keep and continue | Esc go back",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

/// Plain-English recovery guidance for known `signal-cli link` failures, shown
/// under the raw signal-cli error in the linking-error box. Returns an empty
/// slice for unrecognised errors (the raw message is still shown above).
///
/// A 409 / "Link request error" from the Signal server survives clearing local
/// account data (the conflicting registration lives server-side), so the fix is
/// on the phone -- remove stale linked devices -- plus keeping signal-cli
/// current, not deleting local files.
fn link_error_hint(error: &str) -> &'static [&'static str] {
    let e = error.to_ascii_lowercase();
    if e.contains("409") || e.contains("link request error") {
        &[
            "This number still has a previous link registered with Signal.",
            "Clearing local data will not fix it. Instead:",
            "  1. On your phone, open Signal > Settings > Linked Devices",
            "     and remove any old or unused devices.",
            "  2. Make sure signal-cli is up to date.",
            "  3. Press Enter to retry.",
        ]
    } else {
        &[]
    }
}

fn draw_link_error(frame: &mut ratatui::Frame, error: &str) {
    let area = frame.area();

    let hint = link_error_hint(error);
    // Size the box to its content: borders + a blank + the (wrapped) error
    // + the footer, plus the hint block when present.
    let hint_rows = if hint.is_empty() { 0 } else { hint.len() + 1 };
    let content_height = ((8 + hint_rows) as u16).min(area.height.saturating_sub(2));

    let [_, content_area, _] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(content_height),
        Constraint::Min(1),
    ])
    .flex(Flex::Center)
    .areas(area);

    let [content] = Layout::horizontal([Constraint::Percentage(60)])
        .flex(Flex::Center)
        .areas(content_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Red))
        .title(" Linking Error ")
        .title_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
    let inner = block.inner(content);
    frame.render_widget(block, content);

    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {error}"),
            Style::default().fg(Color::Red),
        )),
    ];
    if !hint.is_empty() {
        lines.push(Line::from(""));
        for h in hint {
            lines.push(Line::from(Span::styled(
                format!("  {h}"),
                Style::default().fg(Color::Yellow),
            )));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Enter to retry | Esc to go back",
        Style::default().fg(Color::DarkGray),
    )));

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

fn draw_preferences_step(frame: &mut ratatui::Frame, config: &Config) {
    let area = frame.area();

    let [_, content_area, _] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(16),
        Constraint::Min(1),
    ])
    .flex(Flex::Center)
    .areas(area);

    let [content] = Layout::horizontal([Constraint::Percentage(60)])
        .flex(Flex::Center)
        .areas(content_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Setup ")
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(content);
    frame.render_widget(block, content);

    let on = Style::default().fg(Color::Green);
    let off = Style::default().fg(Color::Red);

    let direct_state = if config.notify_direct {
        ("on", on)
    } else {
        ("off", off)
    };
    let group_state = if config.notify_group {
        ("on", on)
    } else {
        ("off", off)
    };

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}: Notifications", step_label(Step::Preferences)),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Terminal bell when messages arrive in background chats.",
            Style::default().fg(Color::Gray),
        )),
        Line::from(Span::styled(
            "  You can change these later with /bell and /mute.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  1 ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Direct messages  ", Style::default().fg(Color::White)),
            Span::styled(direct_state.0, direct_state.1),
        ]),
        Line::from(vec![
            Span::styled(
                "  2 ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Group messages   ", Style::default().fg(Color::White)),
            Span::styled(group_state.0, group_state.1),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Press 1/2 to toggle | Enter/Esc to continue",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

fn draw_done_screen(frame: &mut ratatui::Frame) {
    let area = frame.area();

    let [_, content_area, _] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(8),
        Constraint::Min(1),
    ])
    .flex(Flex::Center)
    .areas(area);

    let [content] = Layout::horizontal([Constraint::Percentage(60)])
        .flex(Flex::Center)
        .areas(content_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Green));
    let inner = block.inner(content);
    frame.render_widget(block, content);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  All set! Starting siggy...",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Config saved. You can re-run setup anytime with --setup",
            Style::default().fg(Color::Gray),
        )),
    ];

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- wizard_steps (plan U5 gating hook, #640) ---

    #[test]
    fn wizard_steps_gate_binary_detection_per_backend() {
        // signal-cli drives an external binary, so detection is the first
        // step - behavior unchanged today.
        assert_eq!(
            wizard_steps(true),
            vec![
                Step::SignalCli,
                Step::Account,
                Step::Linking,
                Step::Preferences,
                Step::Done,
            ]
        );
        // An in-process engine (native, #640 U10) skips straight to the
        // account step; every other step survives.
        let native = wizard_steps(false);
        assert!(!native.contains(&Step::SignalCli));
        assert_eq!(native.first(), Some(&Step::Account));
        assert_eq!(native.len(), 4);
        // The compiled-in backend today starts at binary detection: the
        // list run_setup actually uses must lead with it.
        let compiled = wizard_steps(backend::NEEDS_CLI_BINARY);
        assert_eq!(compiled.first(), Some(&Step::SignalCli));
    }

    #[tokio::test]
    async fn check_signal_cli_detects_known_command() {
        // `cargo` is always available in our Rust test environment and supports `--version`.
        let (found, location, resolved) = check_signal_cli("cargo").await;
        assert!(found, "expected cargo to be detected");
        assert!(
            location.contains("cargo"),
            "display location should mention the binary, got: {location}"
        );
        assert_eq!(
            resolved, "cargo",
            "resolved path should equal input when the direct spawn works"
        );
    }

    #[tokio::test]
    async fn check_signal_cli_reports_missing_for_fake_command() {
        let (found, location, resolved) =
            check_signal_cli("siggy-fake-binary-does-not-exist-xyz-9999").await;
        assert!(!found, "fake command must not be detected");
        assert!(location.is_empty());
        assert_eq!(resolved, "siggy-fake-binary-does-not-exist-xyz-9999");
    }

    #[tokio::test]
    async fn try_spawn_version_returns_none_for_missing_binary() {
        assert!(
            try_spawn_version("siggy-fake-binary-does-not-exist-xyz-9999")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn try_spawn_version_returns_some_for_working_binary() {
        let display = try_spawn_version("cargo").await;
        assert!(display.is_some());
        let display = display.unwrap();
        assert!(display.starts_with("cargo"));
    }

    // --- accounts_json_contains (#603) ---

    #[test]
    fn accounts_json_contains_finds_matching_number() {
        // The shape signal-cli writes (observed: data/accounts.json, version 2).
        let json = r#"{
            "accounts": [
                { "path": "786736", "environment": "LIVE", "number": "+447588442858", "uuid": "abc" }
            ],
            "version": 2
        }"#;
        assert!(accounts_json_contains(json, "+447588442858"));
        assert!(!accounts_json_contains(json, "+15551234567"));
    }

    #[test]
    fn accounts_json_contains_handles_empty_and_missing() {
        assert!(!accounts_json_contains(
            r#"{"accounts": [], "version": 2}"#,
            "+1"
        ));
        assert!(!accounts_json_contains(r#"{"version": 2}"#, "+1"));
    }

    #[test]
    fn accounts_json_contains_fails_open_on_malformed_json() {
        // Garbage must not match (and must not panic) so a bad file never blocks
        // linking.
        assert!(!accounts_json_contains("not json at all", "+447588442858"));
        assert!(!accounts_json_contains("", "+447588442858"));
    }

    // --- link_error_hint ---

    #[test]
    fn link_error_hint_matches_409_conflict() {
        // The real signal-cli failure text the user hits after an unlink.
        let err = "signal-cli link failed (exit code: Some(3)): \
            Link request error: StatusCode: 409";
        let hint = link_error_hint(err);
        assert!(!hint.is_empty(), "a 409 must produce recovery guidance");
        assert!(
            hint.iter().any(|l| l.contains("Linked Devices")),
            "guidance should point the user at the phone's Linked Devices screen"
        );
    }

    #[test]
    fn link_error_hint_matches_link_request_error_without_code() {
        assert!(!link_error_hint("Link request error: something").is_empty());
    }

    #[test]
    fn link_error_hint_empty_for_unknown_errors() {
        assert!(link_error_hint("Timed out waiting for linking URI").is_empty());
        assert!(link_error_hint("some other failure").is_empty());
    }

    // --- validate_phone (#503) ---

    #[test]
    fn validate_phone_accepts_e164() {
        assert!(validate_phone("+15551234567").is_ok());
        // Leading/trailing whitespace is trimmed before validation.
        assert!(validate_phone("  +15551234567  ").is_ok());
        // Minimum accepted length is 8 chars including the '+'.
        assert!(validate_phone("+1234567").is_ok());
    }

    #[test]
    fn validate_phone_rejects_empty() {
        assert_eq!(
            validate_phone("   ").unwrap_err(),
            "Phone number cannot be empty"
        );
    }

    #[test]
    fn validate_phone_requires_plus_prefix() {
        assert_eq!(
            validate_phone("15551234567").unwrap_err(),
            "Must start with + (E.164 format)"
        );
    }

    #[test]
    fn validate_phone_rejects_too_short() {
        assert_eq!(
            validate_phone("+123").unwrap_err(),
            "Phone number too short"
        );
    }

    #[test]
    fn validate_phone_rejects_non_digits() {
        assert_eq!(
            validate_phone("+1555abc4567").unwrap_err(),
            "Only digits allowed after +"
        );
        // A second '+' after the first is also a non-digit.
        assert_eq!(
            validate_phone("+1555+234567").unwrap_err(),
            "Only digits allowed after +"
        );
    }

    // --- handle_account_key (#503) ---

    fn account_key(
        input: &str,
        cursor: usize,
        modifiers: KeyModifiers,
        code: KeyCode,
    ) -> (AccountStepOutcome, String, usize, Option<String>) {
        let mut phone = input.to_string();
        let mut cur = cursor;
        let mut err: Option<String> = Some("stale".to_string());
        let outcome = handle_account_key(&mut phone, &mut cur, &mut err, modifiers, code);
        (outcome, phone, cur, err)
    }

    #[test]
    fn account_key_typing_inserts_and_clears_error() {
        let (outcome, phone, cur, err) =
            account_key("+1", 2, KeyModifiers::NONE, KeyCode::Char('5'));
        assert_eq!(outcome, AccountStepOutcome::Continue);
        assert_eq!(phone, "+15");
        assert_eq!(cur, 3);
        assert_eq!(err, None);
    }

    #[test]
    fn account_key_inserts_at_cursor() {
        let (_, phone, cur, _) = account_key("+19", 1, KeyModifiers::NONE, KeyCode::Char('5'));
        assert_eq!(phone, "+519");
        assert_eq!(cur, 2);
    }

    #[test]
    fn account_key_backspace_removes_before_cursor() {
        let (outcome, phone, cur, err) =
            account_key("+159", 4, KeyModifiers::NONE, KeyCode::Backspace);
        assert_eq!(outcome, AccountStepOutcome::Continue);
        assert_eq!(phone, "+15");
        assert_eq!(cur, 3);
        assert_eq!(err, None);
        // Backspace at the start is a no-op.
        let (_, phone, cur, _) = account_key("+15", 0, KeyModifiers::NONE, KeyCode::Backspace);
        assert_eq!(phone, "+15");
        assert_eq!(cur, 0);
    }

    #[test]
    fn account_key_cursor_nav_is_bounded() {
        let (_, _, cur, _) = account_key("+15", 0, KeyModifiers::NONE, KeyCode::Left);
        assert_eq!(cur, 0, "left saturates at 0");
        let (_, _, cur, _) = account_key("+15", 3, KeyModifiers::NONE, KeyCode::Right);
        assert_eq!(cur, 3, "right past the end is a no-op");
        let (_, _, cur, _) = account_key("+15", 1, KeyModifiers::NONE, KeyCode::Right);
        assert_eq!(cur, 2);
    }

    #[test]
    fn account_key_esc_goes_back_and_clears_state() {
        let (outcome, phone, cur, err) =
            account_key("+15551234567", 5, KeyModifiers::NONE, KeyCode::Esc);
        assert_eq!(outcome, AccountStepOutcome::Back);
        assert_eq!(phone, "");
        assert_eq!(cur, 0);
        assert_eq!(err, None);
    }

    #[test]
    fn account_key_ctrl_c_cancels() {
        let (outcome, _, _, _) =
            account_key("+15551234567", 0, KeyModifiers::CONTROL, KeyCode::Char('c'));
        assert_eq!(outcome, AccountStepOutcome::Cancel);
    }

    #[test]
    fn account_key_enter_invalid_sets_error_and_stays() {
        let (outcome, phone, _, err) = account_key("123", 3, KeyModifiers::NONE, KeyCode::Enter);
        assert_eq!(outcome, AccountStepOutcome::Continue);
        assert_eq!(phone, "123", "input is preserved on invalid submit");
        assert_eq!(err.as_deref(), Some("Must start with + (E.164 format)"));
    }

    #[test]
    fn account_key_enter_valid_submits() {
        let (outcome, _, _, err) =
            account_key("+15551234567", 12, KeyModifiers::NONE, KeyCode::Enter);
        assert_eq!(
            outcome,
            AccountStepOutcome::Submit("+15551234567".to_string())
        );
        assert_eq!(err, None);
    }

    // --- handle_signal_cli_key (#503) ---

    fn signal_cli_key(
        custom_mode: bool,
        input: &str,
        cursor: usize,
        modifiers: KeyModifiers,
        code: KeyCode,
    ) -> (SignalCliStepOutcome, String, usize) {
        let mut buf = input.to_string();
        let mut cur = cursor;
        let outcome = handle_signal_cli_key(custom_mode, &mut buf, &mut cur, modifiers, code);
        (outcome, buf, cur)
    }

    #[test]
    fn signal_cli_key_ctrl_c_cancels_in_either_mode() {
        let (o, _, _) = signal_cli_key(false, "", 0, KeyModifiers::CONTROL, KeyCode::Char('c'));
        assert_eq!(o, SignalCliStepOutcome::Cancel);
        let (o, _, _) = signal_cli_key(true, "x", 1, KeyModifiers::CONTROL, KeyCode::Char('c'));
        assert_eq!(o, SignalCliStepOutcome::Cancel);
    }

    #[test]
    fn signal_cli_key_esc_cancels_normal_but_exits_custom() {
        let (o, _, _) = signal_cli_key(false, "", 0, KeyModifiers::NONE, KeyCode::Esc);
        assert_eq!(o, SignalCliStepOutcome::Cancel);
        let (o, _, _) = signal_cli_key(true, "x", 1, KeyModifiers::NONE, KeyCode::Esc);
        assert_eq!(o, SignalCliStepOutcome::ExitCustomPath);
    }

    #[test]
    fn signal_cli_key_normal_enter_retries_and_p_enters_custom() {
        let (o, _, _) = signal_cli_key(false, "", 0, KeyModifiers::NONE, KeyCode::Enter);
        assert_eq!(o, SignalCliStepOutcome::Retry);
        // 'p' enters custom mode and clears the buffer.
        let (o, buf, cur) =
            signal_cli_key(false, "stale", 5, KeyModifiers::NONE, KeyCode::Char('p'));
        assert_eq!(o, SignalCliStepOutcome::EnterCustomPath);
        assert_eq!(buf, "");
        assert_eq!(cur, 0);
        // Other normal-mode chars are ignored.
        let (o, _, _) = signal_cli_key(false, "", 0, KeyModifiers::NONE, KeyCode::Char('x'));
        assert_eq!(o, SignalCliStepOutcome::Continue);
    }

    #[test]
    fn signal_cli_key_custom_path_editing() {
        // Typing inserts at the cursor.
        let (o, buf, cur) = signal_cli_key(true, "/usr", 4, KeyModifiers::NONE, KeyCode::Char('/'));
        assert_eq!(o, SignalCliStepOutcome::Continue);
        assert_eq!(buf, "/usr/");
        assert_eq!(cur, 5);
        // Backspace removes before the cursor; no-op at start.
        let (_, buf, cur) = signal_cli_key(true, "/usr", 4, KeyModifiers::NONE, KeyCode::Backspace);
        assert_eq!((buf.as_str(), cur), ("/us", 3));
        let (_, buf, cur) = signal_cli_key(true, "/usr", 0, KeyModifiers::NONE, KeyCode::Backspace);
        assert_eq!((buf.as_str(), cur), ("/usr", 0));
        // Cursor nav is bounded.
        let (_, _, cur) = signal_cli_key(true, "/usr", 0, KeyModifiers::NONE, KeyCode::Left);
        assert_eq!(cur, 0);
        let (_, _, cur) = signal_cli_key(true, "/usr", 4, KeyModifiers::NONE, KeyCode::Right);
        assert_eq!(cur, 4);
    }

    #[test]
    fn signal_cli_key_custom_enter_sets_path_only_when_nonempty() {
        let (o, _, _) = signal_cli_key(
            true,
            "/opt/signal-cli",
            0,
            KeyModifiers::NONE,
            KeyCode::Enter,
        );
        assert_eq!(
            o,
            SignalCliStepOutcome::SetPath("/opt/signal-cli".to_string())
        );
        // Empty input: Enter is ignored (stays on the step).
        let (o, _, _) = signal_cli_key(true, "", 0, KeyModifiers::NONE, KeyCode::Enter);
        assert_eq!(o, SignalCliStepOutcome::Continue);
    }
}
