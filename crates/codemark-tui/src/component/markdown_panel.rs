//! A component for rendering simple Markdown-like text for bookmark info.

use crate::component::Component;
use crate::event::Event;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget},
};

/// A component that renders simple markdown-style text.
#[derive(Debug, Clone, Default)]
pub struct MarkdownPanel {
    /// The content to display
    content: String,
    /// Vertical scroll offset
    scroll_offset: u16,
    /// Whether the component is focused
    focused: bool,
}

impl MarkdownPanel {
    /// Create a new markdown panel.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the content.
    pub fn set_markdown(&mut self, content: impl Into<String>) {
        self.content = content.into();
        self.scroll_offset = 0;
    }

    /// Convert the simple markdown string into Ratatui Text.
    fn parse_to_text(&self) -> Text<'_> {
        let mut lines = Vec::new();

        for line in self.content.lines() {
            if let Some(stripped) = line.strip_prefix("# ") {
                // H1
                lines.push(Line::from(vec![Span::styled(
                    stripped,
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                )]));
                lines.push(Line::from(""));
            } else if let Some(stripped) = line.strip_prefix("## ") {
                // H2
                lines.push(Line::from(vec![Span::styled(
                    stripped,
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                )]));
            } else if let Some(stripped) = line.strip_prefix("> ") {
                // Blockquote
                lines.push(Line::from(vec![
                    Span::styled("┃ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        stripped,
                        Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC),
                    ),
                ]));
            } else if let Some(stripped) = line.strip_prefix("- ") {
                // List item
                lines.push(Line::from(vec![
                    Span::styled("• ", Style::default().fg(Color::Green)),
                    Span::raw(stripped),
                ]));
            } else if line.starts_with('|') {
                // Table line (simple parsing)
                if line.contains("---") {
                    // Ignore separator
                    continue;
                }
                let parts: Vec<&str> = line.split('|').filter(|s| !s.is_empty()).collect();
                let mut spans = Vec::new();
                for (i, part) in parts.iter().enumerate() {
                    let text = part.trim();
                    if i == 0 {
                        // Key
                        let clean_key = text.trim_matches('*');
                        spans.push(Span::styled(
                            format!("{:<15}", clean_key),
                            Style::default().fg(Color::DarkGray),
                        ));
                    } else {
                        // Value
                        spans.push(Span::raw(text));
                    }
                }
                lines.push(Line::from(spans));
            } else {
                // Normal text
                lines.push(Line::from(line));
            }
        }

        Text::from(lines)
    }
}

impl Component for MarkdownPanel {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let text = self.parse_to_text();
        let paragraph = Paragraph::new(text).scroll((self.scroll_offset, 0));

        paragraph.render(area, buf);
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        if !self.focused {
            return false;
        }

        if let Event::Key(key) = event {
            match key.code {
                ratatui::crossterm::event::KeyCode::Down
                | ratatui::crossterm::event::KeyCode::Char('j') => {
                    self.scroll_offset = self.scroll_offset.saturating_add(1);
                    true
                }
                ratatui::crossterm::event::KeyCode::Up
                | ratatui::crossterm::event::KeyCode::Char('k') => {
                    self.scroll_offset = self.scroll_offset.saturating_sub(1);
                    true
                }
                ratatui::crossterm::event::KeyCode::Char('J') => {
                    self.scroll_offset = self.scroll_offset.saturating_add(5);
                    true
                }
                ratatui::crossterm::event::KeyCode::Char('K') => {
                    self.scroll_offset = self.scroll_offset.saturating_sub(5);
                    true
                }
                _ => false,
            }
        } else {
            false
        }
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
    }
}
