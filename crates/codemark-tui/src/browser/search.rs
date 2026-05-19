//! Search bar component for filtering content.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{BorderType, Paragraph, Widget},
};

use crate::component::Component;
use crate::event::Event;

/// A search bar component for text input and filtering.
#[derive(Debug, Clone)]
pub struct SearchBar {
    /// The current search query
    query: String,
    /// Placeholder text when empty
    placeholder: String,
    /// Whether the search bar is focused
    focused: bool,
    /// Cursor position
    cursor: usize,
    /// Current filter mode
    mode: SearchMode,
    /// Last rendered area
    last_area: std::cell::Cell<Rect>,
}

/// Search mode for filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// Full-text search using SQLite FTS
    Fts,
    /// Semantic search using embeddings
    Semantic,
}

impl SearchBar {
    /// Create a new search bar.
    pub fn new() -> Self {
        Self {
            query: String::new(),
            placeholder: "Search...".to_string(),
            focused: false,
            cursor: 0,
            mode: SearchMode::Fts,
            last_area: std::cell::Cell::new(Rect::default()),
        }
    }

    /// Get the last rendered area.
    pub fn last_area(&self) -> Rect {
        self.last_area.get()
    }

    /// Get the current search query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Set the search query.
    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
        self.cursor = self.query.chars().count();
    }

    /// Clear the search query.
    pub fn clear(&mut self) {
        self.query.clear();
        self.cursor = 0;
    }

    /// Get the search mode.
    pub fn mode(&self) -> SearchMode {
        self.mode
    }

    /// Set the search mode.
    pub fn set_mode(&mut self, mode: SearchMode) {
        self.mode = mode;
    }

    /// Cycle through search modes.
    pub fn cycle_mode(&mut self) {
        self.mode = match self.mode {
            SearchMode::Fts => SearchMode::Semantic,
            SearchMode::Semantic => SearchMode::Fts,
        };
    }

    /// Render the search bar content.
    fn render_content(&self, area: Rect, buf: &mut Buffer) {
        let width = area.width as usize;

        // 1. Build the segmented control for the right side
        let normal_style = Style::default().fg(Color::DarkGray);
        let active_style = if self.focused {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        };

        let control_spans = vec![
            Span::styled("[ ", normal_style),
            Span::styled(
                "FTS",
                if self.mode == SearchMode::Fts { active_style } else { normal_style },
            ),
            Span::styled(" | ", normal_style),
            Span::styled(
                "Sem",
                if self.mode == SearchMode::Semantic { active_style } else { normal_style },
            ),
            Span::styled(" ]", normal_style),
        ];
        let control_width = 13; // "[ FTS | Sem ]".len()

        // 2. Build the search query part
        let query =
            if self.query.is_empty() { self.placeholder.as_str() } else { self.query.as_str() };

        // padding(1) + query + padding(1) + control
        let available_width = width.saturating_sub(control_width + 2);
        // Use character-aware truncation for UTF-8 safety
        let truncated = if query.chars().count() > available_width {
            let char_count = query.chars().count();
            let skip_chars = char_count.saturating_sub(available_width);
            query.chars().skip(skip_chars).collect::<String>()
        } else {
            query.to_string()
        };

        let query_span = if self.query.is_empty() {
            Span::styled(&truncated, Style::default().fg(Color::DarkGray))
        } else {
            Span::raw(truncated)
        };

        // Render query left-aligned
        let query_line = Line::from(vec![Span::raw(" "), query_span]);
        Paragraph::new(query_line).render(area, buf);

        // Render control right-aligned
        let control_x = area.right().saturating_sub(control_width as u16 + 1);
        let control_line = Line::from(control_spans);
        let control_area = Rect { x: control_x, y: area.y, width: control_width as u16, height: 1 };
        Paragraph::new(control_line).render(control_area, buf);

        // 3. Draw cursor if focused
        if self.focused {
            // Calculate visible character count before cursor
            let cursor_char_pos = self.query.chars().count();
            let query_char_count = self.query.chars().count();
            let visible_start = query_char_count.saturating_sub(available_width);
            let visible_cursor_pos = cursor_char_pos.saturating_sub(visible_start);

            let cursor_x = area.x + 1 + visible_cursor_pos as u16;
            let cursor_y = area.y;
            let x = cursor_x.min(control_x.saturating_sub(1));
            if let Some(cell) = buf.cell_mut((x, cursor_y)) {
                cell.set_style(Style::default().bg(Color::White).fg(Color::Black));
            }
        }
    }
}

impl Default for SearchBar {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for SearchBar {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.last_area.set(area);
        // Render border
        let border_style = if self.focused {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = ratatui::widgets::Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .borders(ratatui::widgets::Borders::ALL);

        let inner = block.inner(area);
        block.render(area, buf);

        self.render_content(inner, buf);
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        if !self.focused {
            return false;
        }

        match event {
            Event::Key(key) => match key.code {
                ratatui::crossterm::event::KeyCode::Char('s')
                | ratatui::crossterm::event::KeyCode::Char('S')
                    if key.modifiers.contains(ratatui::crossterm::event::KeyModifiers::CONTROL) =>
                {
                    self.cycle_mode();
                    true
                }
                ratatui::crossterm::event::KeyCode::Char(c) => {
                    // Character-aware insertion for UTF-8 safety
                    let char_count = self.query.chars().count();
                    if self.cursor <= char_count {
                        // Find the byte position corresponding to the character position
                        let byte_pos =
                            self.query.chars().take(self.cursor).map(|c| c.len_utf8()).sum();
                        self.query.insert(byte_pos, c);
                        self.cursor += 1;
                    }
                    true
                }
                ratatui::crossterm::event::KeyCode::Backspace => {
                    // Character-aware removal for UTF-8 safety
                    if self.cursor > 0 {
                        let char_indices: Vec<(usize, usize)> =
                            self.query.char_indices().map(|(i, c)| (i, c.len_utf8())).collect();

                        if self.cursor <= char_indices.len() {
                            // Find the byte position of the character before cursor
                            let remove_idx = self.cursor.saturating_sub(1);
                            if let Some(&(byte_start, char_len)) = char_indices.get(remove_idx) {
                                self.query.replace_range(byte_start..byte_start + char_len, "");
                                self.cursor -= 1;
                            }
                        }
                    }
                    true
                }
                ratatui::crossterm::event::KeyCode::Delete => {
                    // Character-aware removal for UTF-8 safety
                    let char_count = self.query.chars().count();
                    if self.cursor < char_count {
                        let char_indices: Vec<(usize, usize)> =
                            self.query.char_indices().map(|(i, c)| (i, c.len_utf8())).collect();

                        if let Some(&(byte_start, char_len)) = char_indices.get(self.cursor) {
                            self.query.replace_range(byte_start..byte_start + char_len, "");
                        }
                    }
                    true
                }
                ratatui::crossterm::event::KeyCode::Left => {
                    self.cursor = self.cursor.saturating_sub(1);
                    true
                }
                ratatui::crossterm::event::KeyCode::Right => {
                    let char_count = self.query.chars().count();
                    self.cursor = self.cursor.saturating_add(1).min(char_count);
                    true
                }
                ratatui::crossterm::event::KeyCode::Home => {
                    self.cursor = 0;
                    true
                }
                ratatui::crossterm::event::KeyCode::End => {
                    self.cursor = self.query.chars().count();
                    true
                }
                ratatui::crossterm::event::KeyCode::Esc => {
                    self.clear();
                    true
                }
                _ => false,
            },
            Event::Mouse(mouse) => {
                if let ratatui::crossterm::event::MouseEventKind::Down(button) = mouse.kind
                    && button == ratatui::crossterm::event::MouseButton::Left
                {
                    let area = self.last_area.get();
                    let control_width = 13;
                    let control_start_x = area.right().saturating_sub(control_width as u16 + 1);

                    if mouse.column >= control_start_x
                        && mouse.column < area.right()
                        && mouse.row == area.y + 1
                    {
                        self.cycle_mode();
                        return true;
                    }
                }
                false
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_bar_creation() {
        let bar = SearchBar::new();
        assert_eq!(bar.query(), "");
        assert!(!bar.focused);
        assert_eq!(bar.cursor, 0);
    }

    #[test]
    fn test_search_bar_query() {
        let mut bar = SearchBar::new();
        bar.set_query("test query");
        assert_eq!(bar.query(), "test query");
        assert_eq!(bar.cursor, 10);
    }

    #[test]
    fn test_search_bar_clear() {
        let mut bar = SearchBar::new();
        bar.set_query("test");
        bar.clear();
        assert_eq!(bar.query(), "");
        assert_eq!(bar.cursor, 0);
    }

    #[test]
    fn test_search_mode_cycle() {
        let mut bar = SearchBar::new();
        assert_eq!(bar.mode(), SearchMode::Fts);

        bar.cycle_mode();
        assert_eq!(bar.mode(), SearchMode::Semantic);

        bar.cycle_mode();
        assert_eq!(bar.mode(), SearchMode::Fts);
    }
}
