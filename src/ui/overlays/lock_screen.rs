//! Full-frame lock screen overlay.
//!
//! When `app.lock.is_locked()`, `ui::draw` short-circuits to call this
//! renderer instead of the normal chat surface. Conversation names,
//! message content, and contact info are never drawn while the lock
//! screen is up.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::app::App;
use crate::domain::LockPhase;

pub(in crate::ui) fn draw_lock_screen(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(Clear, area);

    let theme = &app.theme;

    // Vertical layout: top spacer / title / prompt / error / bottom spacer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(0),
        ])
        .split(area);

    let title = Paragraph::new(Line::from(vec![Span::styled(
        "siggy - locked",
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )]))
    .alignment(Alignment::Center);
    frame.render_widget(title, chunks[1]);

    let prompt_label = match app.lock.phase {
        LockPhase::Unlocked => "", // not reached
        LockPhase::SetPassphrase => "Set passphrase",
        LockPhase::LockEntry => "Passphrase",
        LockPhase::ChangePassphraseOld => "Current passphrase",
        LockPhase::ChangePassphraseNew => "New passphrase",
    };
    let masked: String = "*".repeat(app.lock.input_buffer.chars().count());
    let prompt_text = format!("{prompt_label}: {masked}");

    let prompt = Paragraph::new(Line::from(Span::styled(
        prompt_text,
        Style::default().fg(theme.fg),
    )))
    .alignment(Alignment::Center)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent)),
    );
    frame.render_widget(prompt, chunks[2]);

    if let Some(err) = &app.lock.error {
        let err_widget = Paragraph::new(Line::from(Span::styled(
            err.clone(),
            Style::default()
                .fg(theme.fg_muted)
                .add_modifier(Modifier::ITALIC),
        )))
        .alignment(Alignment::Center);
        frame.render_widget(err_widget, chunks[3]);
    }
}
