//! Code preview component with syntax highlighting and line numbers.

use std::cell::RefCell;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{
        Scrollbar, ScrollbarOrientation,
        ScrollbarState, Widget, StatefulWidget,
    },
};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SyntectStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use std::sync::LazyLock;

use super::Component;
use crate::event::Event;

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

// @lat: [[tui-line-range-selection#CodePreview component]]
/// A component for displaying syntax-highlighted code with line numbers.
#[derive(Debug, Clone)]
pub struct CodePreview {
    /// The code to display
    code: String,
    /// File extension for syntax detection
    extension: String,
    /// Vertical scroll offset
    scroll_offset: u16,
    /// Currently highlighted line (0-indexed)
    selected_line: Option<usize>,
    /// Currently highlighted range (start, end inclusive, 0-indexed)
    selected_range: Option<(usize, usize)>,
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
            scroll_offset: 0,
            selected_line: None,
            selected_range: None,
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
        self.scroll_offset = 0;
        self.selected_line = None;
        self.selected_range = None;
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
            spans.push(Span::styled(
                line_num,
                Style::default().fg(Color::DarkGray),
            ));

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

    // @lat: [[tui-line-range-selection#CodePreview Component#Jump to Range]]
    /// Jump to and select a specific line index (0-indexed).
    pub fn jump_to_line(&mut self, line_index: usize) {
        self.jump_to_range(line_index, None);
    }

    /// Jump to and select a line range (0-indexed).
    /// If end is None, only the start line is highlighted.
    pub fn jump_to_range(&mut self, start: usize, end: Option<usize>) {
        let line_count = self.code.lines().count();
        if line_count > 0 {
            let start = start.min(line_count - 1);
            let end_value = end.map(|e| e.min(line_count - 1)).unwrap_or(start);

            self.selected_line = Some(start);
            // Always set selected_range if we have a multi-line range
            self.selected_range = if let Some(e) = end {
                if start != e {
                    Some((start.min(e), e))
                } else {
                    None
                }
            } else {
                None
            };

            // Adjust scroll offset immediately if we have area info
            let area_height = self.last_area.get().height as usize;

            if area_height > 0 {
                self.scroll_offset = self.calculate_scroll_offset(start, end_value, area_height);
            } else {
                // No area info yet - set a reasonable default
                self.scroll_offset = start.saturating_sub(2) as u16;
            }
        }
    }

    /// Calculate the scroll offset for a given range.
    fn calculate_scroll_offset(&self, start: usize, end: usize, area_height: usize) -> u16 {
        let range_height = end.saturating_sub(start) + 1;

        if range_height >= area_height {
            // Range is larger than viewport, show from start with minimal padding
            start.saturating_sub(1) as u16
        } else {
            // Center the range in viewport
            let range_mid = start + range_height / 2;
            let half_height = area_height / 2;
            range_mid.saturating_sub(half_height) as u16
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
        let selected = self.selected_line;
        let selected_range = self.selected_range;
        let list_len = cached.len();
        let height = area.height as usize;

        // Apply highlighting to the selected line or range
        let mut text_lines = Vec::with_capacity(list_len);
        for (i, line) in cached.iter().enumerate() {
            let is_selected = selected == Some(i);
            let in_range = selected_range.map_or(false, |(start, end)| i >= start && i <= end);

            if is_selected || in_range {
                let bg_color = if in_range && !is_selected {
                    // Lighter background for range (excluding the selected line itself)
                    Color::Rgb(70, 70, 90)
                } else {
                    // Standard highlight for selected line
                    Color::Rgb(90, 90, 110)
                };
                // Clone the line and apply style
                let mut styled_line = line.clone();
                styled_line = styled_line.style(Style::default().bg(bg_color));
                text_lines.push(styled_line);
            } else {
                text_lines.push(line.clone());
            }
        }

        drop(cached); // Drop cached before rendering

        let paragraph = ratatui::widgets::Paragraph::new(text_lines)
            .scroll((self.scroll_offset, 0));

        paragraph.render(area, buf);

        // Render scrollbar
        if !self.code.is_empty() && list_len > height {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(None)
                .thumb_symbol("┃");

            let mut scrollbar_state = ScrollbarState::new(list_len)
                .position(self.scroll_offset as usize);

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

        match event {
            Event::Key(key) => match key.code {
                ratatui::crossterm::event::KeyCode::Down
                | ratatui::crossterm::event::KeyCode::Char('j') => {
                    let line_count = self.code.lines().count();
                    if line_count > 0 {
                        let next = self.selected_line.map_or(0, |i| (i + 1).min(line_count - 1));
                        self.selected_line = Some(next);
                        // Clear range when manually navigating
                        self.selected_range = None;

                        // Auto-scroll if selection goes off screen
                        let height = self.last_area.get().height as usize;
                        if next >= self.scroll_offset as usize + height {
                            self.scroll_offset = (next - height + 1) as u16;
                        } else if next < self.scroll_offset as usize {
                            self.scroll_offset = next as u16;
                        }
                    }
                    true
                }
                ratatui::crossterm::event::KeyCode::Up
                | ratatui::crossterm::event::KeyCode::Char('k') => {
                    let next = self.selected_line.map_or(0, |i| i.saturating_sub(1));
                    self.selected_line = Some(next);
                    // Clear range when manually navigating
                    self.selected_range = None;

                    // Auto-scroll if selection goes off screen
                    if next < self.scroll_offset as usize {
                        self.scroll_offset = next as u16;
                    }
                    true
                }
                ratatui::crossterm::event::KeyCode::Char('J') => {
                    let line_count = self.code.lines().count();
                    self.scroll_offset = self
                        .scroll_offset
                        .saturating_add(5)
                        .min(line_count.saturating_sub(1) as u16);
                    true
                }
                ratatui::crossterm::event::KeyCode::Char('K') => {
                    self.scroll_offset = self.scroll_offset.saturating_sub(5);
                    true
                }
                _ => false,
            },
            Event::Mouse(mouse) => match mouse.kind {
                ratatui::crossterm::event::MouseEventKind::ScrollDown => {
                    let line_count = self.code.lines().count();
                    self.scroll_offset = self.scroll_offset.saturating_add(1)
                        .min(line_count.saturating_sub(1) as u16);
                    true
                }
                ratatui::crossterm::event::MouseEventKind::ScrollUp => {
                    self.scroll_offset = self.scroll_offset.saturating_sub(1);
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
