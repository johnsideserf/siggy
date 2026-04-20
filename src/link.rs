use std::io;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Flex, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt};
use tokio::process::Command;

use crate::config::Config;

/// Result of a device-linking flow.
pub enum LinkResult {
    /// Device was linked successfully.
    Success,
    /// User cancelled the linking (Esc / Ctrl+C).
    Cancelled,
}

/// Check whether the configured account is registered with signal-cli.
/// Returns `Ok(true)` if registered, `Ok(false)` if not.
pub async fn check_account_registered(config: &Config) -> Result<bool> {
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let output = Command::new(&config.signal_cli_path)
            .arg("-a")
            .arg(&config.account)
            .arg("listContacts")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    anyhow::anyhow!(
                        "'{}' not found. Is signal-cli installed and in your PATH?",
                        config.signal_cli_path
                    )
                } else {
                    anyhow::anyhow!("Failed to run '{}': {}", config.signal_cli_path, e)
                }
            })?;
        Ok::<bool, anyhow::Error>(output.success())
    })
    .await;

    match result {
        Ok(inner) => inner,
        Err(_) => Ok(false), // Timeout — treat as unregistered
    }
}

/// Run the interactive device-linking flow: spawn `signal-cli link`, capture the
/// linking URI, display a QR code in the TUI, and wait for completion or cancellation.
pub async fn run_linking_flow(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: &Config,
) -> Result<LinkResult> {
    // Show initial status
    terminal.draw(|frame| {
        let msg = Paragraph::new("Starting device linking...")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Yellow));
        let area = centered_rect(50, 3, frame.area());
        frame.render_widget(msg, area);
    })?;

    // Spawn signal-cli link
    let mut child = Command::new(&config.signal_cli_path)
        .arg("link")
        .arg("-n")
        .arg("siggy")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!(
                    "'{}' not found. Is signal-cli installed and in your PATH?",
                    config.signal_cli_path
                )
            } else {
                anyhow::anyhow!("Failed to start '{}': {}", config.signal_cli_path, e)
            }
        })?;

    let stdout = child
        .stdout
        .take()
        .context("No stdout from signal-cli link")?;
    let mut reader = tokio::io::BufReader::new(stdout).lines();

    // Read lines until we find the linking URI
    let uri = loop {
        let line = tokio::time::timeout(Duration::from_secs(30), reader.next_line()).await;
        match line {
            Ok(Ok(Some(l))) => {
                let trimmed = l.trim().to_string();
                if trimmed.starts_with("tsdevice:") || trimmed.starts_with("sgnl:") {
                    break trimmed;
                }
            }
            Ok(Ok(None)) => {
                // stdout closed without URI — read stderr for details
                let mut stderr_output = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    let _ = stderr.read_to_string(&mut stderr_output).await;
                }
                let detail = stderr_output.trim();
                if detail.is_empty() {
                    anyhow::bail!("signal-cli link exited without producing a linking URI");
                } else {
                    anyhow::bail!("signal-cli link failed: {detail}");
                }
            }
            Ok(Err(e)) => {
                anyhow::bail!("Error reading signal-cli link output: {e}");
            }
            Err(_) => {
                let _ = child.kill().await;
                anyhow::bail!("Timed out waiting for linking URI from signal-cli");
            }
        }
    };

    // Generate QR code
    let qr = qrcode::QrCode::new(uri.as_bytes()).context("Failed to generate QR code")?;
    let qr_lines = render_qr_lines(&qr);

    // Show QR and wait for linking to complete or user to cancel
    show_qr_and_wait(terminal, &qr_lines, &mut child).await
}

/// Convert a QR code matrix into half-block text lines.
/// Uses Unicode half-block characters to pack two QR rows into one terminal row.
fn render_qr_lines(qr: &qrcode::QrCode) -> Vec<Line<'static>> {
    let width = qr.width();
    let colors = qr.to_colors();

    // Add a 2-module quiet zone on each side
    let quiet = 2;
    let total_w = width + quiet * 2;
    let total_h = width + quiet * 2;

    // Build a padded grid (true = dark)
    let mut grid = vec![vec![false; total_w]; total_h];
    for row in 0..width {
        for col in 0..width {
            grid[row + quiet][col + quiet] = colors[row * width + col] == qrcode::Color::Dark;
        }
    }

    let mut lines = Vec::new();

    // Process two rows at a time
    let mut y = 0;
    while y < total_h {
        let mut spans = Vec::new();
        for (x, &top) in grid[y].iter().enumerate() {
            let bottom = if y + 1 < total_h {
                grid[y + 1][x]
            } else {
                false
            };

            let (ch, fg, bg) = match (top, bottom) {
                (true, true) => ('\u{2588}', Color::Black, Color::Reset),
                (true, false) => ('\u{2580}', Color::Black, Color::White),
                (false, true) => ('\u{2584}', Color::Black, Color::White),
                (false, false) => (' ', Color::White, Color::White),
            };
            spans.push(Span::styled(ch.to_string(), Style::default().fg(fg).bg(bg)));
        }
        lines.push(Line::from(spans));
        y += 2;
    }

    lines
}

/// Display the QR code screen and wait for the child process to finish (link success)
/// or for the user to press Esc/Ctrl+C to cancel.
async fn show_qr_and_wait(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    qr_lines: &[Line<'static>],
    child: &mut tokio::process::Child,
) -> Result<LinkResult> {
    loop {
        // Draw
        terminal.draw(|frame| draw_qr_screen(frame, qr_lines))?;

        // Check if the child process finished (non-blocking)
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    // Show success briefly
                    terminal.draw(|frame| {
                        let msg = Paragraph::new("Device linked successfully!")
                            .alignment(Alignment::Center)
                            .style(Style::default().fg(Color::Green));
                        let area = centered_rect(50, 3, frame.area());
                        frame.render_widget(msg, area);
                    })?;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    return Ok(LinkResult::Success);
                } else {
                    anyhow::bail!("signal-cli link failed (exit code: {:?})", status.code());
                }
            }
            Ok(None) => {} // Still running
            Err(e) => anyhow::bail!("Error checking signal-cli link status: {e}"),
        }

        // Poll for keyboard input
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match (key.modifiers, key.code) {
                (_, KeyCode::Esc) | (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                    let _ = child.kill().await;
                    return Ok(LinkResult::Cancelled);
                }
                _ => {}
            }
        }
    }
}

/// Draw the full QR code screen with title, centered QR, and instructions.
fn draw_qr_screen(frame: &mut ratatui::Frame, qr_lines: &[Line<'static>]) {
    let area = frame.area();
    let qr_height = qr_lines.len() as u16;
    let qr_width = qr_lines.first().map_or(0, |l| l.width()) as u16;

    // Check if terminal is too small
    if area.width < qr_width + 4 || area.height < qr_height + 8 {
        let msg =
            Paragraph::new("Terminal too small to display QR code.\nPlease resize your terminal.")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Red));
        let msg_area = centered_rect(60, 4, area);
        frame.render_widget(msg, msg_area);
        return;
    }

    // Vertical layout: title, qr, instructions
    let [_, title_area, _, qr_area, _, instr_area, _] = Layout::vertical([
        Constraint::Min(1),                // top padding
        Constraint::Length(3),             // title
        Constraint::Length(1),             // gap
        Constraint::Length(qr_height + 2), // qr + border
        Constraint::Length(1),             // gap
        Constraint::Length(5),             // instructions
        Constraint::Min(1),                // bottom padding
    ])
    .flex(Flex::Center)
    .areas(area);

    // Title
    let title = Paragraph::new("Link Device")
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Cyan));
    frame.render_widget(title, title_area);

    // QR code in a centered block
    let qr_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let qr_paragraph = Paragraph::new(qr_lines.to_vec())
        .alignment(Alignment::Center)
        .block(qr_block);

    // Center the QR horizontally
    let [qr_centered] = Layout::horizontal([Constraint::Length(qr_width + 2)])
        .flex(Flex::Center)
        .areas(qr_area);
    frame.render_widget(qr_paragraph, qr_centered);

    // Instructions
    let instructions = Paragraph::new(vec![
        Line::from("Scan this QR code with Signal on your phone"),
        Line::from(""),
        Line::from(Span::styled(
            "Settings > Linked Devices > Link New Device",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Press Esc or Ctrl+C to cancel",
            Style::default().fg(Color::DarkGray),
        )),
    ])
    .alignment(Alignment::Center);
    frame.render_widget(instructions, instr_area);
}

/// Helper to create a centered rect of given percentage width and fixed height.
fn centered_rect(
    percent_x: u16,
    height: u16,
    area: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    let [centered] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [centered] = Layout::horizontal([Constraint::Percentage(percent_x)])
        .flex(Flex::Center)
        .areas(centered);
    centered
}
