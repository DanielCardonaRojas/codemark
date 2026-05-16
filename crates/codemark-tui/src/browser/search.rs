//! Search bar component for filtering content.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
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
}

/// Search mode for filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// Search all content
    All,
    /// Search repos only
    Repos,
    /// Search tours only
    Tours,
}

impl SearchBar {
    /// Create a new search bar.
    pub fn new() -> Self {
        Self {
            query: String::new(),
            placeholder: "Search...".to_string(),
            focused: false,
            cursor: 0,
            mode: SearchMode::All,
        }
    }

    /// Get the current search query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Set the search query.
    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
        self.cursor = self.query.len();
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
            SearchMode::All => SearchMode::Repos,
            SearchMode::Repos => SearchMode::Tours,
            SearchMode::Tours => SearchMode::All,
        };
    }

    /// Render the search bar content.
    fn render_content(&self, area: Rect, buf: &mut Buffer) {
        let width = area.width as usize;

        // Build the display text
        let mode_icon = match self.mode {
            SearchMode::All => "1 🔍",
            SearchMode::Repos => "1 📦",
            SearchMode::Tours => "1 🗺️",
        };

        let query = if self.query.is_empty() {
            self.placeholder.as_str()
        } else {
            self.query.as_str()
        };

        let available_width = width.saturating_sub(2 + mode_icon.len()); // icon + space
        let truncated = if query.len() > available_width {
            let start = query.len().saturating_sub(available_width);
            &query[start..]
        } else {
            query
        };

        let line = Line::from(vec![
            Span::styled(
                mode_icon,
                if self.focused {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::DarkGray)
                }
            ),
            Span::raw(" "),
            if self.query.is_empty() {
                Span::styled(truncated, Style::default().fg(Color::DarkGray))
            } else {
                Span::raw(truncated)
            },
        ]);

        Paragraph::new(line).render(area, buf);

        // Draw cursor if focused
        if self.focused && !self.query.is_empty() {
            let cursor_x = area.x + 2 + self.cursor.saturating_sub(
                self.query.len().saturating_sub(available_width)
            ) as u16;
            let cursor_y = area.y;
            let x = cursor_x.min(area.right() - 1);
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
                ratatui::crossterm::event::KeyCode::Char(c) => {
                    self.query.insert(self.cursor, c);
                    self.cursor += 1;
                    true
                }
                ratatui::crossterm::event::KeyCode::Backspace => {
                    if self.cursor > 0 {
                        self.query.remove(self.cursor - 1);
                        self.cursor -= 1;
                    }
                    true
                }
                ratatui::crossterm::event::KeyCode::Delete => {
                    if self.cursor < self.query.len() {
                        self.query.remove(self.cursor);
                    }
                    true
                }
                ratatui::crossterm::event::KeyCode::Left => {
                    self.cursor = self.cursor.saturating_sub(1);
                    true
                }
                ratatui::crossterm::event::KeyCode::Right => {
                    self.cursor = self.cursor.saturating_add(1).min(self.query.len());
                    true
                }
                ratatui::crossterm::event::KeyCode::Home => {
                    self.cursor = 0;
                    true
                }
                ratatui::crossterm::event::KeyCode::End => {
                    self.cursor = self.query.len();
                    true
                }
                ratatui::crossterm::event::KeyCode::Tab => {
                    self.cycle_mode();
                    true
                }
                _ => false,
            },
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
        assert_eq!(bar.cursor, 11);
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
        assert_eq!(bar.mode(), SearchMode::All);

        bar.cycle_mode();
        assert_eq!(bar.mode(), SearchMode::Repos);

        bar.cycle_mode();
        assert_eq!(bar.mode(), SearchMode::Tours);

        bar.cycle_mode();
        assert_eq!(bar.mode(), SearchMode::All);
    }
}
