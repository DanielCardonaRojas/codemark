//! Code preview component with syntax highlighting and line numbers.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{
        List, ListItem, ListState, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget,
    },
};
use std::cell::RefCell;
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SyntectStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use super::Component;
use crate::event::Event;

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(|| SyntaxSet::load_defaults_newlines());
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(|| ThemeSet::load_defaults());

/// A component for displaying syntax-highlighted code with line numbers.
#[derive(Debug, Clone)]
pub struct CodePreview {
    /// The code to display
    code: String,
    /// File extension for syntax detection
    extension: String,
    /// List state for scrolling
    list_state: RefCell<ListState>,
    /// Whether the component is focused
    focused: bool,
    /// Last rendered area
    last_area: std::cell::Cell<Rect>,
    /// Cache for highlighted lines to avoid re-highlighting on every frame
    cached_lines: RefCell<Vec<Line<'static>>>,
}

impl CodePreview {
    /// Create a new code preview.
    pub fn new(code: impl Into<String>, extension: impl Into<String>) -> Self {
        let code = code.into();
        let extension = extension.into();
        let preview = Self {
            code,
            extension,
            list_state: RefCell::new(ListState::default()),
            focused: false,
            last_area: std::cell::Cell::new(Rect::default()),
            cached_lines: RefCell::new(Vec::new()),
        };
        preview.refresh_cache();
        preview
    }

    /// Set the code to display.
    pub fn set_code(&mut self, code: String) {
        self.code = code;
        *self.list_state.borrow_mut() = ListState::default();
        self.refresh_cache();
    }

    /// Set the file extension.
    pub fn set_extension(&mut self, extension: String) {
        if self.extension != extension {
            self.extension = extension;
            self.refresh_cache();
        }
    }

    /// Refresh the syntax highlighting cache.
    fn refresh_cache(&self) {
        let ps = &*SYNTAX_SET;
        let ts = &*THEME_SET;
        let syntax = ps
            .find_syntax_by_extension(&self.extension)
            .unwrap_or_else(|| ps.find_syntax_plain_text());
        let theme = &ts.themes["base16-ocean.dark"];

        let mut h = HighlightLines::new(syntax, theme);
        let mut highlighted = Vec::new();

        for (i, line) in LinesWithEndings::from(&self.code).enumerate() {
            let ranges: Vec<(SyntectStyle, &str)> = h.highlight_line(line, ps).unwrap_or_default();
            let mut spans = Vec::new();

            // Add line number (gutter)
            let line_num = format!("{:>3} ", i + 1);
            spans.push(Span::styled(line_num, Style::default().fg(Color::DarkGray)));

            // Convert syntect style to ratatui style
            for (style, text) in ranges {
                let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
                spans.push(Span::styled(
                    text.trim_end_matches(['\n', '\r']).to_string(),
                    Style::default().fg(fg),
                ));
            }
            highlighted.push(Line::from(spans));
        }
        *self.cached_lines.borrow_mut() = highlighted;
    }

    /// Jump to and select a specific line index (0-indexed).
    pub fn jump_to_line(&mut self, line_index: usize) {
        let line_count = self.code.lines().count();
        if line_count > 0 {
            let target = line_index.min(line_count - 1);
            self.list_state.borrow_mut().select(Some(target));
        }
    }

    /// Get the last rendered area.
    pub fn last_area(&self) -> Rect {
        self.last_area.get()
    }
}

impl Component for CodePreview {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.last_area.set(area);

        let cached = self.cached_lines.borrow();
        let selected = self.list_state.borrow().selected();

        let list_items: Vec<ListItem> = cached
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let mut list_item = ListItem::new(line.clone());
                if selected == Some(i) {
                    let bg_color = if self.focused {
                        Color::Rgb(50, 50, 50) // Light gray highlight for focused
                    } else {
                        Color::Rgb(35, 35, 35) // Darker gray for unfocused
                    };
                    list_item = list_item.style(Style::default().bg(bg_color));
                }
                list_item
            })
            .collect();

        let inner = area; // Assuming no border for now as TabbedPanel handles it
        let height = inner.height as usize;
        let list_len = list_items.len();

        let list = List::new(list_items);
        let mut state = self.list_state.borrow_mut();

        StatefulWidget::render(list, inner, buf, &mut *state);

        // Render scrollbar
        if !self.code.is_empty() && list_len > height {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(None)
                .thumb_symbol("┃");

            let mut scrollbar_state = ScrollbarState::new(list_len).position(state.offset());

            let scrollbar_area = Rect {
                x: area.right().saturating_sub(1),
                y: area.top(),
                width: 1,
                height: area.height,
            };

            scrollbar.render(scrollbar_area, buf, &mut scrollbar_state);
        }
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        if !self.focused {
            return false;
        }

        if let Event::Key(key) = event {
            match key.code {
                ratatui::crossterm::event::KeyCode::Down
                | ratatui::crossterm::event::KeyCode::Char('j') => {
                    let mut state = self.list_state.borrow_mut();
                    let line_count = self.code.lines().count();
                    if line_count > 0 {
                        let next = state.selected().map_or(0, |i| (i + 1).min(line_count - 1));
                        state.select(Some(next));
                    }
                    true
                }
                ratatui::crossterm::event::KeyCode::Up
                | ratatui::crossterm::event::KeyCode::Char('k') => {
                    let mut state = self.list_state.borrow_mut();
                    let next = state.selected().map_or(0, |i| i.saturating_sub(1));
                    state.select(Some(next));
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
