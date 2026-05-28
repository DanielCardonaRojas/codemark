//! A component for rendering simple Markdown-like text for bookmark info.

use crate::component::Component;
use crate::event::Event;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget, Wrap},
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
    /// Last rendered area
    last_area: std::cell::Cell<Rect>,
    /// Cached parsed text to avoid re-parsing on every frame
    cached_text: std::cell::RefCell<Text<'static>>,
    /// Cached content hash to detect when content changes
    cached_content_hash: std::cell::Cell<u64>,
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
        // Invalidate cache by setting hash to 0
        self.cached_content_hash.set(0);
    }

    /// Get the current markdown content.
    pub fn markdown(&self) -> &str {
        &self.content
    }

    /// Convert the simple markdown string into Ratatui Text.
    fn parse_to_text(&self) -> Text<'static> {
        let mut lines = Vec::new();

        for line in self.content.lines() {
            if let Some(stripped) = line.strip_prefix("# ") {
                // H1
                lines.push(Line::from(vec![Span::styled(
                    stripped.to_string(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                )]));
                lines.push(Line::from(""));
            } else if let Some(stripped) = line.strip_prefix("## ") {
                // H2
                lines.push(Line::from(vec![Span::styled(
                    stripped.to_string(),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                )]));
            } else if let Some(stripped) = line.strip_prefix("> ") {
                // Blockquote
                let mut spans = vec![Span::styled("┃ ", Style::default().fg(Color::DarkGray))];
                spans.extend(self.parse_inline(
                    stripped,
                    Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC),
                ));
                lines.push(Line::from(spans));
            } else if let Some(stripped) = line.strip_prefix("- ") {
                // List item
                let mut spans = vec![Span::styled("• ", Style::default().fg(Color::Green))];
                spans.extend(self.parse_inline(stripped, Style::default()));
                lines.push(Line::from(spans));
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
                        spans.extend(self.parse_inline(text, Style::default()));
                        if i < parts.len() - 1 {
                            spans.push(Span::raw(" "));
                        }
                    }
                }
                lines.push(Line::from(spans));
            } else {
                // Normal text
                lines.push(Line::from(self.parse_inline(line, Style::default())));
            }
        }

        Text::from(lines)
    }

    /// Refresh the cached text if content has changed.
    fn refresh_cache(&self) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // Calculate hash of current content
        let mut hasher = DefaultHasher::new();
        self.content.hash(&mut hasher);
        let current_hash = hasher.finish();

        // If content changed, re-parse and cache
        if self.cached_content_hash.get() != current_hash {
            let text = self.parse_to_text();
            *self.cached_text.borrow_mut() = text;
            self.cached_content_hash.set(current_hash);
        }
    }

    /// Get the rendered line count, accounting for line wrapping.
    /// Uses the known viewport width to compute per-line wrap counts.
    fn line_count(&self) -> usize {
        self.refresh_cache();
        let width = self.last_area.get().width as usize;
        if width == 0 {
            return self.cached_text.borrow().lines.len();
        }
        self.cached_text
            .borrow()
            .lines
            .iter()
            .map(|l| {
                let char_count: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
                if char_count == 0 { 1 } else { char_count.div_ceil(width) }
            })
            .sum()
    }

    /// Parse inline formatting like `code` and **bold**.
    fn parse_inline(&self, text: &str, base_style: Style) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        let mut current = text;

        while !current.is_empty() {
            // Find next occurrence of ` or **
            let next_code = current.find('`');
            let next_bold = current.find("**");

            match (next_code, next_bold) {
                (Some(i), Some(j)) if i < j => {
                    // Handle code first
                    if i > 0 {
                        spans.push(Span::styled(current[..i].to_string(), base_style));
                    }
                    current = &current[i + 1..];
                    if let Some(end) = current.find('`') {
                        spans.push(Span::styled(
                            current[..end].to_string(),
                            base_style.fg(Color::Yellow),
                        ));
                        current = &current[end + 1..];
                    } else {
                        spans.push(Span::styled("`".to_string(), base_style));
                    }
                }
                (_, Some(j)) => {
                    // Handle bold
                    if j > 0 {
                        spans.push(Span::styled(current[..j].to_string(), base_style));
                    }
                    current = &current[j + 2..];
                    if let Some(end) = current.find("**") {
                        spans.push(Span::styled(
                            current[..end].to_string(),
                            base_style.add_modifier(Modifier::BOLD),
                        ));
                        current = &current[end + 2..];
                    } else {
                        spans.push(Span::styled("**".to_string(), base_style));
                    }
                }
                (Some(i), _) => {
                    // Handle code
                    if i > 0 {
                        spans.push(Span::styled(current[..i].to_string(), base_style));
                    }
                    current = &current[i + 1..];
                    if let Some(end) = current.find('`') {
                        spans.push(Span::styled(
                            current[..end].to_string(),
                            base_style.fg(Color::Yellow),
                        ));
                        current = &current[end + 1..];
                    } else {
                        spans.push(Span::styled("`".to_string(), base_style));
                    }
                }
                (None, None) => {
                    spans.push(Span::styled(current.to_string(), base_style));
                    break;
                }
            }
        }

        spans
    }
}

impl Component for MarkdownPanel {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.last_area.set(area);
        self.refresh_cache();
        let cached = self.cached_text.borrow().clone();
        let paragraph =
            Paragraph::new(cached).wrap(Wrap { trim: false }).scroll((self.scroll_offset, 0));

        paragraph.render(area, buf);
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(key) => {
                if !self.focused {
                    return false;
                }
                match key.code {
                    ratatui::crossterm::event::KeyCode::Down
                    | ratatui::crossterm::event::KeyCode::Char('j') => {
                        let height = self.last_area.get().height as usize;
                        let line_count = self.line_count();
                        if line_count > height {
                            let old_offset = self.scroll_offset;
                            self.scroll_offset = self
                                .scroll_offset
                                .saturating_add(1)
                                .min((line_count - height) as u16);
                            return old_offset != self.scroll_offset;
                        }
                        false
                    }
                    ratatui::crossterm::event::KeyCode::Up
                    | ratatui::crossterm::event::KeyCode::Char('k') => {
                        let old_offset = self.scroll_offset;
                        self.scroll_offset = self.scroll_offset.saturating_sub(1);
                        old_offset != self.scroll_offset
                    }
                    ratatui::crossterm::event::KeyCode::Char('J') => {
                        let height = self.last_area.get().height as usize;
                        let line_count = self.line_count();
                        if line_count > height {
                            let old_offset = self.scroll_offset;
                            self.scroll_offset = self
                                .scroll_offset
                                .saturating_add(5)
                                .min((line_count - height) as u16);
                            return old_offset != self.scroll_offset;
                        }
                        false
                    }
                    ratatui::crossterm::event::KeyCode::Char('K') => {
                        let old_offset = self.scroll_offset;
                        self.scroll_offset = self.scroll_offset.saturating_sub(5);
                        old_offset != self.scroll_offset
                    }
                    _ => false,
                }
            }
            Event::Mouse(mouse) => {
                let area = self.last_area.get();
                let is_hovered =
                    area.contains(ratatui::layout::Position::from((mouse.column, mouse.row)));

                match mouse.kind {
                    ratatui::crossterm::event::MouseEventKind::ScrollDown if is_hovered => {
                        let height = area.height as usize;
                        let line_count = self.line_count();
                        if line_count > height {
                            let old_offset = self.scroll_offset;
                            self.scroll_offset = self
                                .scroll_offset
                                .saturating_add(1)
                                .min((line_count - height) as u16);
                            return old_offset != self.scroll_offset;
                        }
                        false
                    }
                    ratatui::crossterm::event::MouseEventKind::ScrollUp if is_hovered => {
                        let old_offset = self.scroll_offset;
                        self.scroll_offset = self.scroll_offset.saturating_sub(1);
                        old_offset != self.scroll_offset
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
    }
}
