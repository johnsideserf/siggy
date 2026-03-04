use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Flex, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Terminal,
};
use tokio::process::Command;

use crate::config::Config;
use crate::link;

pub enum SetupResult {
    /// Wizard finished successfully, use this config.
    Completed(Config),
    /// User had a valid config, no setup needed.
    Skipped,
    /// User cancelled during setup.
    Cancelled,
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    SignalCli,
    Account,
    Linking,
    Preferences,
    Done,
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
    let mut step = Step::SignalCli;
    let mut signal_cli_path = working_config.signal_cli_path.clone();
    let mut phone_input = String::new();
    let mut phone_cursor: usize = 0;
    let mut phone_error: Option<String> = None;
    let mut signal_cli_found = false;
    let mut signal_cli_location = String::new();
    let mut custom_path_mode = false;
    let mut custom_path_input = String::new();
    let mut custom_path_cursor: usize = 0;

    loop {
        match step {
            Step::SignalCli => {
                // Check for signal-cli
                if !signal_cli_found {
                    let (found, location) = check_signal_cli(&signal_cli_path).await;
                    signal_cli_found = found;
                    signal_cli_location = location;
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
                if event::poll(Duration::from_millis(50))? {
                    if let Event::Key(key) = event::read()? {
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }
                        match (key.modifiers, key.code) {
                            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                                return Ok(SetupResult::Cancelled);
                            }
                            (_, KeyCode::Esc) if custom_path_mode => {
                                custom_path_mode = false;
                            }
                            (_, KeyCode::Esc) => {
                                return Ok(SetupResult::Cancelled);
                            }
                            _ if custom_path_mode => match key.code {
                                KeyCode::Enter => {
                                    if !custom_path_input.is_empty() {
                                        signal_cli_path = custom_path_input.clone();
                                        signal_cli_found = false;
                                        custom_path_mode = false;
                                        // Will re-check on next loop
                                    }
                                }
                                KeyCode::Backspace => {
                                    if custom_path_cursor > 0 {
                                        custom_path_cursor -= 1;
                                        custom_path_input.remove(custom_path_cursor);
                                    }
                                }
                                KeyCode::Left => {
                                    custom_path_cursor = custom_path_cursor.saturating_sub(1);
                                }
                                KeyCode::Right => {
                                    if custom_path_cursor < custom_path_input.len() {
                                        custom_path_cursor += 1;
                                    }
                                }
                                KeyCode::Char(c) => {
                                    custom_path_input.insert(custom_path_cursor, c);
                                    custom_path_cursor += 1;
                                }
                                _ => {}
                            },
                            (_, KeyCode::Enter) => {
                                // Retry check
                                signal_cli_found = false;
                            }
                            (_, KeyCode::Char('p')) => {
                                // Enter custom path mode
                                custom_path_mode = true;
                                custom_path_input.clear();
                                custom_path_cursor = 0;
                            }
                            _ => {}
                        }
                    }
                }
            }

            Step::Account => {
                terminal.draw(|frame| {
                    draw_account_step(frame, &phone_input, phone_cursor, phone_error.as_deref());
                })?;

                if event::poll(Duration::from_millis(50))? {
                    if let Event::Key(key) = event::read()? {
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }
                        match (key.modifiers, key.code) {
                            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                                return Ok(SetupResult::Cancelled);
                            }
                            (_, KeyCode::Esc) => {
                                step = Step::SignalCli;
                                signal_cli_found = false;
                                custom_path_mode = false;
                                phone_input.clear();
                                phone_cursor = 0;
                                phone_error = None;
                            }
                            (_, KeyCode::Enter) => {
                                match validate_phone(&phone_input) {
                                    Ok(()) => {
                                        working_config.account = phone_input.clone();
                                        phone_error = None;
                                        step = Step::Linking;
                                    }
                                    Err(msg) => {
                                        phone_error = Some(msg);
                                    }
                                }
                            }
                            (_, KeyCode::Backspace) => {
                                if phone_cursor > 0 {
                                    phone_cursor -= 1;
                                    phone_input.remove(phone_cursor);
                                }
                                phone_error = None;
                            }
                            (_, KeyCode::Left) => {
                                phone_cursor = phone_cursor.saturating_sub(1);
                            }
                            (_, KeyCode::Right) => {
                                if phone_cursor < phone_input.len() {
                                    phone_cursor += 1;
                                }
                            }
                            (_, KeyCode::Char(c)) => {
                                phone_input.insert(phone_cursor, c);
                                phone_cursor += 1;
                                phone_error = None;
                            }
                            _ => {}
                        }
                    }
                }
            }

            Step::Linking => {
                // Check if already registered
                let registered = link::check_account_registered(&working_config).await.unwrap_or(false);
                if registered {
                    // Already registered, skip linking
                    terminal.draw(|frame| {
                        draw_registered_screen(frame, &working_config.account);
                    })?;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    step = Step::Preferences;
                    continue;
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
                                if event::poll(Duration::from_millis(50))? {
                                    if let Event::Key(key) = event::read()? {
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
            }

            Step::Preferences => {
                terminal.draw(|frame| {
                    draw_preferences_step(frame, &working_config);
                })?;

                if event::poll(Duration::from_millis(50))? {
                    if let Event::Key(key) = event::read()? {
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
            }

            Step::Done => {
                // Save config and finish
                working_config.save()?;

                terminal.draw(|frame| {
                    draw_done_screen(frame);
                })?;
                tokio::time::sleep(Duration::from_millis(1500)).await;

                return Ok(SetupResult::Completed(working_config));
            }
        }
    }
}

async fn check_signal_cli(path: &str) -> (bool, String) {
    // Try running the command to see if it exists
    let which_cmd = if cfg!(windows) { "where" } else { "which" };

    match Command::new(path)
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await
    {
        Ok(output) => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if output.status.success() && !version.is_empty() {
                (true, format!("{path} ({version})"))
            } else {
                // Command exists but no version info — resolve full path
                let location = Command::new(which_cmd)
                    .arg(path)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .output()
                    .await
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_else(|| path.to_string());
                (true, location)
            }
        }
        Err(_) => {
            // Command failed to run — try `which` / `where` to find it
            match Command::new(which_cmd)
                .arg(path)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .output()
                .await
            {
                Ok(output) if output.status.success() => {
                    let location = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    (true, location)
                }
                _ => (false, String::new()),
            }
        }
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
        .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    let inner = block.inner(content);
    frame.render_widget(block, content);

    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Welcome to siggy!",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Let's get you set up.",
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}: Signal-CLI", step_label(Step::SignalCli)),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
    ];

    let mut input_line_idx: Option<usize> = None;

    if found {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled("V ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
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
            Span::styled("X ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::styled(
                "signal-cli not found",
                Style::default().fg(Color::Red),
            ),
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
        .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    let inner = block.inner(content);
    frame.render_widget(block, content);

    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}: Phone Number", step_label(Step::Account)),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
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
            Span::styled("  V ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
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

fn draw_link_error(frame: &mut ratatui::Frame, error: &str) {
    let area = frame.area();

    let [_, content_area, _] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(10),
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

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {error}"),
            Style::default().fg(Color::Red),
        )),
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            "  Enter to retry | Esc to go back",
            Style::default().fg(Color::DarkGray),
        )),
    ];

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
        .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    let inner = block.inner(content);
    frame.render_widget(block, content);

    let on = Style::default().fg(Color::Green);
    let off = Style::default().fg(Color::Red);

    let direct_state = if config.notify_direct { ("on", on) } else { ("off", off) };
    let group_state = if config.notify_group { ("on", on) } else { ("off", off) };

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}: Notifications", step_label(Step::Preferences)),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
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
            Span::styled("  1 ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled("Direct messages  ", Style::default().fg(Color::White)),
            Span::styled(direct_state.0, direct_state.1),
        ]),
        Line::from(vec![
            Span::styled("  2 ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
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
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
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
