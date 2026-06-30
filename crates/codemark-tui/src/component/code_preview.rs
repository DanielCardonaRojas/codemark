//! Code preview component with syntax highlighting and line numbers.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget},
};
use std::cell::RefCell;
use std::sync::{Arc, LazyLock, RwLock};
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

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(load_syntax_set);

/// Width of the gutter (sign column + line number) that precedes code content.
/// 1-char sign column + `"{:>3} "` line number (4 chars) = 5 columns. The block
/// cursor is offset by this when mapping a code column to a buffer x position.
const GUTTER_WIDTH: u16 = 5;

/// Process-wide default theme applied to previews created after startup. Set
/// once from config via [`set_default_theme`]; otherwise resolves the registry
/// fallback on first use. Shared via `Arc` so each preview clone is cheap.
static DEFAULT_THEME: RwLock<Option<Arc<Theme>>> = RwLock::new(None);

/// Set the default theme for every [`CodePreview`] created afterwards.
pub fn set_default_theme(theme: Theme) {
    if let Ok(mut guard) = DEFAULT_THEME.write() {
        *guard = Some(Arc::new(theme));
    }
}

/// The current process-wide default preview theme (shared via `Arc`).
///
/// Exposed so existing previews can be re-themed after a runtime theme change:
/// previews capture their theme at construction, so changing the default alone
/// does not update them — callers hand this to [`CodePreview::set_theme`].
pub fn current_default_theme() -> Arc<Theme> {
    default_theme()
}

/// The current default theme, resolving and caching the registry fallback on
/// first use if [`set_default_theme`] was never called.
fn default_theme() -> Arc<Theme> {
    if let Ok(guard) = DEFAULT_THEME.read()
        && let Some(t) = guard.as_ref()
    {
        return t.clone();
    }
    let theme = Arc::new(crate::theme::default_theme());
    if let Ok(mut guard) = DEFAULT_THEME.write()
        && guard.is_none()
    {
        *guard = Some(theme.clone());
    }
    theme
}

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
    /// Currently highlighted line (0-indexed). Doubles as the cursor's row when
    /// the preview is focused.
    selected_line: Option<usize>,
    /// Cursor column within the current line (0-indexed char offset into the
    /// code content, excluding the gutter). Only meaningful while
    /// `selected_line` is `Some`.
    cursor_col: usize,
    /// Currently highlighted range (start, end inclusive, 0-indexed)
    selected_range: Option<(usize, usize)>,
    /// Whether the component is focused
    focused: bool,
    /// Last rendered area
    last_area: std::cell::Cell<Rect>,
    /// Cache for highlighted lines to avoid re-highlighting on every frame
    cached_lines: RefCell<Vec<Line<'static>>>,
    /// Optional file name header displayed above the code
    file_header: Option<String>,
    /// Theme used for syntax highlighting. Shared via `Arc` so cloning a
    /// `CodePreview` does not duplicate the (large) theme data.
    theme: Arc<Theme>,
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
            cursor_col: 0,
            selected_range: None,
            focused: false,
            last_area: std::cell::Cell::new(Rect::default()),
            cached_lines: RefCell::new(Vec::new()),
            file_header: None,
            theme: default_theme(),
        };
        preview.refresh_cache();
        preview
    }

    /// Set the code to display.
    pub fn set_code(&mut self, code: String) {
        self.code = code;
        self.scroll_offset = 0;
        self.selected_line = None;
        self.cursor_col = 0;
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

    /// Set the file header displayed above the code preview.
    pub fn set_file_header(&mut self, header: Option<String>) {
        self.file_header = header;
    }

    /// Set the syntax highlighting theme and rebuild the highlight cache.
    ///
    /// The theme is shared via `Arc`; resolve it once at the app level (e.g. via
    /// [`crate::theme::ThemeRegistry`]) and hand the same `Arc` to every preview.
    pub fn set_theme(&mut self, theme: Arc<Theme>) {
        self.theme = theme;
        self.refresh_cache();
    }

    /// Refresh the syntax highlighting cache.
    fn refresh_cache(&self) {
        let syntax_set = &*SYNTAX_SET;
        let theme = &*self.theme;
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
            // Sign column shows: '┃' for lines in the bookmark range
            let sign = " "; // Default: no mark
            spans.push(Span::styled(sign, Style::default().fg(crate::theme::palette().dim)));

            // Add line number (gutter)
            let line_num = format!("{:>3} ", i + 1);
            spans.push(Span::styled(line_num, Style::default().fg(crate::theme::palette().dim)));

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
            self.cursor_col = 0;
            // Use the clamped end_value and normalize the ordering.
            // Always set selected_range when end is Some, even for single-line ranges.
            self.selected_range = if end.is_some() {
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

    /// Move the viewport by `delta` lines (positive scrolls down, negative up),
    /// clamped to the content bounds. Returns true if the offset changed.
    ///
    /// This is a plain viewport move that leaves the line selection untouched, so
    /// callers can scroll the preview regardless of focus — e.g. `J`/`K` while
    /// focus stays on another pane.
    pub fn scroll_by(&mut self, delta: i32) -> bool {
        let height = self.last_area.get().height as usize;
        let max_offset = self.line_count().saturating_sub(height) as i32;
        let old_offset = self.scroll_offset;
        self.scroll_offset = (self.scroll_offset as i32 + delta).clamp(0, max_offset) as u16;
        old_offset != self.scroll_offset
    }

    /// Number of characters in the code content of `line_idx` (excluding the
    /// gutter and trailing newline). Used to clamp the cursor column.
    fn line_char_len(&self, line_idx: usize) -> usize {
        self.code.lines().nth(line_idx).map_or(0, |l| l.chars().count())
    }

    /// Width available for code content within the last rendered area, i.e. the
    /// pane width minus the gutter and the scrollbar (when one is shown).
    fn content_width(&self) -> usize {
        let area = self.last_area.get();
        let height = area.height as usize;
        let scrollbar = if self.line_count() > height { 1 } else { 0 };
        (area.width as usize).saturating_sub(GUTTER_WIDTH as usize + scrollbar)
    }

    /// The furthest column the cursor may occupy on `line_idx`: clamped to both
    /// the line length and the visible content width so the cursor stays on
    /// screen (there is no horizontal scroll yet).
    fn max_cursor_col(&self, line_idx: usize) -> usize {
        self.line_char_len(line_idx).min(self.content_width().saturating_sub(1))
    }

    /// Clamp `cursor_col` to the current line after a vertical move.
    fn clamp_cursor_col(&mut self) {
        if let Some(line) = self.selected_line {
            self.cursor_col = self.cursor_col.min(self.max_cursor_col(line));
        }
    }
}

impl Component for CodePreview {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Render file header if set, and offset code area accordingly
        let code_area = if let Some(ref header) = self.file_header {
            if area.height < 2 {
                // Not enough room for header + code
                area
            } else {
                let header_area = Rect { height: 1, ..area };
                let code_area =
                    Rect { y: area.y + 1, height: area.height.saturating_sub(1), ..area };

                let header_line = Line::from(Span::styled(
                    format!(" {}", header),
                    Style::default()
                        .fg(crate::theme::palette().accent)
                        .add_modifier(Modifier::BOLD),
                ));
                ratatui::widgets::Paragraph::new(header_line).render(header_area, buf);

                code_area
            }
        } else {
            area
        };

        // Store the code area (not full area) so scroll calculations use the correct height
        self.last_area.set(code_area);

        let cached = self.cached_lines.borrow();
        let selected_range = self.selected_range;
        let list_len = cached.len();
        let height = code_area.height as usize;

        // Build text lines with sign column indicators
        let mut text_lines = Vec::with_capacity(list_len);
        for (i, line) in cached.iter().enumerate() {
            let in_range = selected_range.is_some_and(|(start, end)| i >= start && i <= end);

            // Sign indicator: show ┃ for range lines (independent of current selection)
            let sign = if in_range {
                "┃" // Thick vertical bar for lines in the bookmark range
            } else {
                " "
            };

            // Sign color: magenta for range lines, regardless of current selection
            let sign_color =
                if in_range { crate::theme::palette().marker } else { crate::theme::palette().dim };

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

        paragraph.render(code_area, buf);

        // Highlight the selected line's full-width background so it reads the
        // same as the list panes' selected row (see `crate::theme::SELECTION_BG`).
        // Painted after the paragraph and before the scrollbar so the scrollbar
        // thumb keeps its own styling on the highlighted row.
        if let Some(selected) = self.selected_line {
            let offset = self.scroll_offset as usize;
            if selected >= offset {
                let row = (selected - offset) as u16;
                if row < code_area.height {
                    let y = code_area.y + row;
                    for x in code_area.left()..code_area.right() {
                        if let Some(cell) = buf.cell_mut((x, y)) {
                            cell.set_bg(crate::theme::SELECTION_BG);
                        }
                    }
                }
            }
        }

        // Draw the block cursor on the focused row/column. Painted on top of the
        // selection background by reversing the cell under the cursor, so it
        // keeps the underlying character and stays theme-independent.
        if self.focused
            && let Some(selected) = self.selected_line
        {
            let offset = self.scroll_offset as usize;
            if selected >= offset {
                let row = (selected - offset) as u16;
                let x = code_area.x + GUTTER_WIDTH + self.cursor_col as u16;
                if row < code_area.height && x < code_area.right() {
                    let y = code_area.y + row;
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
                    }
                }
            }
        }

        // Render scrollbar
        if !self.code.is_empty() && list_len > height {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(None)
                .thumb_symbol("┃");

            // Ratatui 0.29's thumb math sizes the thumb as
            // `viewport / (content_length - 1 + viewport)` and caps the thumb position at
            // `content_length - 1`. Passing the raw line count there makes the thumb too
            // short and unable to reach the bottom (a gap of one thumb-height is left).
            // Instead, model `content_length` as the number of scroll positions
            // (`list_len - height + 1`) and leave `viewport_content_length` at its default
            // so Ratatui uses the track height as the viewport. This yields a thumb of size
            // `height^2 / list_len` (the correct visible/total ratio) and a max position of
            // `list_len - height`, so the thumb reaches the bottom on the last page.
            let scroll_positions = list_len - height + 1;
            let mut scrollbar_state =
                ScrollbarState::new(scroll_positions).position(self.scroll_offset as usize);

            let scrollbar_area = Rect {
                x: code_area.right().saturating_sub(1),
                y: code_area.top(),
                width: 1,
                height: code_area.height,
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
                            self.clamp_cursor_col();
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
                        self.clamp_cursor_col();
                        true
                    }
                    ratatui::crossterm::event::KeyCode::Left
                    | ratatui::crossterm::event::KeyCode::Char('h') => {
                        if self.selected_line.is_some() {
                            self.cursor_col = self.cursor_col.saturating_sub(1);
                            true
                        } else {
                            false
                        }
                    }
                    ratatui::crossterm::event::KeyCode::Right
                    | ratatui::crossterm::event::KeyCode::Char('l') => {
                        if let Some(line) = self.selected_line {
                            self.cursor_col = (self.cursor_col + 1).min(self.max_cursor_col(line));
                            true
                        } else {
                            false
                        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeRegistry;

    /// Collect the foreground color of every cached span, so two highlight
    /// passes can be compared.
    fn span_colors(preview: &CodePreview) -> Vec<Option<Color>> {
        preview
            .cached_lines
            .borrow()
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.style.fg))
            .collect()
    }

    /// Render the preview into a fresh buffer of the given size.
    fn render_to_buffer(preview: &CodePreview, width: u16, height: u16) -> Buffer {
        let area = Rect { x: 0, y: 0, width, height };
        let mut buf = Buffer::empty(area);
        preview.render(area, &mut buf);
        buf
    }

    /// The background color of the first cell on a given row.
    fn row_bg(buf: &Buffer, y: u16) -> Color {
        buf.cell((0, y)).expect("cell within bounds").style().bg.unwrap_or(Color::Reset)
    }

    /// Whether the cell at (x, y) carries the reversed (cursor) modifier.
    fn cell_reversed(buf: &Buffer, x: u16, y: u16) -> bool {
        buf.cell((x, y))
            .expect("cell within bounds")
            .style()
            .add_modifier
            .contains(Modifier::REVERSED)
    }

    /// Send a single character key to the preview.
    fn press(preview: &mut CodePreview, c: char) -> bool {
        preview.handle_event(&Event::Key(ratatui::crossterm::event::KeyEvent::from(
            ratatui::crossterm::event::KeyCode::Char(c),
        )))
    }

    #[test]
    fn hl_moves_cursor_column_and_renders_block_cursor() {
        let code = "one\ntwo\nthree\nfour\n";
        let mut preview = CodePreview::new(code, "txt");
        preview.set_focus(true);
        // Establish the rendered area so column clamping has a width to work with.
        render_to_buffer(&preview, 30, 4);

        // No cursor row until the user starts navigating: h/l are no-ops.
        assert!(!press(&mut preview, 'h'));
        assert!(!press(&mut preview, 'l'));
        assert_eq!(preview.cursor_col, 0);

        // Select the first line, then walk the cursor right with `l`.
        assert!(press(&mut preview, 'j'));
        assert_eq!(preview.selected_line, Some(0));
        assert!(press(&mut preview, 'l'));
        assert!(press(&mut preview, 'l'));
        assert_eq!(preview.cursor_col, 2);

        // The block cursor is rendered (reversed) at gutter + cursor_col.
        let buf = render_to_buffer(&preview, 30, 4);
        assert!(cell_reversed(&buf, GUTTER_WIDTH + 2, 0));

        // `h` walks it back, saturating at column 0.
        assert!(press(&mut preview, 'h'));
        assert!(press(&mut preview, 'h'));
        assert!(press(&mut preview, 'h'));
        assert_eq!(preview.cursor_col, 0);
    }

    #[test]
    fn cursor_col_clamps_to_shorter_line_on_vertical_move() {
        // Line 2 ("three") is longer than line 1 ("two").
        let code = "one\ntwo\nthree\nfour\n";
        let mut preview = CodePreview::new(code, "txt");
        preview.set_focus(true);
        render_to_buffer(&preview, 30, 4);

        // Move to "three" and push the cursor near its end.
        press(&mut preview, 'j');
        press(&mut preview, 'j');
        press(&mut preview, 'j');
        assert_eq!(preview.selected_line, Some(2));
        for _ in 0..5 {
            press(&mut preview, 'l');
        }
        assert_eq!(preview.cursor_col, 5); // end of "three"

        // Moving up to "two" clamps the column to that line's length.
        press(&mut preview, 'k');
        assert_eq!(preview.selected_line, Some(1));
        assert_eq!(preview.cursor_col, 3); // end of "two"
    }

    #[test]
    fn cursor_only_renders_when_focused() {
        let code = "one\ntwo\n";
        let mut preview = CodePreview::new(code, "txt");
        preview.set_focus(true);
        render_to_buffer(&preview, 30, 2);
        press(&mut preview, 'j');
        press(&mut preview, 'l');

        let buf = render_to_buffer(&preview, 30, 2);
        assert!(cell_reversed(&buf, GUTTER_WIDTH + 1, 0));

        // Blur the pane: the cursor disappears.
        preview.set_focus(false);
        let buf = render_to_buffer(&preview, 30, 2);
        assert!(!cell_reversed(&buf, GUTTER_WIDTH + 1, 0));
    }

    #[test]
    fn jk_moves_selected_line_and_highlights_it() {
        let code = "one\ntwo\nthree\nfour\n";
        let mut preview = CodePreview::new(code, "txt");
        preview.set_focus(true);

        // No selection yet: nothing is highlighted.
        let buf = render_to_buffer(&preview, 20, 4);
        for y in 0..4 {
            assert_ne!(row_bg(&buf, y), crate::theme::SELECTION_BG);
        }

        // `j` selects the first line; it is highlighted with the shared color.
        let down = Event::Key(ratatui::crossterm::event::KeyEvent::from(
            ratatui::crossterm::event::KeyCode::Char('j'),
        ));
        assert!(preview.handle_event(&down));
        assert_eq!(preview.selected_line, Some(0));
        let buf = render_to_buffer(&preview, 20, 4);
        assert_eq!(row_bg(&buf, 0), crate::theme::SELECTION_BG);
        assert_ne!(row_bg(&buf, 1), crate::theme::SELECTION_BG);

        // `j` again moves the highlight down a row.
        assert!(preview.handle_event(&down));
        assert_eq!(preview.selected_line, Some(1));
        let buf = render_to_buffer(&preview, 20, 4);
        assert_ne!(row_bg(&buf, 0), crate::theme::SELECTION_BG);
        assert_eq!(row_bg(&buf, 1), crate::theme::SELECTION_BG);

        // `k` moves it back up.
        let up = Event::Key(ratatui::crossterm::event::KeyEvent::from(
            ratatui::crossterm::event::KeyCode::Char('k'),
        ));
        assert!(preview.handle_event(&up));
        assert_eq!(preview.selected_line, Some(0));
        let buf = render_to_buffer(&preview, 20, 4);
        assert_eq!(row_bg(&buf, 0), crate::theme::SELECTION_BG);
    }

    #[test]
    fn set_theme_rehighlights_with_new_colors() {
        let code = "fn main() {\n    let x = 42;\n}\n";
        let mut preview = CodePreview::new(code, "rs");
        let before = span_colors(&preview);

        // Dracula differs from the default OneHalfDark palette, so the
        // highlighted span colors must change after applying it.
        let dracula = ThemeRegistry::new().resolve("Dracula");
        preview.set_theme(Arc::new(dracula));
        let after = span_colors(&preview);

        assert_ne!(before, after, "switching theme should change highlight colors");
    }
}
