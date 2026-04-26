//! Main settings overlay + the Customize sub-overlay it spawns.
//!
//! Section-grouped (Notifications / Display / Messages / Interface)
//! list of toggles plus three "special" rows: notification preview
//! mode, image protocol mode, and the entry into the Customize
//! sub-overlay. Each row reads its current value from `App` and
//! shows a one-line hint for the focused setting at the bottom.
//!
//! `draw_customize` is the three-row launcher (Theme / Keybindings /
//! Settings profile) reached from the Settings overlay's
//! "Customize..." row. It lives here rather than in its own file
//! because Settings is its only caller.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::{SETTINGS_POPUP_HEIGHT, SETTINGS_POPUP_WIDTH, centered_popup};
use crate::app::{
    App, SETTINGS, SETTINGS_SECTION_DISPLAY, SETTINGS_SECTION_INTERFACE, SETTINGS_SECTION_MESSAGES,
    SettingDef,
};

pub(in crate::ui) fn draw_settings(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let height = SETTINGS_POPUP_HEIGHT;
    let (popup_area, block) = centered_popup(
        frame,
        area,
        SETTINGS_POPUP_WIDTH,
        height,
        " Settings ",
        theme,
    );

    let header_style = Style::default()
        .fg(theme.fg_muted)
        .add_modifier(Modifier::BOLD);

    // Render a toggle row
    let render_toggle = |lines: &mut Vec<Line>, i: usize, def: &SettingDef| {
        let enabled = app.setting_value(i);
        let checkbox = if enabled { "[x]" } else { "[ ]" };
        let is_selected = i == app.settings_index;
        let style = if is_selected {
            Style::default()
                .bg(theme.bg_selected)
                .fg(theme.fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg_secondary)
        };
        let check_style = if is_selected {
            Style::default()
                .bg(theme.bg_selected)
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else if enabled {
            Style::default().fg(theme.success)
        } else {
            Style::default().fg(theme.fg_muted)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("    {} ", checkbox), check_style),
            Span::styled(def.label.to_string(), style),
        ]));
    };

    // Render a special (non-toggle) row
    let render_special = |lines: &mut Vec<Line>, label: &str, value: &str, index: usize| {
        let is_selected = app.settings_index == index;
        let label_style = if is_selected {
            Style::default()
                .bg(theme.bg_selected)
                .fg(theme.fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg_secondary)
        };
        let value_style = if is_selected {
            Style::default().bg(theme.bg_selected).fg(theme.accent)
        } else {
            Style::default().fg(theme.accent)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("    {label}"), label_style),
            Span::styled(value.to_string(), value_style),
        ]));
    };

    let preview_index = SETTINGS.len();
    let image_mode_index = SETTINGS.len() + 1;
    let customize_index = SETTINGS.len() + 2;

    let mut lines: Vec<Line> = Vec::new();

    // — Notifications —
    lines.push(Line::from(Span::styled("  Notifications", header_style)));
    for (i, def) in SETTINGS.iter().enumerate().take(SETTINGS_SECTION_DISPLAY) {
        render_toggle(&mut lines, i, def);
    }
    render_special(
        &mut lines,
        "Notification preview: ",
        &app.notifications.notification_preview,
        preview_index,
    );

    // — Display —
    lines.push(Line::from(Span::styled("  Display", header_style)));
    for (i, def) in SETTINGS
        .iter()
        .enumerate()
        .take(SETTINGS_SECTION_MESSAGES)
        .skip(SETTINGS_SECTION_DISPLAY)
    {
        render_toggle(&mut lines, i, def);
    }
    render_special(
        &mut lines,
        "Image mode: ",
        &app.image.image_mode,
        image_mode_index,
    );

    // — Messages —
    lines.push(Line::from(Span::styled("  Messages", header_style)));
    for (i, def) in SETTINGS
        .iter()
        .enumerate()
        .take(SETTINGS_SECTION_INTERFACE)
        .skip(SETTINGS_SECTION_MESSAGES)
    {
        render_toggle(&mut lines, i, def);
    }

    // — Interface —
    lines.push(Line::from(Span::styled("  Interface", header_style)));
    for (i, def) in SETTINGS.iter().enumerate().skip(SETTINGS_SECTION_INTERFACE) {
        render_toggle(&mut lines, i, def);
    }
    render_special(&mut lines, "Customize...", "", customize_index);

    // Hint line for the currently selected item
    let hint = if app.settings_index < SETTINGS.len() {
        SETTINGS[app.settings_index].hint
    } else {
        match app.settings_index - SETTINGS.len() {
            0 => "Control message content in notifications",
            1 => "native (terminal protocol), halfblock, or none",
            2 => "Theme, keybindings, and settings profiles",
            _ => "",
        }
    };
    lines.push(Line::from(Span::styled(
        format!("  {hint}"),
        Style::default()
            .fg(theme.fg_muted)
            .add_modifier(Modifier::ITALIC),
    )));

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}

pub(in crate::ui) fn draw_customize(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let items = ["Theme", "Keybindings", "Settings profile"];
    let (popup_area, block) = centered_popup(frame, area, 30, 5, " Customize ", theme);

    let mut lines: Vec<Line> = Vec::new();
    for (i, label) in items.iter().enumerate() {
        let is_selected = i == app.customize_index;
        let style = if is_selected {
            Style::default()
                .bg(theme.bg_selected)
                .fg(theme.fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg_secondary)
        };
        lines.push(Line::from(Span::styled(format!("  {label}"), style)));
    }

    let popup = Paragraph::new(lines).block(block);
    frame.render_widget(popup, popup_area);
}
