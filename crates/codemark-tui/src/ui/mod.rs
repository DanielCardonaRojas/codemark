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

/// Default relevance for a binding that doesn't set one explicitly.
pub const DEFAULT_BINDING_PRIORITY: u8 = 50;

/// A key binding for the status bar.
///
/// Bindings carry a `priority` used to rank them by relevance when the status
/// bar can't fit them all. Two values are sentinels: [`HIDDEN_BINDING_PRIORITY`]
/// (never shown) and [`ALWAYS_SHOW_BINDING_PRIORITY`] (always shown regardless
/// of space, e.g. `?` to open the full help popup).
#[derive(Debug, Clone)]
pub struct KeyBinding {
    pub key: String,
    pub description: String,
    /// Higher values are considered more relevant and shown first.
    pub priority: u8,
}

impl KeyBinding {
    pub fn new(key: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            description: description.into(),
            priority: DEFAULT_BINDING_PRIORITY,
        }
    }

    /// Set the relevance priority (higher = shown first when space is tight).
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }

    /// Mark this binding as always shown, even when space is limited.
    ///
    /// Shorthand for the [`ALWAYS_SHOW_BINDING_PRIORITY`] sentinel.
    pub fn pinned(mut self) -> Self {
        self.priority = ALWAYS_SHOW_BINDING_PRIORITY;
        self
    }

    /// Display width of this binding rendered as `description: key`.
    fn display_width(&self) -> usize {
        use unicode_width::UnicodeWidthStr;
        // "description" + ": " + "key"
        self.description.width() + 2 + self.key.width()
    }
}

/// Width of the " | " separator drawn between bindings.
const SEPARATOR_WIDTH: usize = 3;

/// Display width of the " …" overflow hint appended when bindings are dropped.
const OVERFLOW_HINT_WIDTH: usize = 2;

/// Priority value marking a binding as hidden from the status bar entirely.
///
/// Used for universally understood keys (e.g. Enter) that don't need to take up
/// status-bar space. They still appear in the full help popup.
pub const HIDDEN_BINDING_PRIORITY: u8 = 0;

/// Priority value marking a binding as always shown, regardless of space.
///
/// Used for the `?` Help binding, which is the entry point to the full help
/// popup, so it must never be dropped.
pub const ALWAYS_SHOW_BINDING_PRIORITY: u8 = u8::MAX;

/// Select which bindings to show given the available display width.
///
/// Priority acts as both a ranking and two sentinels:
/// [`HIDDEN_BINDING_PRIORITY`] bindings are never shown (and don't count as
/// "dropped"), while [`ALWAYS_SHOW_BINDING_PRIORITY`] bindings are reserved
/// first so they are never dropped. The remaining bindings are added in
/// descending priority order until the next one would not fit. The returned
/// bindings preserve the input order so the status bar layout stays stable.
/// Returns the selected bindings and whether any were dropped for lack of space.
fn select_bindings(bindings: &[KeyBinding], available_width: usize) -> (Vec<&KeyBinding>, bool) {
    // Indices of always-shown bindings (kept first) and the rest (ranked).
    let mut chosen: Vec<usize> = Vec::with_capacity(bindings.len());
    let mut used_width = 0usize;
    let mut count = 0usize;

    // Helper closure to compute the width cost of adding one more binding.
    let cost = |used: usize, count: usize, b: &KeyBinding| -> usize {
        let sep = if count == 0 { 0 } else { SEPARATOR_WIDTH };
        used + sep + b.display_width()
    };

    // 1. Reserve space for always-shown bindings first (they cannot be dropped).
    for (i, b) in bindings.iter().enumerate() {
        if b.priority == ALWAYS_SHOW_BINDING_PRIORITY {
            used_width = cost(used_width, count, b);
            count += 1;
            chosen.push(i);
        }
    }

    // 2. Add remaining bindings by descending priority, then input order.
    //    Hidden (priority 0) and already-reserved always-shown bindings are excluded.
    let mut ranked: Vec<usize> = bindings
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            b.priority > HIDDEN_BINDING_PRIORITY && b.priority < ALWAYS_SHOW_BINDING_PRIORITY
        })
        .map(|(i, _)| i)
        .collect();
    ranked.sort_by(|&a, &b| bindings[b].priority.cmp(&bindings[a].priority).then(a.cmp(&b)));

    let mut dropped = false;
    for i in ranked {
        let next = cost(used_width, count, &bindings[i]);
        if next <= available_width {
            used_width = next;
            count += 1;
            chosen.push(i);
        } else {
            dropped = true;
        }
    }

    // Restore input order for stable rendering.
    chosen.sort_unstable();
    (chosen.into_iter().map(|i| &bindings[i]).collect(), dropped)
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

    // Reserve only as much space for the metadata as it actually needs, so the
    // keybindings can use the rest of the row. Each side gets 1 column of inner
    // horizontal padding from the `Margin` applied below.
    let right_width = right_text.as_ref().map(|m| m.width() as u16 + 2).unwrap_or(0);
    // Don't let metadata consume more than half the row on narrow terminals.
    let right_width = right_width.min(area.width / 2);

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),              // Keybindings
            Constraint::Length(right_width), // Metadata (right side)
        ])
        .split(area);

    // 1. Keybindings (Left side), ranked by relevance to fit the space.
    let left_inner = chunks[0].inner(Margin { horizontal: 1, vertical: 0 });
    let full_width = left_inner.width as usize;
    // Try the full width first. If anything was dropped we'll show a " …" hint,
    // so re-select against a budget that reserves room for it — that way the
    // hint never overflows, but we don't waste those columns when everything
    // already fits.
    let (mut selected, mut dropped) = select_bindings(bindings, full_width);
    if dropped {
        let (s, d) = select_bindings(bindings, full_width.saturating_sub(OVERFLOW_HINT_WIDTH));
        selected = s;
        dropped = d;
    }

    let mut left_spans = Vec::new();
    for (i, binding) in selected.iter().enumerate() {
        if i > 0 {
            left_spans.push(Span::styled(" | ", Style::default().fg(crate::theme::palette().gray)));
        }
        left_spans.push(Span::styled(
            &binding.description,
            Style::default().fg(crate::theme::palette().emphasis),
        ));
        left_spans.push(Span::raw(": "));
        left_spans.push(Span::styled(
            &binding.key,
            Style::default().fg(crate::theme::palette().accent).bold(),
        ));
    }

    // Hint that more bindings exist (use `?` to see the full list).
    if dropped {
        left_spans.push(Span::styled(" …", Style::default().fg(crate::theme::palette().gray)));
    }

    let left_text = Paragraph::new(Line::from(left_spans))
        .alignment(Alignment::Left)
        .style(Style::default().bg(crate::theme::palette().dim));

    left_text.render(left_inner, buf);

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
    let text = Line::from(vec![
        Span::styled(prompt, Style::default().fg(crate::theme::palette().accent)),
        Span::raw(input),
    ]);

    let paragraph = Paragraph::new(text)
        .style(Style::default().fg(crate::theme::palette().emphasis))
        .wrap(Wrap { trim: false });

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
            Span::styled(
                format!(" {:10} ", key),
                Style::default().fg(crate::theme::palette().accent).bold(),
            ),
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
/// Used for confirming destructive actions. The dialog is centered over the
/// given area with a titled border and a `y`/`n` prompt.
pub fn render_confirmation(area: Rect, buf: &mut Buffer, title: &str, message: &str) {
    // Calculate dialog dimensions (centered, 60% of width, max 60 chars)
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
                .title(title.to_string())
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

    let paragraph = Paragraph::new(text)
        .style(style.bg(crate::theme::palette().dim))
        .alignment(Alignment::Left);

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

    fn keys(selected: &[&KeyBinding]) -> Vec<String> {
        selected.iter().map(|b| b.key.clone()).collect()
    }

    #[test]
    fn select_bindings_keeps_all_when_space_is_ample() {
        let bindings = vec![
            KeyBinding::new("Enter", "Select").with_priority(95),
            KeyBinding::new("/", "Filter").with_priority(60),
            KeyBinding::new("?", "Help").pinned(),
        ];
        let (selected, dropped) = select_bindings(&bindings, 1000);
        assert_eq!(selected.len(), 3);
        assert!(!dropped);
    }

    #[test]
    fn select_bindings_always_keeps_pinned_even_at_tiny_width() {
        let bindings = vec![
            KeyBinding::new("Enter", "Select Step").with_priority(95),
            KeyBinding::new("o", "Open File").with_priority(85),
            KeyBinding::new("?", "Help").with_priority(65).pinned(),
        ];
        // Width of 1 cannot fit anything, but the pinned binding must survive.
        let (selected, dropped) = select_bindings(&bindings, 1);
        assert_eq!(keys(&selected), vec!["?"]);
        assert!(dropped);
    }

    #[test]
    fn select_bindings_drops_lowest_priority_first() {
        let bindings = vec![
            KeyBinding::new("Enter", "Select").with_priority(95),
            KeyBinding::new("o", "Open").with_priority(85),
            KeyBinding::new("+/-", "Resize").with_priority(20),
            KeyBinding::new("?", "Help").pinned(),
        ];
        // Enough room for the pinned binding plus the two highest-priority ones,
        // but not the low-priority "Resize".
        // pinned "Help: ?" = 4+2+1 = 7
        // "Select: Enter" = 6+2+5 = 13 (+3 sep)
        // "Open: o" = 4+2+1 = 7 (+3 sep)
        let width = 7 + 3 + 13 + 3 + 7;
        let (selected, dropped) = select_bindings(&bindings, width);
        assert!(dropped);
        assert!(selected.iter().any(|b| b.key == "?"));
        assert!(selected.iter().any(|b| b.key == "Enter"));
        assert!(selected.iter().any(|b| b.key == "o"));
        assert!(!selected.iter().any(|b| b.key == "+/-"));
    }

    #[test]
    fn select_bindings_hides_priority_zero_without_marking_dropped() {
        let bindings = vec![
            KeyBinding::new("Enter", "Select").with_priority(HIDDEN_BINDING_PRIORITY),
            KeyBinding::new("o", "Open").with_priority(85),
            KeyBinding::new("?", "Help").pinned(),
        ];
        let (selected, dropped) = select_bindings(&bindings, 1000);
        assert_eq!(keys(&selected), vec!["o", "?"]);
        // Everything that *can* show is showing, so nothing was dropped for space.
        assert!(!dropped);
    }

    #[test]
    fn select_bindings_preserves_input_order() {
        let bindings = vec![
            KeyBinding::new("Enter", "Select").with_priority(95),
            KeyBinding::new("/", "Filter").with_priority(60),
            KeyBinding::new("?", "Help").with_priority(65).pinned(),
        ];
        let (selected, _) = select_bindings(&bindings, 1000);
        assert_eq!(keys(&selected), vec!["Enter", "/", "?"]);
    }
}
