//! Code preview component with syntax highlighting and line numbers.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget},
};
use std::cell::RefCell;
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SyntectStyle, Theme};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use super::Component;
use crate::event::Event;

/// Load the syntax set from embedded syntect-assets data.
///
/// This is called once at startup via `LazyLock`. The data is bundled
/// at compile time from the syntect-assets package (which sources from
/// the bat project's syntax definitions).
fn load_syntax_set() -> SyntaxSet {
    syntect_assets::assets::HighlightingAssets::from_binary()
        .get_syntax_set()
        .expect("syntect-assets binary data should always contain a valid syntax set")
        .clone()
}

/// Load the theme from embedded syntect-assets data.
///
/// This is called once at startup via `LazyLock`. The theme is bundled
/// at compile time from the syntect-assets package.
///
/// Note: `get_theme` returns a reference to the theme directly, not a
/// `Result`. If the theme name is not found, syntect-assets will
/// silently fall back to a default theme. This is a known limitation
/// of the library's API.
fn load_theme() -> Theme {
    syntect_assets::assets::HighlightingAssets::from_binary().get_theme("OneHalfDark").clone()
}

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(load_syntax_set);
static THEME: LazyLock<Theme> = LazyLock::new(load_theme);

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
        let syntax_set = &*SYNTAX_SET;
        let theme = &*THEME;
        let syntax = syntax_set
            .find_syntax_by_extension(&self.extension)
            .unwrap_or_else(|| syntax_set.find_syntax_plain_text());

        let mut h = HighlightLines::new(syntax, theme);
        let mut highlighted = Vec::new();

        for (i, line) in LinesWithEndings::from(&self.code).enumerate() {
            let ranges: Vec<(SyntectStyle, &str)> =
                h.highlight_line(line, syntax_set).unwrap_or_default();
            let mut spans = Vec::new();

            // Add sign column indicator (1 char) + line number (4 chars) + space (1 char) = 6 chars total gutter
            // Sign column shows: '■' for single line, '├' for start, '│' for middle, '└' for end of range
            let sign = " "; // Default: no mark
            spans.push(Span::styled(sign, Style::default().fg(Color::DarkGray)));

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
            // Use the clamped end_value and normalize the ordering
            self.selected_range = if end.is_some() && start != end_value {
                Some((start.min(end_value), start.max(end_value)))
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

    /// Get the line count of the code.
    fn line_count(&self) -> usize {
        self.cached_lines.borrow().len()
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
        let selected_range = self.selected_range;
        let list_len = cached.len();
        let height = area.height as usize;

        // Build text lines with sign column indicators
        let mut text_lines = Vec::with_capacity(list_len);
        for (i, line) in cached.iter().enumerate() {
            let in_range = selected_range.is_some_and(|(start, end)| i >= start && i <= end);

            // Sign indicator: show │ for range lines (independent of current selection)
            let sign = if in_range {
                "│" // Vertical bar for lines in the bookmark range
            } else {
                " "
            };

            // Sign color: cyan for range lines, regardless of current selection
            let sign_color = if in_range { Color::Cyan } else { Color::DarkGray };

            // Rebuild the line with the correct sign indicator
            // The cached line has: sign (1 char) + line number (4 chars) + content
            // We need to replace just the sign span
            let mut new_spans = Vec::new();
            new_spans.push(Span::styled(sign, Style::default().fg(sign_color)));

            // Add the rest of the line (line number + content), skipping the original sign span
            if line.spans.len() > 1 {
                for span in &line.spans[1..] {
                    new_spans.push(span.clone());
                }
            }
            text_lines.push(Line::from(new_spans));
        }

        drop(cached); // Drop cached before rendering

        let paragraph =
            ratatui::widgets::Paragraph::new(text_lines).scroll((self.scroll_offset, 0));

        paragraph.render(area, buf);

        // Render scrollbar
        if !self.code.is_empty() && list_len > height {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(None)
                .thumb_symbol("┃");

            let mut scrollbar_state =
                ScrollbarState::new(list_len).position(self.scroll_offset as usize);

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
        match event {
            Event::Key(key) => {
                if !self.focused {
                    return false;
                }
                match key.code {
                    ratatui::crossterm::event::KeyCode::Down
                    | ratatui::crossterm::event::KeyCode::Char('j') => {
                        let line_count = self.line_count();
                        if line_count > 0 {
                            let next =
                                self.selected_line.map_or(0, |i| (i + 1).min(line_count - 1));
                            self.selected_line = Some(next);

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

                        // Auto-scroll if selection goes off screen
                        if next < self.scroll_offset as usize {
                            self.scroll_offset = next as u16;
                        }
                        true
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
