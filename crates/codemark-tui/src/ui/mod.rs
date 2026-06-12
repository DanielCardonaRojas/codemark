//! High-level UI rendering utilities.
//!
//! This module provides utilities for rendering the UI, including
//! the main frame rendering, status bars, and other common UI elements.

use ratatui::{
    Frame, Terminal,
    backend::Backend,
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Clear, Paragraph, Widget, Wrap},
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
    terminal.draw(|f| {
        render_fn(f, state);
    })?;
    Ok(())
}

/// A key binding for the status bar.
#[derive(Debug, Clone)]
pub struct KeyBinding {
    pub key: String,
    pub description: String,
}

impl KeyBinding {
    pub fn new(key: impl Into<String>, description: impl Into<String>) -> Self {
        Self { key: key.into(), description: description.into() }
    }
}

/// Render a status bar at the bottom of the screen.
///
/// The status bar shows mode, context-aware keybindings on the left,
/// and active filters/metadata on the right.
pub fn render_status_bar(
    area: Rect,
    buf: &mut Buffer,
    mode: AppMode,
    bindings: &[KeyBinding],
    right_text: Option<Line>,
    search_query: Option<&str>,
) {
    // Fill background
    buf.set_style(area, Style::default().bg(crate::theme::palette().dim));

    if mode == AppMode::Search {
        let query = search_query.unwrap_or("");
        let text = Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(crate::theme::palette().accent).bold()),
            Span::styled(query, Style::default().fg(crate::theme::palette().emphasis)),
        ]);
        let para = Paragraph::new(text).style(Style::default().bg(crate::theme::palette().dim));
        para.render(area.inner(Margin { horizontal: 1, vertical: 0 }), buf);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),         // Keybindings
            Constraint::Percentage(50), // Metadata (right side)
        ])
        .split(area);

    // 1. Keybindings (Left side)
    let mut left_spans = Vec::new();
    for (i, binding) in bindings.iter().enumerate() {
        if i > 0 {
            left_spans.push(Span::styled(" | ", Style::default().fg(crate::theme::palette().gray)));
        }
        left_spans.push(Span::styled(&binding.description, Style::default().fg(crate::theme::palette().emphasis)));
        left_spans.push(Span::raw(": "));
        left_spans.push(Span::styled(
            &binding.key,
            Style::default().fg(crate::theme::palette().accent).bold(),
        ));
    }

    let left_text = Paragraph::new(Line::from(left_spans))
        .alignment(Alignment::Left)
        .style(Style::default().bg(crate::theme::palette().dim));

    // Add some padding
    let left_area = chunks[0].inner(Margin { horizontal: 1, vertical: 0 });
    left_text.render(left_area, buf);

    // 2. Metadata (Right side)
    if let Some(meta) = right_text {
        let right_para = Paragraph::new(meta)
            .alignment(Alignment::Right)
            .style(Style::default().bg(crate::theme::palette().dim));

        let right_area = chunks[1].inner(Margin { horizontal: 1, vertical: 0 });
        right_para.render(right_area, buf);
    }
}

/// Render a command line input area.
///
/// Used for command mode and search mode.
pub fn render_command_line(
    area: Rect,
    buf: &mut Buffer,
    prompt: &str,
    input: &str,
    _cursor_pos: usize,
) {
    let text =
        Line::from(vec![Span::styled(prompt, Style::default().fg(crate::theme::palette().accent)), Span::raw(input)]);

    let paragraph =
        Paragraph::new(text).style(Style::default().fg(crate::theme::palette().emphasis)).wrap(Wrap { trim: false });

    paragraph.render(area, buf);

    // Note: Actual cursor positioning is handled by the terminal backend
    // This is a simplified version
}

/// Render a help panel with key bindings.
///
/// Displays a table of key bindings and their actions.
pub fn render_help_panel(area: Rect, buf: &mut Buffer, bindings: &[(&str, &str)]) {
    // Clear the background to avoid overlap with border
    Widget::render(Clear, area, buf);

    let mut text = Text::default();

    for (key, action) in bindings {
        text.push_line(Line::from(vec![
            Span::styled(format!(" {:10} ", key), Style::default().fg(crate::theme::palette().accent).bold()),
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
    let height = area.height.min(8);

    let dialog_area = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };

    // Clear the background to avoid overlap
    Widget::render(Clear, dialog_area, buf);

    let text = Text::from(vec![
        Line::from(""),
        Line::from(vec![Span::styled(message, Style::default().bold())]),
        Line::from(""),
        Line::from(vec![
            Span::raw("Press "),
            Span::styled("y", Style::default().fg(crate::theme::palette().success).bold()),
            Span::raw(" to confirm, "),
            Span::styled("n", Style::default().fg(crate::theme::palette().error).bold()),
            Span::raw(" to cancel"),
        ]),
    ]);

    let paragraph = Paragraph::new(text)
        .block(
            Block::bordered()
                .title("Confirm")
                .title_style(Style::default().bold())
                .border_style(Style::default().fg(crate::theme::palette().warning)),
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

    // Clear the background to avoid overlap
    Widget::render(Clear, popup_area, buf);

    let paragraph = Paragraph::new(content.clone())
        .block(
            Block::bordered()
                .title(title)
                .title_style(Style::default().bold())
                .border_style(Style::default().fg(crate::theme::palette().emphasis)),
        )
        .wrap(Wrap { trim: false });

    paragraph.render(popup_area, buf);
}

/// Render a notification/toast message.
///
/// Notifications appear briefly at the bottom or top of the screen.
pub fn render_notification(
    area: Rect,
    buf: &mut Buffer,
    message: &str,
    notification_type: NotificationType,
) {
    let (style, icon) = match notification_type {
        NotificationType::Info => (Style::default().fg(crate::theme::palette().info), ""),
        NotificationType::Success => (Style::default().fg(crate::theme::palette().success), "✓ "),
        NotificationType::Warning => (Style::default().fg(crate::theme::palette().warning), "! "),
        NotificationType::Error => (Style::default().fg(crate::theme::palette().error), "✗ "),
    };

    let text = Line::from(vec![Span::styled(icon, style), Span::styled(message, style)]);

    let paragraph =
        Paragraph::new(text).style(style.bg(crate::theme::palette().dim)).alignment(Alignment::Left);

    // Render at the bottom of the area
    let notification_area =
        Rect { x: area.x, y: area.bottom().saturating_sub(1), width: area.width, height: 1 };

    Widget::render(Clear, notification_area, buf);
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
        Span::styled(spinner_char, Style::default().fg(crate::theme::palette().accent)),
        Span::raw(" "),
        Span::styled(message, Style::default().fg(crate::theme::palette().emphasis)),
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
    let bar_style = Style::default().bg(crate::theme::palette().dim);
    buf.set_style(area, bar_style);

    // Draw the filled portion
    if filled_width > 0 {
        let filled_area = Rect { x: area.x, y: area.y, width: filled_width, height: area.height };
        let filled_style = Style::default().bg(crate::theme::palette().success);
        buf.set_style(filled_area, filled_style);
    }

    // Draw the text overlay
    let text_line = Line::from(text);
    let paragraph = Paragraph::new(text_line)
        .alignment(Alignment::Center)
        .style(Style::default().fg(crate::theme::palette().emphasis).bold());

    paragraph.render(area, buf);
}

/// Create a bordered frame with rounded corners.
///
/// Returns a Rect representing the inner area of the frame.
pub fn bordered_frame(area: Rect, buf: &mut Buffer, title: Option<&str>) -> Rect {
    let block = if let Some(t) = title { Block::bordered().title(t) } else { Block::bordered() };

    block.render(area, buf);

    // Return the inner area (excluding borders)
    area.inner(Margin { horizontal: 1, vertical: 1 })
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
