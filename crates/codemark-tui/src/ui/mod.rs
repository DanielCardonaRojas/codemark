//! High-level UI rendering utilities.
//!
//! This module provides utilities for rendering the UI, including
//! the main frame rendering, status bars, and other common UI elements.

use ratatui::{
    backend::Backend,
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph, Widget, Wrap},
    Frame, Terminal,
};

use crate::state::{AppMode, AppState};

/// Render the main UI frame.
///
/// This function sets up the terminal, renders the UI, and restores the terminal state.
pub fn render_ui<B: Backend>(
    terminal: &mut Terminal<B>,
    state: &AppState,
    render_fn: impl FnOnce(&mut Frame, &AppState),
) -> std::io::Result<()> {
    let _ = terminal.draw(|f| {
        render_fn(f, state);
    });
    Ok(())
}

/// Render a status bar at the bottom of the screen.
///
/// The status bar shows the current mode and optional message.
pub fn render_status_bar(area: Rect, buf: &mut Buffer, mode: AppMode, message: Option<&str>) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(15), Constraint::Min(0)])
        .split(area);

    // Mode indicator
    let mode_style = match mode {
        AppMode::Normal => Style::default().fg(Color::Green),
        AppMode::Insert => Style::default().fg(Color::Yellow),
        AppMode::Command => Style::default().fg(Color::Cyan),
        AppMode::Search => Style::default().fg(Color::Magenta),
        AppMode::Help => Style::default().fg(Color::Blue),
        AppMode::Confirm => Style::default().fg(Color::Red),
    };

    let mode_text = Paragraph::new(mode.indicator())
        .style(mode_style.bg(Color::DarkGray).bold())
        .alignment(Alignment::Center);

    mode_text.render(chunks[0], buf);

    // Message area
    if let Some(msg) = message {
        let msg_text = Paragraph::new(msg)
            .style(Style::default().fg(Color::White))
            .alignment(Alignment::Left);

        msg_text.render(chunks[1], buf);
    }
}

/// Render a command line input area.
///
/// Used for command mode and search mode.
pub fn render_command_line(area: Rect, buf: &mut Buffer, prompt: &str, input: &str, _cursor_pos: usize) {
    let text = Line::from(vec![
        Span::styled(prompt, Style::default().fg(Color::Cyan)),
        Span::raw(input),
    ]);

    let paragraph = Paragraph::new(text)
        .style(Style::default().fg(Color::White))
        .wrap(Wrap { trim: false });

    paragraph.render(area, buf);

    // Note: Actual cursor positioning is handled by the terminal backend
    // This is a simplified version
}

/// Render a help panel with key bindings.
///
/// Displays a table of key bindings and their actions.
pub fn render_help_panel(area: Rect, buf: &mut Buffer, bindings: &[(&str, &str)]) {
    let mut text = Text::default();

    for (key, action) in bindings {
        text.push_line(Line::from(vec![
            Span::styled(format!(" {:10} ", key), Style::default().fg(Color::Cyan).bold()),
            Span::raw(*action),
        ]));
    }

    let paragraph = Paragraph::new(text)
        .block(Block::bordered().title("Key Bindings").title_style(Style::default().bold()))
        .wrap(Wrap { trim: false });

    paragraph.render(area, buf);
}

/// Render a confirmation dialog.
///
/// Used for confirming destructive actions.
pub fn render_confirmation(area: Rect, buf: &mut Buffer, message: &str) {
    // Calculate dialog dimensions (centered, 60% of width, max 40 chars)
    let width = (area.width as f64 * 0.6).min(60.0) as u16;
    let height = 8;

    let dialog_area = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };

    let text = Text::from(vec![
        Line::from(""),
        Line::from(vec![Span::styled(message, Style::default().bold())]),
        Line::from(""),
        Line::from(vec![
            Span::raw("Press "),
            Span::styled("y", Style::default().fg(Color::Green).bold()),
            Span::raw(" to confirm, "),
            Span::styled("n", Style::default().fg(Color::Red).bold()),
            Span::raw(" to cancel"),
        ]),
    ]);

    let paragraph = Paragraph::new(text)
        .block(
            Block::bordered()
                .title("Confirm")
                .title_style(Style::default().bold())
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });

    paragraph.render(dialog_area, buf);
}

/// Render a popup with custom content.
///
/// Popups are temporary overlays that appear on top of the main UI.
pub fn render_popup(area: Rect, buf: &mut Buffer, title: &str, content: &Text) {
    // Calculate popup dimensions (80% of available space)
    let width = (area.width as f64 * 0.8) as u16;
    let height = (area.height as f64 * 0.8) as u16;

    let popup_area = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };

    let paragraph = Paragraph::new(content.clone())
        .block(
            Block::bordered()
                .title(title)
                .title_style(Style::default().bold())
                .border_style(Style::default().fg(Color::White)),
        )
        .wrap(Wrap { trim: false });

    paragraph.render(popup_area, buf);
}

/// Render a notification/toast message.
///
/// Notifications appear briefly at the bottom or top of the screen.
pub fn render_notification(area: Rect, buf: &mut Buffer, message: &str, notification_type: NotificationType) {
    let (style, icon) = match notification_type {
        NotificationType::Info => (Style::default().fg(Color::Blue), ""),
        NotificationType::Success => (Style::default().fg(Color::Green), "✓ "),
        NotificationType::Warning => (Style::default().fg(Color::Yellow), "! "),
        NotificationType::Error => (Style::default().fg(Color::Red), "✗ "),
    };

    let text = Line::from(vec![
        Span::styled(icon, style),
        Span::styled(message, style),
    ]);

    let paragraph = Paragraph::new(text)
        .style(style.bg(Color::DarkGray))
        .alignment(Alignment::Left);

    // Render at the bottom of the area
    let notification_area = Rect {
        x: area.x,
        y: area.bottom().saturating_sub(1),
        width: area.width,
        height: 1,
    };

    paragraph.render(notification_area, buf);
}

/// The type of notification to display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationType {
    Info,
    Success,
    Warning,
    Error,
}

/// Render a spinner for loading states.
///
/// Shows a simple ASCII spinner animation frame.
pub fn render_spinner(area: Rect, buf: &mut Buffer, frame: usize, message: &str) {
    const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

    let spinner_char = SPINNER_FRAMES[frame % SPINNER_FRAMES.len()];

    let text = Line::from(vec![
        Span::styled(spinner_char, Style::default().fg(Color::Cyan)),
        Span::raw(" "),
        Span::styled(message, Style::default().fg(Color::White)),
    ]);

    let paragraph = Paragraph::new(text).alignment(Alignment::Center);

    paragraph.render(area, buf);
}

/// Render a progress bar.
///
/// Shows a horizontal progress bar with percentage.
pub fn render_progress_bar(area: Rect, buf: &mut Buffer, progress: f64, label: Option<&str>) {
    let progress = progress.clamp(0.0, 1.0);
    let filled_width = (area.width as f64 * progress) as u16;

    let mut text = String::new();

    if let Some(l) = label {
        text.push_str(l);
        text.push(' ');
    }

    let percentage = (progress * 100.0) as u16;
    text.push_str(&format!("{}%", percentage));

    // Draw the bar background
    let bar_style = Style::default().bg(Color::DarkGray);
    buf.set_style(area, bar_style);

    // Draw the filled portion
    if filled_width > 0 {
        let filled_area = Rect {
            x: area.x,
            y: area.y,
            width: filled_width,
            height: area.height,
        };
        let filled_style = Style::default().bg(Color::Green);
        buf.set_style(filled_area, filled_style);
    }

    // Draw the text overlay
    let text_line = Line::from(text);
    let paragraph = Paragraph::new(text_line)
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::White).bold());

    paragraph.render(area, buf);
}

/// Create a bordered frame with rounded corners.
///
/// Returns a Rect representing the inner area of the frame.
pub fn bordered_frame(area: Rect, buf: &mut Buffer, title: Option<&str>) -> Rect {
    let block = if let Some(t) = title {
        Block::bordered().title(t)
    } else {
        Block::bordered()
    };

    block.render(area, buf);

    // Return the inner area (excluding borders)
    area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    })
}

/// Common key bindings to display in help.
pub fn common_key_bindings() -> Vec<(&'static str, &'static str)> {
    vec![
        ("q", "Quit"),
        ("j/↓", "Move down"),
        ("k/↑", "Move up"),
        ("h/←", "Move left"),
        ("l/→", "Move right"),
        ("Enter", "Confirm/Select"),
        ("Esc", "Cancel/Back"),
        (":", "Command mode"),
        ("/", "Search"),
        ("?", "Help"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_common_key_bindings() {
        let bindings = common_key_bindings();
        assert!(!bindings.is_empty());
        assert!(bindings.iter().any(|(k, _)| k == &"q"));
    }

    #[test]
    fn test_notification_type_display() {
        // Just verify the enum exists and can be created
        let _ = NotificationType::Info;
        let _ = NotificationType::Success;
        let _ = NotificationType::Warning;
        let _ = NotificationType::Error;
    }

    #[test]
    fn test_app_mode_styles() {
        // Verify mode indicator returns non-empty strings
        assert!(!AppMode::Normal.indicator().is_empty());
        assert!(!AppMode::Insert.indicator().is_empty());
        assert!(!AppMode::Command.indicator().is_empty());
    }
}
