//! Search bar component for filtering content.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{BorderType, Paragraph, Widget},
};

use crate::component::Component;
use crate::event::Event;

/// Braille frames for the in-progress loading spinner shown while a search
/// (notably the slower semantic search) is running.
const SPINNER_FRAMES: &[&str] = &[
    "\u{28cb}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280f}",
];

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
    /// Error message to display
    error_message: Option<String>,
    /// Whether a search is currently in progress (drives the loading spinner)
    loading: bool,
    /// Animation frame index for the loading spinner
    spinner_frame: usize,
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
            error_message: None,
            loading: false,
            spinner_frame: 0,
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
        self.set_loading(false);
    }

    /// Whether a search is currently in progress.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Mark a search as in progress (or finished), showing/hiding the spinner.
    pub fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
        if !loading {
            self.spinner_frame = 0;
        }
    }

    /// Advance the loading spinner animation by one frame.
    pub fn advance_spinner(&mut self) {
        self.spinner_frame = self.spinner_frame.wrapping_add(1);
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

    /// Set an error message to display.
    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.error_message = Some(msg.into());
        // An error ends the in-flight search, so stop the loading spinner.
        self.set_loading(false);
    }

    /// Clear any error message.
    pub fn clear_error(&mut self) {
        self.error_message = None;
    }

    /// Get the current error message.
    pub fn error_message(&self) -> Option<&str> {
        self.error_message.as_deref()
    }

    /// Render the search bar content.
    fn render_content(&self, area: Rect, buf: &mut Buffer) {
        let width = area.width as usize;

        // 1. Build the segmented control for the right side
        let normal_style = Style::default().fg(crate::theme::palette().dim);
        let active_style = if self.focused {
            Style::default().fg(crate::theme::palette().accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(crate::theme::palette().emphasis).add_modifier(Modifier::BOLD)
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

        // While a search is running, a spinner glyph (plus a trailing space)
        // sits just left of the control, so reserve those columns.
        let spinner_width = if self.loading { 2 } else { 0 };

        // 2. Build the search query part
        let query =
            if self.query.is_empty() { self.placeholder.as_str() } else { self.query.as_str() };

        // padding(1) + query + padding(1) + spinner + control
        let available_width = width.saturating_sub(control_width + 2 + spinner_width);
        // Use character-aware truncation for UTF-8 safety
        let truncated = if query.chars().count() > available_width {
            let char_count = query.chars().count();
            let skip_chars = char_count.saturating_sub(available_width);
            query.chars().skip(skip_chars).collect::<String>()
        } else {
            query.to_string()
        };

        let query_span = if self.query.is_empty() {
            Span::styled(&truncated, Style::default().fg(crate::theme::palette().dim))
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

        // Render the loading spinner just left of the control while searching.
        if self.loading {
            let frame = SPINNER_FRAMES[self.spinner_frame % SPINNER_FRAMES.len()];
            let spinner_x = control_x.saturating_sub(spinner_width as u16);
            let spinner_area =
                Rect { x: spinner_x, y: area.y, width: spinner_width as u16, height: 1 };
            let spinner_style = Style::default().fg(crate::theme::palette().warning);
            Paragraph::new(Line::from(Span::styled(format!("{frame} "), spinner_style)))
                .render(spinner_area, buf);
        }

        // 3. Draw cursor if focused
        if self.focused {
            // Calculate visible character count before cursor
            let cursor_char_pos = self.cursor;
            let query_char_count = self.query.chars().count();
            let visible_start = query_char_count.saturating_sub(available_width);
            let visible_cursor_pos = cursor_char_pos.saturating_sub(visible_start);

            let cursor_x = area.x + 1 + visible_cursor_pos as u16;
            let cursor_y = area.y;
            let x = cursor_x.min(control_x.saturating_sub(spinner_width as u16 + 1));
            if let Some(cell) = buf.cell_mut((x, cursor_y)) {
                cell.set_style(
                    Style::default()
                        .bg(crate::theme::palette().emphasis)
                        .fg(crate::theme::palette().inverse),
                );
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
        let border_style = if self.error_message.is_some() {
            Style::default().fg(crate::theme::palette().error)
        } else if self.focused {
            Style::default().fg(crate::theme::palette().accent)
        } else {
            Style::default().fg(crate::theme::palette().dim)
        };

        let block = ratatui::widgets::Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .borders(ratatui::widgets::Borders::ALL);

        let inner = block.inner(area);
        block.render(area, buf);

        self.render_content(inner, buf);

        // Render error message below the search bar if present
        if let Some(ref error) = self.error_message {
            let error_area = Rect {
                x: area.x,
                y: area.bottom().min(area.y.saturating_add(2)),
                width: area.width.min(60),
                height: 1,
            };
            let error_style =
                Style::default().fg(crate::theme::palette().error).add_modifier(Modifier::BOLD);
            Paragraph::new(Line::from(vec![
                Span::styled("! ", error_style),
                Span::styled(error, Style::default().fg(crate::theme::palette().error)),
            ]))
            .render(error_area, buf);
        }
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
                    // Clear error on input
                    self.clear_error();
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
                    // Clear error on input
                    self.clear_error();
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
                    // Clear error on input
                    self.clear_error();
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
    fn test_loading_state_toggles_and_resets_spinner() {
        let mut bar = SearchBar::new();
        assert!(!bar.is_loading());

        bar.set_loading(true);
        bar.advance_spinner();
        bar.advance_spinner();
        assert!(bar.is_loading());
        assert_eq!(bar.spinner_frame, 2);

        // Clearing loading also resets the animation frame.
        bar.set_loading(false);
        assert!(!bar.is_loading());
        assert_eq!(bar.spinner_frame, 0);
    }

    #[test]
    fn test_clear_and_error_stop_loading() {
        let mut bar = SearchBar::new();

        bar.set_loading(true);
        bar.clear();
        assert!(!bar.is_loading());

        bar.set_loading(true);
        bar.set_error("boom");
        assert!(!bar.is_loading());
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
