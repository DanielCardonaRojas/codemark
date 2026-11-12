//! Code preview component with syntax highlighting and line numbers.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget},
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::{Arc, LazyLock, RwLock};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SyntectStyle, Theme};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use super::Component;
use super::inline_comments::{
    BUBBLE_CHROME, BubbleStyle, DisplayRow, InlineThread, RowLayout, build_layout,
    render_bubble_line,
};
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

/// Width of the line-number field (right-aligned, minimum 3 digits) for a file
/// with `total_lines` lines. Shared by gutter rendering ([`CodePreview::refresh_cache`])
/// and cursor geometry ([`CodePreview::gutter_width`]) so the rendered gutter and
/// the cursor's x offset always agree.
fn line_number_width(total_lines: usize) -> usize {
    total_lines.max(1).to_string().len().max(3)
}

/// Upper bound on distinct highlighted files retained by a single preview's
/// [`highlight_cache`](CodePreview::highlight_cache). Comfortably exceeds a
/// typical collection's step count; on overflow the cache is cleared wholesale.
const HIGHLIGHT_CACHE_CAP: usize = 64;

// @lat: [[tui-line-range-selection#CodePreview component]]
/// A component for displaying syntax-highlighted code with line numbers.
#[derive(Debug, Clone)]
pub struct CodePreview {
    /// The code to display
    code: String,
    /// File extension for syntax detection
    extension: String,
    /// Vertical scroll offset
    scroll_offset: Cell<u16>,
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
    /// Whether the component is in selection mode, allowing column-level cursor movement
    selection_mode: bool,
    /// Where `v` started a visual selection, as `(line, col)`. The selection
    /// runs from here to the cursor, both ends included. Only set in
    /// selection mode.
    visual_anchor: Option<(usize, usize)>,
    /// Last rendered area
    last_area: std::cell::Cell<Rect>,
    /// Highlighted lines for the code currently displayed, shared via `Rc` so
    /// swapping to a cached result (see [`highlight_cache`](Self::highlight_cache))
    /// is O(1) rather than a deep clone.
    cached_lines: RefCell<Rc<Vec<Line<'static>>>>,
    /// Highlighted output keyed by a hash of (extension + code), so revisiting a
    /// step reuses its highlight instead of re-running syntect over the whole
    /// file. Keyed by content (not path) so an on-disk edit naturally misses.
    /// Cleared on theme change; capped to bound memory.
    highlight_cache: RefCell<HashMap<u64, Rc<Vec<Line<'static>>>>>,
    /// Optional file name header displayed above the code
    file_header: Option<String>,
    /// Theme used for syntax highlighting. Shared via `Arc` so cloning a
    /// `CodePreview` does not duplicate the (large) theme data.
    theme: Arc<Theme>,
    /// Inline comment threads and their display-row layout. Boxed so the
    /// preview (a [`TabContent`](crate::browser) variant) stays small.
    inline: Box<InlineState>,
}

/// Inline comment state for a [`CodePreview`].
#[derive(Debug, Clone, Default)]
struct InlineState {
    /// Anchored comment threads rendered as bubbles under their code.
    threads: Vec<InlineThread>,
    /// Root ids of folded threads. Kept across reloads of the same threads.
    folded: HashSet<String>,
    /// Display-row mapping (code lines + bubble rows). Rebuilt lazily when
    /// `layout_key` no longer matches.
    layout: RefCell<RowLayout>,
    /// `(generation, bubble text width)` the layout was built for.
    layout_key: Cell<Option<(u64, usize)>>,
    /// Bumped whenever code, threads or folds change.
    layout_gen: Cell<u64>,
    /// Root id of the thread whose bubble the cursor is on. `None` while the
    /// cursor is on a code line (`selected_line`); the bubble always sits
    /// under `selected_line`.
    cursor_on: Option<String>,
}

impl CodePreview {
    /// Create a new code preview.
    pub fn new(code: impl Into<String>, extension: impl Into<String>) -> Self {
        let code = code.into();
        let extension = extension.into();
        let preview = Self {
            code,
            extension,
            scroll_offset: Cell::new(0),
            selected_line: None,
            cursor_col: 0,
            selected_range: None,
            focused: false,
            selection_mode: false,
            visual_anchor: None,
            last_area: std::cell::Cell::new(Rect::default()),
            cached_lines: RefCell::new(Rc::new(Vec::new())),
            highlight_cache: RefCell::new(HashMap::new()),
            file_header: None,
            theme: default_theme(),
            inline: Box::default(),
        };
        preview.refresh_cache();
        preview
    }

    /// Set the code to display.
    pub fn set_code(&mut self, code: String) {
        self.code = code;
        self.reset_view();
        self.refresh_cache();
    }

    /// Set the code and extension together, reusing a cached highlight when the
    /// same content was rendered before.
    ///
    /// This is the hot path for stepping through a collection: re-tokenizing a
    /// whole file with syntect is the dominant per-step cost, so a repeat visit
    /// swaps in the cached result in O(1). Setting both fields up front also
    /// avoids the double highlight that `set_code` + `set_extension` would incur
    /// when the language changes between steps.
    pub fn set_code_keyed(&mut self, code: String, extension: String) {
        self.code = code;
        self.extension = extension;
        self.reset_view();

        let key = self.highlight_key();
        if let Some(lines) = self.highlight_cache.borrow().get(&key) {
            *self.cached_lines.borrow_mut() = Rc::clone(lines);
            self.invalidate_layout();
            return;
        }
        let lines = Rc::new(self.build_highlighted_lines());
        let mut cache = self.highlight_cache.borrow_mut();
        // Crude cap: distinct files in a collection are few, so clearing on
        // overflow effectively never fires, yet bounds a pathological session.
        if cache.len() >= HIGHLIGHT_CACHE_CAP {
            cache.clear();
        }
        cache.insert(key, Rc::clone(&lines));
        *self.cached_lines.borrow_mut() = lines;
        drop(cache);
        self.invalidate_layout();
    }

    /// Set the file extension.
    pub fn set_extension(&mut self, extension: String) {
        if self.extension != extension {
            self.extension = extension;
            self.refresh_cache();
        }
    }

    /// Reset scroll/selection view state to defaults (shared by the `set_code*`
    /// entry points).
    fn reset_view(&mut self) {
        self.inline.threads.clear();
        self.inline.cursor_on = None;
        self.invalidate_layout();
        self.scroll_offset.set(0);
        self.selected_line = None;
        self.cursor_col = 0;
        self.selection_mode = false;
        self.visual_anchor = None;
        self.selected_range = None;
    }

    /// Cache key for the current highlight: a hash of extension + code. The
    /// theme is not folded in because a theme change clears the whole cache.
    fn highlight_key(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.extension.hash(&mut hasher);
        self.code.hash(&mut hasher);
        hasher.finish()
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
        // Cached highlights carry the old theme's colors; drop them.
        self.highlight_cache.borrow_mut().clear();
        self.refresh_cache();
    }

    /// Rebuild the highlight for the current code and store it as the displayed
    /// lines. Does not touch the keyed [`highlight_cache`](Self::highlight_cache)
    /// — that is populated only via [`set_code_keyed`](Self::set_code_keyed).
    fn refresh_cache(&self) {
        *self.cached_lines.borrow_mut() = Rc::new(self.build_highlighted_lines());
        self.invalidate_layout();
    }

    /// Syntax-highlight the current code into gutter-prefixed lines.
    fn build_highlighted_lines(&self) -> Vec<Line<'static>> {
        let syntax_set = &*SYNTAX_SET;
        let theme = &*self.theme;
        let syntax = syntax_set
            .find_syntax_by_extension(&self.extension)
            .unwrap_or_else(|| syntax_set.find_syntax_plain_text());

        let mut h = HighlightLines::new(syntax, theme);
        let mut highlighted = Vec::new();

        // Pad every line number to the same width (the file's widest number, min
        // 3 digits) so the gutter is uniform across all lines. `{:>3}` alone
        // leaves lines 1-999 narrower than lines 1000+ in the same file, which
        // would desync the rendered gutter from `gutter_width()` and misplace
        // the cursor on the narrower lines.
        let total_lines = LinesWithEndings::from(&self.code).count();
        let num_width = line_number_width(total_lines);

        for (i, line) in LinesWithEndings::from(&self.code).enumerate() {
            let ranges: Vec<(SyntectStyle, &str)> =
                h.highlight_line(line, syntax_set).unwrap_or_default();
            let mut spans = Vec::new();

            // Gutter = 1-char sign column + line number padded to `num_width` + a
            // trailing space. Sign column shows '┃' for lines in the bookmark range.
            let sign = " "; // Default: no mark
            spans.push(Span::styled(sign, Style::default().fg(crate::theme::palette().dim)));

            // Add line number (gutter)
            let line_num = format!("{:>num_width$} ", i + 1);
            spans.push(Span::styled(line_num, Style::default().fg(crate::theme::palette().dim)));

            // Comment sign column (`▾` / `▸` where a comment thread sits under
            // the line), filled in at render time, then a separator space.
            spans.push(Span::raw(" "));
            spans.push(Span::raw(" "));

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
        highlighted
    }

    // @lat: [[tui-line-range-selection#CodePreview Component#Jump to Range]]
    /// Jump to and select a specific line index (0-indexed).
    pub fn jump_to_line(&mut self, line_index: usize) {
        self.jump_to_range(line_index, None);
    }

    /// Jump to and select a line range (0-indexed).
    /// If end is None, only the start line is highlighted.
    pub fn jump_to_range(&mut self, start: usize, end: Option<usize>) {
        self.inline.cursor_on = None;
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

            let (start_row, end_row) = (self.row_of(start), self.row_of(end_value));
            if area_height > 0 {
                self.scroll_offset.set(self.calculate_scroll_offset(
                    start_row,
                    end_row,
                    area_height,
                ));
            } else {
                // No area info yet - set a reasonable default
                self.scroll_offset.set(start_row.saturating_sub(2) as u16);
            }
        }
    }

    /// Clear the line selection and range highlight, leaving the code and scroll
    /// position untouched.
    ///
    /// Used when a bookmark's query no longer resolves: the preview still shows
    /// the file, but without a range highlight (or gutter `┃`) that would point
    /// at a stale, misleading location.
    pub fn clear_selection(&mut self) {
        self.selected_line = None;
        self.inline.cursor_on = None;
        self.selected_range = None;
        self.selection_mode = false;
        self.visual_anchor = None;
        self.cursor_col = 0;
    }

    /// Calculate the scroll offset for a given range of display rows.
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
        let max_offset = self.total_rows().saturating_sub(height) as i32;
        let old_offset = self.scroll_offset.get();
        self.scroll_offset
            .set((self.scroll_offset.get() as i32 + delta).clamp(0, max_offset) as u16);
        old_offset != self.scroll_offset.get()
    }

    /// Width of the gutter (range sign + line number + space + comment sign +
    /// space) that precedes code content, used to map a code column to a
    /// buffer x position.
    ///
    /// Derived from the same [`line_number_width`] that `refresh_cache` pads with,
    /// so the cursor's x offset matches the rendered gutter for every line.
    fn gutter_width(&self) -> u16 {
        (1 + line_number_width(self.line_count()) + 1 + 2) as u16
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
        let scrollbar = if self.total_rows() > height { 1 } else { 0 };
        (area.width as usize).saturating_sub(self.gutter_width() as usize + scrollbar)
    }

    /// The furthest column the cursor may occupy on `line_idx`: clamped to both
    /// the line length and the visible content width so the cursor stays on
    /// screen (there is no horizontal scroll yet).
    ///
    /// Before the first render `last_area` is unset (0×0), so `content_width()`
    /// is 0; in that case bound only by the line length rather than pinning the
    /// cursor to column 0 until a frame establishes the viewport.
    fn max_cursor_col(&self, line_idx: usize) -> usize {
        let line_len = self.line_char_len(line_idx);
        match self.content_width() {
            0 => line_len,
            width => line_len.min(width - 1),
        }
    }

    /// Clamp `cursor_col` to the current line after a vertical move.
    fn clamp_cursor_col(&mut self) {
        if let Some(line) = self.selected_line {
            self.cursor_col = self.cursor_col.min(self.max_cursor_col(line));
            // A visual selection includes the cursor's character, so keep it
            // on one rather than past the line's end.
            if self.visual_anchor.is_some() {
                self.cursor_col = self.cursor_col.min(self.line_char_len(line).saturating_sub(1));
            }
        }
    }

    /// The code currently displayed.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Cursor position: the line (0-indexed) and, while in selection mode,
    /// the column. `None` before the cursor is placed.
    pub fn cursor_position(&self) -> Option<(usize, Option<usize>)> {
        self.selected_line.map(|line| (line, self.selection_mode.then_some(self.cursor_col)))
    }

    /// The visual (`v`) selection as `((line, col), (line, col))`: ordered,
    /// 0-indexed, both ends included, columns in chars. `None` outside
    /// visual mode.
    pub fn visual_selection(&self) -> Option<((usize, usize), (usize, usize))> {
        let anchor = self.visual_anchor?;
        let cursor = (self.selected_line?, self.cursor_col);
        Some(if anchor <= cursor { (anchor, cursor) } else { (cursor, anchor) })
    }

    /// Leave visual mode, keeping the column cursor where it is.
    pub fn exit_visual(&mut self) {
        self.visual_anchor = None;
    }

    /// `v`: start a visual selection at the cursor (at the line's first
    /// non-blank character when there is no column cursor yet), or end the
    /// current one. Does nothing on a comment bubble.
    fn toggle_visual(&mut self) -> bool {
        if self.visual_anchor.take().is_some() {
            return true;
        }
        let Some(line) = self.selected_line else { return false };
        if self.inline.cursor_on.is_some() {
            return true;
        }
        if !self.selection_mode {
            self.selection_mode = true;
            self.cursor_col = self.first_non_blank(line);
        }
        // The column cursor alone may sit just past the line's end; a
        // selection includes its ends, so start it on a character.
        self.cursor_col = self.cursor_col.min(self.line_char_len(line).saturating_sub(1));
        self.visual_anchor = Some((line, self.cursor_col));
        true
    }

    /// Char column of the first non-blank character on `line_idx` (0 on a
    /// blank line).
    fn first_non_blank(&self, line_idx: usize) -> usize {
        let text = self.code.lines().nth(line_idx).unwrap_or("");
        if text.trim().is_empty() {
            return 0;
        }
        text.chars().take_while(|c| c.is_whitespace()).count()
    }

    /// Vim-style motions on the cursor line while the column cursor is on:
    /// `w` / `b` / `e` by word, `0` / `^` / `$` to the line's start, first
    /// non-blank and last character. Returns `false` outside selection mode
    /// so the key falls through.
    fn line_motion(&mut self, key: char) -> bool {
        if !self.selection_mode || self.inline.cursor_on.is_some() {
            return false;
        }
        let Some(line) = self.selected_line else { return false };
        let chars: Vec<char> = self.code.lines().nth(line).unwrap_or("").chars().collect();
        let n = chars.len();
        let class = |i: usize| chars.get(i).map(|&c| char_class(c));
        let mut i = self.cursor_col.min(n);
        match key {
            'w' => {
                let start = class(i);
                while i < n && class(i) == start {
                    i += 1;
                }
                while i < n && class(i) == Some(CharClass::Blank) {
                    i += 1;
                }
            }
            'e' => {
                i += 1;
                while i < n && class(i) == Some(CharClass::Blank) {
                    i += 1;
                }
                let word = class(i);
                while i + 1 < n && class(i + 1) == word {
                    i += 1;
                }
            }
            'b' => {
                while i > 0 && class(i - 1) == Some(CharClass::Blank) {
                    i -= 1;
                }
                if let Some(word) = i.checked_sub(1).and_then(class) {
                    while i > 0 && class(i - 1) == Some(word) {
                        i -= 1;
                    }
                }
            }
            '0' => i = 0,
            '^' => i = self.first_non_blank(line),
            '$' => i = n.saturating_sub(1),
            _ => return false,
        }
        // Stay on a character: a visual selection includes its last column.
        self.cursor_col = i.min(n.saturating_sub(1)).min(self.max_cursor_col(line));
        true
    }

    /// Replace the inline comment threads, keeping folds for threads that are
    /// still present and the cursor's line at the same place on screen.
    pub fn set_threads(&mut self, mut threads: Vec<InlineThread>) {
        threads.sort_by_key(|t| (t.anchor_end, t.anchor_start));
        if threads == self.inline.threads {
            return;
        }
        self.inline.folded.retain(|id| threads.iter().any(|t| &t.root_id == id));
        self.relayout_keeping_cursor(|preview| preview.inline.threads = threads);
        self.drop_hidden_cursor_bubble();
    }

    /// The inline threads, in display order.
    pub fn threads(&self) -> &[InlineThread] {
        &self.inline.threads
    }

    /// Index of the thread whose bubble the cursor is on.
    fn cursor_thread(&self) -> Option<usize> {
        let id = self.inline.cursor_on.as_deref()?;
        self.inline.threads.iter().position(|t| t.root_id == id)
    }

    /// The thread the cursor is on: the bubble under the cursor, else the
    /// narrowest thread whose anchored lines contain the cursor line. Only
    /// while the preview has focus.
    fn focused_thread(&self) -> Option<usize> {
        if !self.focused {
            return None;
        }
        if let Some(t) = self.cursor_thread() {
            return Some(t);
        }
        let line = self.selected_line?;
        self.inline
            .threads
            .iter()
            .enumerate()
            .filter(|(_, t)| t.anchor_start <= line && line <= t.anchor_end)
            .min_by_key(|(_, t)| t.anchor_end - t.anchor_start)
            .map(|(i, _)| i)
    }

    /// Root id of the thread the cursor is on (see [`Self::focused_thread`]).
    pub fn focused_thread_id(&self) -> Option<&str> {
        self.focused_thread().map(|i| self.inline.threads[i].root_id.as_str())
    }

    /// The line a thread's bubble sits under (its anchor end, clamped).
    fn thread_line(&self, thread: &InlineThread) -> usize {
        thread.anchor_end.min(self.line_count().saturating_sub(1))
    }

    /// Indices of threads whose bubbles sit under `line`, in display order.
    fn threads_under(&self, line: usize) -> Vec<usize> {
        (0..self.inline.threads.len())
            .filter(|&i| self.thread_line(&self.inline.threads[i]) == line)
            .collect()
    }

    /// [`Self::threads_under`], expanded ones only: the bubbles the cursor
    /// can stop on.
    fn bubbles_under(&self, line: usize) -> Vec<usize> {
        self.threads_under(line)
            .into_iter()
            .filter(|&i| !self.inline.folded.contains(&self.inline.threads[i].root_id))
            .collect()
    }

    /// `z`: on a bubble, fold that thread (the cursor returns to its line);
    /// on a code line with threads under it, fold them all, or unfold them
    /// all when every one is folded. Returns `false` when there is nothing to
    /// fold, so the key can fall through.
    fn toggle_fold(&mut self) -> bool {
        if let Some(t) = self.cursor_thread() {
            let id = self.inline.threads[t].root_id.clone();
            self.relayout_keeping_cursor(|preview| {
                preview.inline.folded.insert(id);
                preview.inline.cursor_on = None;
            });
            return true;
        }
        let Some(line) = self.selected_line else { return false };
        let ids: Vec<String> = self
            .threads_under(line)
            .into_iter()
            .map(|i| self.inline.threads[i].root_id.clone())
            .collect();
        if ids.is_empty() {
            return false;
        }
        let all_folded = ids.iter().all(|id| self.inline.folded.contains(id));
        self.relayout_keeping_cursor(|preview| {
            for id in ids {
                if all_folded {
                    preview.inline.folded.remove(&id);
                } else {
                    preview.inline.folded.insert(id);
                }
            }
        });
        true
    }

    /// Clear `cursor_on` when its thread was removed or folded away.
    fn drop_hidden_cursor_bubble(&mut self) {
        if let Some(id) = self.inline.cursor_on.clone()
            && (self.cursor_thread().is_none() || self.inline.folded.contains(&id))
        {
            self.inline.cursor_on = None;
        }
    }

    /// Move the cursor one stop down: through each expanded bubble under the
    /// current line, then to the next line.
    fn cursor_down(&mut self) {
        let line_count = self.line_count();
        if line_count == 0 {
            return;
        }
        let Some(line) = self.selected_line else {
            self.selected_line = Some(0);
            return;
        };
        let bubbles = self.bubbles_under(line);
        let next_bubble = match self.cursor_thread() {
            Some(t) => bubbles.iter().position(|&b| b == t).and_then(|p| bubbles.get(p + 1)),
            None => bubbles.first(),
        };
        if let Some(&b) = next_bubble {
            self.inline.cursor_on = Some(self.inline.threads[b].root_id.clone());
        } else if line + 1 < line_count {
            self.inline.cursor_on = None;
            self.selected_line = Some(line + 1);
        }
    }

    /// Move the cursor one stop up: through the bubbles under the current
    /// line back to the line, then into the previous line's last bubble.
    fn cursor_up(&mut self) {
        let Some(line) = self.selected_line else {
            self.selected_line = Some(0);
            return;
        };
        if let Some(t) = self.cursor_thread() {
            let bubbles = self.bubbles_under(line);
            let previous = bubbles.iter().position(|&b| b == t).filter(|&p| p > 0);
            self.inline.cursor_on =
                previous.map(|p| self.inline.threads[bubbles[p - 1]].root_id.clone());
            return;
        }
        if line == 0 {
            return;
        }
        self.selected_line = Some(line - 1);
        self.inline.cursor_on =
            self.bubbles_under(line - 1).last().map(|&b| self.inline.threads[b].root_id.clone());
    }

    /// Move the cursor a whole code line while extending a visual selection,
    /// passing over comment bubbles.
    fn visual_line_step(&mut self, down: bool) {
        let Some(line) = self.selected_line else { return };
        let next = if down { line + 1 } else { line.wrapping_sub(1) };
        if next < self.line_count() {
            self.inline.cursor_on = None;
            self.selected_line = Some(next);
        }
    }

    /// Display rows the cursor covers: its bubble, or its code line.
    fn cursor_rows(&self) -> Option<(usize, usize)> {
        if let Some(t) = self.cursor_thread()
            && let Some(rows) = self.layout().bubble_rows(t)
        {
            return Some(rows);
        }
        self.selected_line.map(|line| {
            let row = self.row_of(line);
            (row, row)
        })
    }

    /// Scroll just enough to show the cursor; a whole bubble when it fits.
    fn scroll_to_cursor(&mut self) {
        let Some((first, last)) = self.cursor_rows() else { return };
        let height = self.last_area.get().height as usize;
        if height == 0 {
            return;
        }
        let offset = self.scroll_offset.get() as usize;
        if last >= offset + height {
            self.scroll_offset.set((last + 1).saturating_sub(height).min(first) as u16);
        } else if first < offset {
            self.scroll_offset.set(first as u16);
        }
    }

    /// `n` / `N`: jump to the next (or previous) comment thread in display
    /// order, wrapping around. A folded target is unfolded so it can be read.
    /// Returns `false` when there are no threads.
    fn jump_to_thread(&mut self, forward: bool) -> bool {
        let count = self.inline.threads.len();
        if count == 0 {
            return false;
        }
        let line = self.selected_line.unwrap_or(0);
        let target = match (self.cursor_thread(), forward) {
            (Some(t), true) => (t + 1) % count,
            (Some(t), false) => (t + count - 1) % count,
            (None, true) => {
                (0..count).find(|&i| self.thread_line(&self.inline.threads[i]) >= line).unwrap_or(0)
            }
            (None, false) => (0..count)
                .rev()
                .find(|&i| self.thread_line(&self.inline.threads[i]) < line)
                .unwrap_or(count - 1),
        };
        let id = self.inline.threads[target].root_id.clone();
        if self.inline.folded.remove(&id) {
            self.invalidate_layout();
        }
        self.selected_line = Some(self.thread_line(&self.inline.threads[target]));
        // A selection can't end on a bubble: leave visual mode.
        self.visual_anchor = None;
        self.inline.cursor_on = Some(id);
        self.scroll_to_cursor();
        true
    }

    /// `Z`: fold every thread, or unfold them all when all are folded.
    fn toggle_fold_all(&mut self) -> bool {
        if self.inline.threads.is_empty() {
            return false;
        }
        let ids: Vec<String> = self.inline.threads.iter().map(|t| t.root_id.clone()).collect();
        let all_folded = ids.iter().all(|id| self.inline.folded.contains(id));
        self.relayout_keeping_cursor(|preview| {
            if all_folded {
                preview.inline.folded.clear();
            } else {
                preview.inline.folded.extend(ids);
            }
        });
        true
    }

    /// Put the cursor on whatever is drawn at screen row `y`: a code line or
    /// a comment bubble. Returns whether the row held anything.
    fn click_row(&mut self, y: u16) -> bool {
        let area = self.last_area.get();
        let height = area.height as usize;
        let offset =
            (self.scroll_offset.get() as usize).min(self.total_rows().saturating_sub(height));
        self.scroll_offset.set(offset as u16);
        let row = offset + y.saturating_sub(area.y) as usize;
        let Some(&display_row) = self.layout().rows.get(row) else { return false };
        match display_row {
            DisplayRow::Code(line) => {
                self.selected_line = Some(line);
                self.inline.cursor_on = None;
                self.clamp_cursor_col();
            }
            DisplayRow::Bubble { thread, .. } => {
                let thread = &self.inline.threads[thread];
                self.selected_line = Some(self.thread_line(thread));
                self.inline.cursor_on = Some(thread.root_id.clone());
            }
        }
        true
    }

    /// Apply `change` (which alters threads or folds), then shift the scroll
    /// offset so the cursor's line stays on the same screen row even when
    /// rows above it appeared or vanished.
    fn relayout_keeping_cursor(&mut self, change: impl FnOnce(&mut Self)) {
        let anchor = self.selected_line.unwrap_or(0);
        let screen_row = self.row_of(anchor) as i64 - self.scroll_offset.get() as i64;
        change(self);
        self.invalidate_layout();
        self.drop_hidden_cursor_bubble();
        let height = self.last_area.get().height as usize;
        if height == 0 {
            // No frame yet, so no screen position to keep.
            return;
        }
        let max_offset = self.total_rows().saturating_sub(height) as i64;
        self.scroll_offset
            .set((self.row_of(anchor) as i64 - screen_row).clamp(0, max_offset) as u16);
    }

    fn invalidate_layout(&self) {
        self.inline.layout_gen.set(self.inline.layout_gen.get().wrapping_add(1));
    }

    /// Room for bubble text at the current width; `0` before the first render
    /// (no wrapping). Always reserves the scrollbar column so the width does
    /// not depend on the row count it determines.
    fn bubble_text_width(&self) -> usize {
        let width = self.last_area.get().width as usize;
        if width == 0 {
            return 0;
        }
        width.saturating_sub(self.gutter_width() as usize + 1 + BUBBLE_CHROME).max(1)
    }

    /// The display-row layout, rebuilt if code, threads, folds or width changed.
    fn layout(&self) -> std::cell::Ref<'_, RowLayout> {
        let key = (self.inline.layout_gen.get(), self.bubble_text_width());
        let previous = self.inline.layout_key.get();
        if previous != Some(key) {
            // Only the width changed (a resize): bubbles re-wrap, so rows above
            // the cursor can grow or shrink. Note where the cursor sits on
            // screen to put it back there.
            let resized = previous.is_some_and(|(generation, _)| generation == key.0);
            let cursor_screen_row = resized
                .then(|| self.cursor_first_row(&self.inline.layout.borrow()))
                .flatten()
                .map(|row| row as i64 - self.scroll_offset.get() as i64);
            let layout =
                build_layout(self.line_count(), &self.inline.threads, &self.inline.folded, key.1);
            if let Some(screen_row) = cursor_screen_row
                && let Some(row) = self.cursor_first_row(&layout)
            {
                let height = self.last_area.get().height as usize;
                let max_offset = layout.total_rows().saturating_sub(height) as i64;
                self.scroll_offset.set((row as i64 - screen_row).clamp(0, max_offset) as u16);
            }
            *self.inline.layout.borrow_mut() = layout;
            self.inline.layout_key.set(Some(key));
        }
        self.inline.layout.borrow()
    }

    /// First display row of the cursor in `layout`: its bubble's top row, or
    /// its code line.
    fn cursor_first_row(&self, layout: &RowLayout) -> Option<usize> {
        if let Some(t) = self.cursor_thread()
            && let Some((first, _)) = layout.bubble_rows(t)
        {
            return Some(first);
        }
        self.selected_line.map(|line| layout.row_of(line))
    }

    /// Display row of source line `line`.
    fn row_of(&self, line: usize) -> usize {
        self.layout().row_of(line)
    }

    /// Display rows: source lines plus expanded bubble rows.
    fn total_rows(&self) -> usize {
        self.layout().total_rows()
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
        let height = code_area.height as usize;
        let layout = self.layout();
        let list_len = layout.total_rows();
        let palette = crate::theme::palette();
        let focused_thread = self.focused_thread();
        let gutter = self.gutter_width() as usize;
        let row_width = code_area.width as usize;

        // Comment sign per line with threads under it: `▾` while any is
        // expanded, `▸` when all are folded.
        let mut comment_signs: HashMap<usize, bool> = HashMap::new();
        for t in &self.inline.threads {
            let line = self.thread_line(t);
            let expanded = !self.inline.folded.contains(&t.root_id);
            *comment_signs.entry(line).or_default() |= expanded;
        }
        let cursor_thread = self.cursor_thread().filter(|_| self.focused);

        // Build only the visible rows: the paragraph is rendered pre-scrolled.
        // Clamp to the content: an offset set before the first frame (no
        // height known yet) or before bubbles were folded can overshoot, which
        // would hide the top of a file that fits on screen.
        let offset = (self.scroll_offset.get() as usize).min(list_len.saturating_sub(height));
        let mut text_lines = Vec::with_capacity(height);
        for display_row in layout.rows.iter().skip(offset).take(height) {
            match *display_row {
                DisplayRow::Code(i) => {
                    let Some(line) = cached.get(i) else { continue };
                    let in_range =
                        selected_range.is_some_and(|(start, end)| i >= start && i <= end);
                    // Sign column: ┃ (marker colour) for lines in the bookmark range.
                    let (sign, sign_color) =
                        if in_range { ("┃", palette.marker) } else { (" ", palette.dim) };
                    let mut new_spans = vec![Span::styled(sign, Style::default().fg(sign_color))];
                    // The cached line is range sign, line number, comment sign,
                    // separator, then content: swap in both signs.
                    new_spans.extend(line.spans.get(1).cloned());
                    new_spans.push(match comment_signs.get(&i) {
                        Some(true) => Span::styled("▾", Style::default().fg(palette.accent)),
                        Some(false) => Span::styled("▸", Style::default().fg(palette.accent)),
                        None => Span::raw(" "),
                    });
                    new_spans.extend(line.spans.iter().skip(3).cloned());
                    text_lines.push(Line::from(new_spans));
                }
                DisplayRow::Bubble { thread, row } => {
                    let focused = focused_thread == Some(thread);
                    let style = BubbleStyle {
                        border: if focused { palette.marker } else { palette.gray },
                        text: palette.emphasis,
                        dim: palette.dim,
                        accent: palette.accent,
                    };
                    text_lines.push(render_bubble_line(
                        &layout.bubbles[thread][row],
                        gutter,
                        // Leave the last column to the scrollbar.
                        row_width.saturating_sub(1),
                        focused,
                        &style,
                    ));
                }
            }
        }

        // The focused comment's node as (display row, from cell, to cell).
        let node_rows: Vec<(usize, usize, usize)> = focused_thread
            .map(|t| {
                let t = &self.inline.threads[t];
                span_cells(
                    &self.code,
                    (t.anchor_start, t.start_col),
                    (t.anchor_end, t.end_col),
                    true,
                )
                .into_iter()
                .map(|(line, from, to)| (layout.row_of(line), from, to))
                .collect()
            })
            .unwrap_or_default();
        // The visual selection, end column included.
        let visual_rows: Vec<(usize, usize, usize)> = self
            .visual_selection()
            .filter(|_| self.focused)
            .map(|(start, (end_line, end_col))| {
                span_cells(&self.code, start, (end_line, end_col + 1), false)
                    .into_iter()
                    .map(|(line, from, to)| (layout.row_of(line), from, to))
                    .collect()
            })
            .unwrap_or_default();
        // The cursor is on a code line, or on a bubble (shown by its border).
        let cursor_bubble = cursor_thread.and_then(|t| layout.bubble_rows(t));
        let selected_row =
            self.selected_line.filter(|_| cursor_bubble.is_none()).map(|line| layout.row_of(line));
        drop(layout);
        drop(cached); // Drop cached before rendering

        ratatui::widgets::Paragraph::new(text_lines).render(code_area, buf);

        let paint_row = |buf: &mut Buffer, row: usize, color: Color| {
            if row >= offset && row - offset < height {
                let y = code_area.y + (row - offset) as u16;
                for x in code_area.left()..code_area.right() {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_bg(color);
                    }
                }
            }
        };

        // Highlight the selected line's full-width background so it reads the
        // same as the list panes' selected row (see `crate::theme::SELECTION_BG`).
        // Painted after the paragraph and before the scrollbar so the scrollbar
        // thumb keeps its own styling on the highlighted row.
        if let Some(row) = selected_row {
            paint_row(buf, row, crate::theme::SELECTION_BG);
        }
        // The focused comment's node: just its characters, over the cursor
        // line's background, so the exact target reads at a glance.
        let code_x = code_area.x + self.gutter_width();
        let paint_cells = |buf: &mut Buffer, rows: &[(usize, usize, usize)], color: Color| {
            for &(row, from, to) in rows {
                if row < offset || row - offset >= height {
                    continue;
                }
                let y = code_area.y + (row - offset) as u16;
                for col in from..to.max(from + 1) {
                    let x = code_x + col as u16;
                    if x >= code_area.right().saturating_sub(1) {
                        break;
                    }
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_bg(color);
                    }
                }
            }
        };
        paint_cells(buf, &node_rows, crate::theme::COMMENT_ANCHOR_BG);
        // The visual selection sits over the node so its extent stays visible.
        paint_cells(buf, &visual_rows, crate::theme::VISUAL_BG);

        // Draw the block cursor on the focused row/column. Painted on top of the
        // selection background by reversing the cell under the cursor, so it
        // keeps the underlying character and stays theme-independent.
        if self.focused
            && self.selection_mode
            && let Some(row) = selected_row
            && row >= offset
        {
            let screen_row = (row - offset) as u16;
            // Terminal cells, like the selection: `界` takes two.
            let line = self.code.lines().nth(self.selected_line.unwrap_or(0)).unwrap_or("");
            let cells = unicode_width::UnicodeWidthStr::width(
                line.chars().take(self.cursor_col).collect::<String>().as_str(),
            );
            let x = code_area.x + self.gutter_width() + cells as u16;
            if screen_row < code_area.height && x < code_area.right() {
                let y = code_area.y + screen_row;
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
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
            let mut scrollbar_state = ScrollbarState::new(scroll_positions).position(offset);

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
                        // Comment bubbles are cursor stops between lines,
                        // except while extending a visual selection.
                        if self.visual_anchor.is_some() {
                            self.visual_line_step(true);
                        } else {
                            self.cursor_down();
                        }
                        self.scroll_to_cursor();
                        self.clamp_cursor_col();
                        true
                    }
                    ratatui::crossterm::event::KeyCode::Up
                    | ratatui::crossterm::event::KeyCode::Char('k') => {
                        if self.visual_anchor.is_some() {
                            self.visual_line_step(false);
                        } else {
                            self.cursor_up();
                        }
                        self.scroll_to_cursor();
                        self.clamp_cursor_col();
                        true
                    }
                    ratatui::crossterm::event::KeyCode::Char('z') => self.toggle_fold(),
                    ratatui::crossterm::event::KeyCode::Char('Z') => self.toggle_fold_all(),
                    ratatui::crossterm::event::KeyCode::Char('n') => self.jump_to_thread(true),
                    ratatui::crossterm::event::KeyCode::Char('N') => self.jump_to_thread(false),
                    ratatui::crossterm::event::KeyCode::Enter => {
                        self.selection_mode = !self.selection_mode;
                        if !self.selection_mode {
                            self.visual_anchor = None;
                        }
                        true
                    }
                    ratatui::crossterm::event::KeyCode::Char('v') => self.toggle_visual(),
                    ratatui::crossterm::event::KeyCode::Char(
                        c @ ('w' | 'b' | 'e' | '0' | '^' | '$'),
                    ) => self.line_motion(c),
                    ratatui::crossterm::event::KeyCode::Esc => {
                        if self.visual_anchor.take().is_some() {
                            true
                        } else if self.selection_mode {
                            self.selection_mode = false;
                            true
                        } else {
                            false
                        }
                    }
                    ratatui::crossterm::event::KeyCode::Left
                    | ratatui::crossterm::event::KeyCode::Char('h') => {
                        if self.selection_mode
                            && self.selected_line.is_some()
                            && self.inline.cursor_on.is_none()
                        {
                            self.cursor_col = self.cursor_col.saturating_sub(1);
                            true
                        } else {
                            false
                        }
                    }
                    ratatui::crossterm::event::KeyCode::Right
                    | ratatui::crossterm::event::KeyCode::Char('l') => {
                        if self.selection_mode
                            && self.inline.cursor_on.is_none()
                            && let Some(line) = self.selected_line
                        {
                            self.cursor_col = (self.cursor_col + 1).min(self.max_cursor_col(line));
                            self.clamp_cursor_col();
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
                    ratatui::crossterm::event::MouseEventKind::Down(
                        ratatui::crossterm::event::MouseButton::Left,
                    ) if is_hovered => self.click_row(mouse.row),
                    ratatui::crossterm::event::MouseEventKind::ScrollDown if is_hovered => {
                        let height = area.height as usize;
                        let total_rows = self.total_rows();
                        if total_rows > height {
                            let old_offset = self.scroll_offset.get();
                            self.scroll_offset.set(
                                self.scroll_offset
                                    .get()
                                    .saturating_add(1)
                                    .min((total_rows - height) as u16),
                            );
                            return old_offset != self.scroll_offset.get();
                        }
                        false
                    }
                    ratatui::crossterm::event::MouseEventKind::ScrollUp if is_hovered => {
                        let old_offset = self.scroll_offset.get();
                        self.scroll_offset.set(self.scroll_offset.get().saturating_sub(1));
                        old_offset != self.scroll_offset.get()
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
        if !focused {
            self.selection_mode = false;
            self.visual_anchor = None;
        }
    }
}

/// Character classes for word motions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Blank,
    Word,
    Punct,
}

fn char_class(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Blank
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

/// The span from `(line, col)` to `(line, col)` (end exclusive, columns in
/// chars) as `(line, from cell, to cell)` per line. Middle and leading lines
/// run to the line's end; with `skip_indent` middle and trailing lines start
/// at their first non-blank character (a node), else at column 0 (a
/// selection).
fn span_cells(
    code: &str,
    (start_line, start_col): (usize, usize),
    (end_line, end_col): (usize, usize),
    skip_indent: bool,
) -> Vec<(usize, usize, usize)> {
    code.lines()
        .enumerate()
        .skip(start_line)
        .take((end_line + 1).saturating_sub(start_line))
        .map(|(line, text)| {
            let indent = if skip_indent {
                text.chars().take_while(|c| c.is_whitespace()).count()
            } else {
                0
            };
            let from = if line == start_line { start_col } else { indent };
            let to = if line == end_line { end_col } else { text.chars().count() };
            // Terminal cells, not chars: `界` takes two cells and a combining
            // mark none.
            let cells = |chars: usize| -> usize {
                unicode_width::UnicodeWidthStr::width(
                    text.chars().take(chars).collect::<String>().as_str(),
                )
            };
            (line, cells(from), cells(to))
        })
        .collect()
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

    /// The rendered character at (x, y).
    fn cell_symbol(buf: &Buffer, x: u16, y: u16) -> String {
        buf.cell((x, y)).expect("cell within bounds").symbol().to_string()
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
    fn navigation_keys_unhandled_when_not_in_selection_mode() {
        let mut preview = CodePreview::new("one\ntwo\n", "txt");
        preview.set_focus(true);
        // Select the first line
        assert!(press(&mut preview, 'j'));

        // h and l return false (unhandled) when selection mode is off
        assert!(!press(&mut preview, 'h'));
        assert!(!press(&mut preview, 'l'));

        // Left and Right return false when selection mode is off
        let press_key = |preview: &mut CodePreview, code: ratatui::crossterm::event::KeyCode| {
            preview.handle_event(&Event::Key(ratatui::crossterm::event::KeyEvent::from(code)))
        };
        assert!(!press_key(&mut preview, ratatui::crossterm::event::KeyCode::Left));
        assert!(!press_key(&mut preview, ratatui::crossterm::event::KeyCode::Right));

        // Enter selection mode
        assert!(press(&mut preview, 'v'));

        // Now they return true (handled)
        assert!(press_key(&mut preview, ratatui::crossterm::event::KeyCode::Right));
        assert!(press_key(&mut preview, ratatui::crossterm::event::KeyCode::Left));
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

        // Select the first line, then enter selection mode to walk the cursor right with `l`.
        assert!(press(&mut preview, 'j'));
        assert_eq!(preview.selected_line, Some(0));
        assert!(press(&mut preview, 'v'));
        assert!(press(&mut preview, 'l'));
        assert!(press(&mut preview, 'l'));
        assert_eq!(preview.cursor_col, 2);

        // The block cursor is rendered (reversed) at gutter + cursor_col.
        let buf = render_to_buffer(&preview, 30, 4);
        assert!(cell_reversed(&buf, preview.gutter_width() + 2, 0));

        // `h` walks it back, saturating at column 0.
        assert!(press(&mut preview, 'h'));
        assert!(press(&mut preview, 'h'));
        assert!(press(&mut preview, 'h'));
        assert_eq!(preview.cursor_col, 0);
    }

    #[test]
    fn v_selects_a_range_with_word_and_line_motions() {
        let code = "    let total = sum(items);\nnext();\n";
        let mut preview = CodePreview::new(code, "txt");
        preview.set_focus(true);
        render_to_buffer(&preview, 40, 4);
        assert!(press(&mut preview, 'j'));
        assert_eq!(preview.visual_selection(), None);

        // From a plain line cursor, `v` starts at the first non-blank char.
        assert!(press(&mut preview, 'v'));
        assert_eq!(preview.visual_selection(), Some(((0, 4), (0, 4))));
        assert!(press(&mut preview, 'e'));
        assert_eq!(preview.visual_selection(), Some(((0, 4), (0, 6))), "end of `let`");
        assert!(press(&mut preview, 'w'));
        assert!(press(&mut preview, 'w'));
        assert!(press(&mut preview, 'w'));
        assert_eq!(preview.cursor_col, 16, "start of `sum`");
        assert!(press(&mut preview, 'b'));
        assert_eq!(preview.cursor_col, 14, "back to `=`");
        assert!(press(&mut preview, '$'));
        assert_eq!(preview.visual_selection(), Some(((0, 4), (0, 26))), "through `;`");
        assert!(press(&mut preview, '0'));
        assert_eq!(preview.visual_selection(), Some(((0, 0), (0, 4))), "ordered ends");
        assert!(press(&mut preview, '^'));
        assert_eq!(preview.cursor_col, 4);

        // Esc leaves visual mode but keeps the column cursor; `v` again
        // starts a new selection where the cursor is.
        assert!(press(&mut preview, 'l'));
        preview.handle_event(&key(ratatui::crossterm::event::KeyCode::Esc));
        assert_eq!(preview.visual_selection(), None);
        assert_eq!(preview.cursor_position(), Some((0, Some(5))));
        assert!(press(&mut preview, 'v'));
        assert_eq!(preview.visual_selection(), Some(((0, 5), (0, 5))));
        assert!(press(&mut preview, 'v'), "v toggles visual off");
        assert_eq!(preview.visual_selection(), None);
    }

    #[test]
    fn word_motions_fall_through_without_a_column_cursor() {
        let mut preview = CodePreview::new("one two\n", "txt");
        preview.set_focus(true);
        assert!(press(&mut preview, 'j'));
        for c in ['w', 'b', 'e', '0', '$'] {
            assert!(!press(&mut preview, c), "{c} is free for other bindings");
        }
    }

    #[test]
    fn visual_selection_spans_lines_and_is_painted() {
        let code = "fn a() {\n    let x = 1;\n    x\n}\n";
        let mut preview = CodePreview::new(code, "txt");
        preview.set_focus(true);
        render_to_buffer(&preview, 40, 6);
        assert!(press(&mut preview, 'j'));
        assert!(press(&mut preview, 'j'));
        assert!(press(&mut preview, 'v'));
        assert!(press(&mut preview, 'w'));
        assert!(press(&mut preview, 'j'));
        assert_eq!(preview.visual_selection(), Some(((1, 4), (2, 4))));

        let buf = render_to_buffer(&preview, 40, 6);
        let gutter = preview.gutter_width();
        let painted = |y: u16| -> Vec<u16> {
            (gutter..40).filter(|&x| buf[(x, y)].bg == crate::theme::VISUAL_BG).collect()
        };
        assert_eq!(painted(1), (gutter + 4..gutter + 14).collect::<Vec<_>>(), "`let x = 1;`");
        assert_eq!(painted(2), (gutter..gutter + 5).collect::<Vec<_>>(), "line start to `x`");
        assert!(painted(0).is_empty() && painted(3).is_empty());

        // Losing focus drops the selection.
        preview.set_focus(false);
        assert_eq!(preview.visual_selection(), None);
    }

    #[test]
    fn cursor_moves_before_first_render() {
        // `last_area` is unset (0x0) until the first render. The cursor must
        // still move on h/l rather than being pinned to column 0 by a zero
        // content width.
        let mut preview = CodePreview::new("one\ntwo\n", "txt");
        preview.set_focus(true);
        assert!(press(&mut preview, 'j'));
        assert!(press(&mut preview, 'v'));
        assert!(press(&mut preview, 'l'));
        assert_eq!(preview.cursor_col, 1);
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
        // The column cursor alone (Enter) may sit just past the line's end.
        preview.handle_event(&key(ratatui::crossterm::event::KeyCode::Enter));
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
    fn gutter_width_grows_past_999_lines_and_cursor_tracks_it() {
        // Up to 999 lines the line-number field is 3 digits:
        // gutter = range sign + 3 + space + comment sign + space.
        let small: String = (1..=10).map(|n| format!("line {n}\n")).collect();
        let preview = CodePreview::new(small, "txt");
        assert_eq!(preview.gutter_width(), 7);

        // At 1000+ lines the field widens to 4 digits, so the gutter (and the
        // cursor's x offset) must grow with it rather than stay pinned at 7.
        let big: String = (1..=1000).map(|n| format!("line {n}\n")).collect();
        let mut preview = CodePreview::new(big, "txt");
        assert_eq!(preview.gutter_width(), 8);

        preview.set_focus(true);
        render_to_buffer(&preview, 40, 6);
        press(&mut preview, 'j');
        press(&mut preview, 'v');
        press(&mut preview, 'l');
        let buf = render_to_buffer(&preview, 40, 6);
        let gutter = preview.gutter_width();

        // Verify against the *actual rendered characters*, not just the formula:
        // line 1 ("line 1") must be padded so its content starts exactly at the
        // gutter boundary even though it is a 1-3 digit number in a 4-digit file.
        assert_eq!(cell_symbol(&buf, gutter, 0), "l"); // column 0
        assert_eq!(cell_symbol(&buf, gutter + 1, 0), "i"); // column 1
        // The cursor (column 1) lands on that character.
        assert!(cell_reversed(&buf, gutter + 1, 0));
    }

    #[test]
    fn cursor_only_renders_when_focused() {
        let code = "one\ntwo\n";
        let mut preview = CodePreview::new(code, "txt");
        preview.set_focus(true);
        render_to_buffer(&preview, 30, 2);
        press(&mut preview, 'j');
        press(&mut preview, 'v');
        press(&mut preview, 'l');

        let buf = render_to_buffer(&preview, 30, 2);
        assert!(cell_reversed(&buf, preview.gutter_width() + 1, 0));

        // Blur the pane: the cursor disappears.
        preview.set_focus(false);
        let buf = render_to_buffer(&preview, 30, 2);
        assert!(!cell_reversed(&buf, preview.gutter_width() + 1, 0));
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
    fn clear_selection_removes_range_highlight_and_gutter() {
        let code = "one\ntwo\nthree\nfour\n";
        let mut preview = CodePreview::new(code, "txt");

        // Selecting a range highlights it and draws the gutter bar.
        preview.jump_to_range(1, Some(2));
        assert_eq!(preview.selected_range, Some((1, 2)));
        let buf = render_to_buffer(&preview, 20, 4);
        assert_eq!(cell_symbol(&buf, 0, 1), "┃");
        assert_eq!(row_bg(&buf, 1), crate::theme::SELECTION_BG);

        // Clearing the selection drops both the gutter bar and the row highlight
        // while leaving the code intact.
        preview.clear_selection();
        assert_eq!(preview.selected_line, None);
        assert_eq!(preview.selected_range, None);
        let buf = render_to_buffer(&preview, 20, 4);
        for y in 0..4 {
            assert_eq!(cell_symbol(&buf, 0, y), " ");
            assert_ne!(row_bg(&buf, y), crate::theme::SELECTION_BG);
        }
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

    #[test]
    fn set_code_keyed_reuses_highlight_on_revisit() {
        let mut preview = CodePreview::new("", "rs");

        preview.set_code_keyed("fn a() {}\n".to_string(), "rs".to_string());
        let first = Rc::clone(&preview.cached_lines.borrow());

        // Moving to a different step produces a distinct highlight.
        preview.set_code_keyed("fn b() {}\n".to_string(), "rs".to_string());
        assert!(!Rc::ptr_eq(&first, &preview.cached_lines.borrow()));

        // Returning to the first step swaps in the *same* cached Rc — no
        // re-highlight — which is the whole point of the cache.
        preview.set_code_keyed("fn a() {}\n".to_string(), "rs".to_string());
        assert!(Rc::ptr_eq(&first, &preview.cached_lines.borrow()));
    }

    #[test]
    fn set_theme_invalidates_keyed_cache() {
        let mut preview = CodePreview::new("", "rs");
        preview.set_code_keyed("fn a() {}\n".to_string(), "rs".to_string());
        let before = Rc::clone(&preview.cached_lines.borrow());

        preview.set_theme(Arc::new(ThemeRegistry::new().resolve("Dracula")));

        // A revisit after the theme change must re-highlight (fresh Rc), not
        // serve the stale pre-theme colors from the cache.
        preview.set_code_keyed("fn a() {}\n".to_string(), "rs".to_string());
        assert!(!Rc::ptr_eq(&before, &preview.cached_lines.borrow()));
    }

    fn key(code: ratatui::crossterm::event::KeyCode) -> Event {
        Event::Key(ratatui::crossterm::event::KeyEvent::from(code))
    }

    fn row_text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    #[test]
    fn v_from_past_the_line_end_starts_on_the_last_char() {
        let mut preview = CodePreview::new("ab", "txt");
        preview.set_focus(true);
        render_to_buffer(&preview, 20, 2);
        assert!(press(&mut preview, 'j'));
        preview.handle_event(&key(ratatui::crossterm::event::KeyCode::Enter));
        for _ in 0..3 {
            press(&mut preview, 'l');
        }
        assert_eq!(preview.cursor_col, 2, "the column cursor alone may pass the end");
        assert!(press(&mut preview, 'v'));
        assert_eq!(preview.visual_selection(), Some(((0, 1), (0, 1))));
    }

    #[test]
    fn jumping_to_a_comment_leaves_visual_mode() {
        let mut preview = CodePreview::new("one\ntwo\nthree\n", "txt");
        preview.set_focus(true);
        preview.set_threads(vec![thread("t", 2, 2, &["hi"])]);
        render_to_buffer(&preview, 30, 8);
        assert!(press(&mut preview, 'j'));
        assert!(press(&mut preview, 'v'));
        assert!(press(&mut preview, 'n'));
        assert_eq!(preview.focused_thread_id(), Some("t"));
        assert_eq!(preview.visual_selection(), None);
    }

    #[test]
    fn block_cursor_sits_in_terminal_cells() {
        let mut preview = CodePreview::new("界x\n", "txt");
        preview.set_focus(true);
        render_to_buffer(&preview, 20, 2);
        assert!(press(&mut preview, 'j'));
        assert!(press(&mut preview, 'v'));
        assert!(press(&mut preview, 'l'));
        assert_eq!(preview.cursor_col, 1);
        let buf = render_to_buffer(&preview, 20, 2);
        let gutter = preview.gutter_width();
        assert!(cell_reversed(&buf, gutter + 2, 0), "`x` is in the third cell");
        assert!(!cell_reversed(&buf, gutter + 1, 0));
    }

    fn thread(id: &str, start: usize, end: usize, bodies: &[&str]) -> InlineThread {
        InlineThread {
            root_id: id.into(),
            anchor_start: start,
            anchor_end: end,
            start_col: 0,
            end_col: 5,
            entries: bodies
                .iter()
                .map(|b| crate::component::InlineComment {
                    author: "claude".into(),
                    when: "2026-11-12 10:00".into(),
                    body: b.to_string(),
                })
                .collect(),
        }
    }

    fn numbered_code(lines: usize) -> String {
        (0..lines).map(|i| format!("line{i}\n")).collect()
    }

    #[test]
    fn inline_thread_renders_under_its_anchored_line() {
        let mut preview = CodePreview::new(numbered_code(6), "txt");
        render_to_buffer(&preview, 60, 12);
        preview.set_threads(vec![thread("t1", 1, 1, &["lex runs on every call", "agreed"])]);
        let buf = render_to_buffer(&preview, 60, 12);

        assert!(row_text(&buf, 1).contains("line1"));
        assert!(row_text(&buf, 2).contains("┌ claude"), "{}", row_text(&buf, 2));
        assert!(row_text(&buf, 3).contains("lex runs on every call"));
        assert!(row_text(&buf, 4).contains("↳ claude · 2026-11-12 10:00"), "reply has a date");
        assert!(row_text(&buf, 5).contains("  agreed"), "reply body sits under its header");
        assert!(row_text(&buf, 6).contains("└"));
        assert!(row_text(&buf, 7).contains("line2"), "code resumes after the bubble");
    }

    #[test]
    fn z_folds_the_thread_under_the_cursor_and_keeps_its_line_in_place() {
        let mut preview = CodePreview::new(numbered_code(30), "txt");
        preview.set_focus(true);
        render_to_buffer(&preview, 60, 10);
        preview.set_threads(vec![thread("t1", 3, 4, &["one", "two"])]);
        preview.jump_to_line(4);
        render_to_buffer(&preview, 60, 10);
        let rows_before = preview.total_rows();
        let screen_row = preview.row_of(4) - preview.scroll_offset.get() as usize;

        assert!(preview.handle_event(&key(ratatui::crossterm::event::KeyCode::Char('z'))));
        let buf = render_to_buffer(&preview, 60, 10);
        assert_eq!(preview.total_rows(), 30, "folded bubbles take no rows (was {rows_before})");
        assert_eq!(preview.row_of(4) - preview.scroll_offset.get() as usize, screen_row);
        let line4 = (0..10).map(|y| row_text(&buf, y)).find(|r| r.contains("line4")).unwrap();
        assert!(line4.contains("5 ▸ line4"), "folded sign sits beside the number: {line4}");

        // Off the thread, `z` falls through.
        preview.jump_to_line(10);
        assert!(!preview.handle_event(&key(ratatui::crossterm::event::KeyCode::Char('z'))));
    }

    #[test]
    fn focused_thread_highlights_just_its_node_and_shows_key_hints() {
        let mut preview = CodePreview::new("fn a() {\n    call(x);\n}\n".to_string(), "txt");
        preview.set_focus(true);
        render_to_buffer(&preview, 60, 14);
        // Anchored to `call(x)`: line 1, chars 4..11.
        let mut t = thread("t1", 1, 1, &["why call here?"]);
        (t.start_col, t.end_col) = (4, 11);
        preview.set_threads(vec![t]);
        preview.jump_to_line(1);
        assert_eq!(preview.focused_thread_id(), Some("t1"), "cursor on the node's line");
        let buf = render_to_buffer(&preview, 60, 14);

        let gutter = preview.gutter_width();
        let bg = |x: u16| buf[(x, 1)].bg;
        assert_eq!(bg(gutter + 4), crate::theme::COMMENT_ANCHOR_BG, "node start");
        assert_eq!(bg(gutter + 10), crate::theme::COMMENT_ANCHOR_BG, "node end");
        assert_eq!(bg(gutter + 3), crate::theme::SELECTION_BG, "indent stays the cursor line");
        assert_eq!(bg(gutter + 11), crate::theme::SELECTION_BG, "`;` is not part of the node");
        assert!((0..14).any(|y| row_text(&buf, y).contains("r reply · z fold · n next")));

        // Off the node's lines, nothing is highlighted.
        preview.jump_to_line(2);
        assert_eq!(preview.focused_thread_id(), None);
        let buf = render_to_buffer(&preview, 60, 14);
        assert_ne!(buf[(gutter + 4, 1)].bg, crate::theme::COMMENT_ANCHOR_BG);
    }

    #[test]
    fn scrolling_and_scrollbar_count_bubble_rows() {
        let mut preview = CodePreview::new(numbered_code(20), "txt");
        render_to_buffer(&preview, 60, 10);
        preview.set_threads(vec![thread("t1", 19, 19, &["at the very end"])]);
        render_to_buffer(&preview, 60, 10);
        // Scroll to the bottom: the bubble under the last line must be reachable.
        preview.scroll_by(1000);
        let buf = render_to_buffer(&preview, 60, 10);
        assert_eq!(preview.scroll_offset.get() as usize, preview.total_rows() - 10);
        assert!(row_text(&buf, 9).contains("└"), "last row is the bubble's bottom");
    }

    #[test]
    fn new_code_drops_threads() {
        let mut preview = CodePreview::new(numbered_code(4), "txt");
        preview.set_threads(vec![thread("t1", 0, 0, &["x"])]);
        preview.set_code(numbered_code(4));
        assert!(preview.threads().is_empty());
        assert_eq!(preview.total_rows(), 4);
    }

    #[test]
    fn expanded_threads_show_a_sign_beside_the_line_number() {
        let mut preview = CodePreview::new(numbered_code(4), "txt");
        render_to_buffer(&preview, 60, 12);
        preview.set_threads(vec![thread("t1", 1, 1, &["x"])]);
        let buf = render_to_buffer(&preview, 60, 12);
        assert!(row_text(&buf, 1).contains("2 ▾ line1"), "{}", row_text(&buf, 1));
        assert!(row_text(&buf, 0).contains("1   line0"), "no sign without a thread");
    }

    #[test]
    fn bubbles_are_closed_boxes_clear_of_the_scrollbar() {
        let mut preview = CodePreview::new(numbered_code(40), "txt");
        render_to_buffer(&preview, 50, 10);
        preview.set_threads(vec![thread("t1", 0, 0, &["body", "reply"])]);
        let buf = render_to_buffer(&preview, 50, 10);
        // Rows 1..=5: top, body, reply head, reply body, bottom. The
        // scrollbar owns column 49.
        assert_eq!(cell_symbol(&buf, 48, 1), "┐", "{}", row_text(&buf, 1));
        for y in 2..=4 {
            let row = row_text(&buf, y);
            assert_eq!(row.matches('│').count(), 2, "left and right border: {row}");
        }
        assert!(row_text(&buf, 5).contains('┘'));
        for y in 1..=5 {
            let edge = cell_symbol(&buf, 48, y);
            assert!(
                edge == "┐" || edge == "│" || edge == "┘",
                "box ends left of the scrollbar: {edge:?}"
            );
        }
    }

    #[test]
    fn cursor_stops_on_every_bubble_under_a_line() {
        let mut preview = CodePreview::new(numbered_code(6), "txt");
        preview.set_focus(true);
        render_to_buffer(&preview, 60, 30);
        preview
            .set_threads(vec![thread("first", 2, 2, &["one"]), thread("second", 2, 2, &["two"])]);
        preview.jump_to_line(2);
        assert_eq!(preview.focused_thread_id(), Some("first"), "the line focuses its first thread");

        let down = key(ratatui::crossterm::event::KeyCode::Char('j'));
        let up = key(ratatui::crossterm::event::KeyCode::Char('k'));
        preview.handle_event(&down);
        assert_eq!(preview.focused_thread_id(), Some("first"));
        preview.handle_event(&down);
        assert_eq!(preview.focused_thread_id(), Some("second"), "the second bubble is reachable");
        preview.handle_event(&down);
        assert_eq!(preview.cursor_position(), Some((3, None)));
        preview.handle_event(&up);
        assert_eq!(preview.focused_thread_id(), Some("second"), "k re-enters the last bubble");
        assert_eq!(preview.cursor_position(), Some((2, None)));

        // The bubble under the cursor shows focus through its border, not a
        // background: no row of it is painted as selected.
        let buf = render_to_buffer(&preview, 60, 30);
        let two = (0..30).find(|&y| row_text(&buf, y).contains("two")).unwrap();
        let border_x = row_text(&buf, two)
            .find('│')
            .map(|b| row_text(&buf, two)[..b].chars().count())
            .unwrap() as u16;
        assert_eq!(buf[(border_x, two)].fg, crate::theme::palette().marker);
        assert_ne!(row_bg(&buf, two), crate::theme::SELECTION_BG);
        let one = (0..30).find(|&y| row_text(&buf, y).contains("one")).unwrap();
        assert_eq!(
            buf[(border_x, one)].fg,
            crate::theme::palette().gray,
            "other bubbles stay quiet"
        );
    }

    #[test]
    fn z_folds_the_bubble_under_the_cursor_or_every_thread_on_the_line() {
        let mut preview = CodePreview::new(numbered_code(6), "txt");
        preview.set_focus(true);
        render_to_buffer(&preview, 60, 30);
        preview.set_threads(vec![thread("a", 2, 2, &["one"]), thread("b", 2, 2, &["two"])]);
        preview.jump_to_line(2);
        let z = key(ratatui::crossterm::event::KeyCode::Char('z'));
        let down = key(ratatui::crossterm::event::KeyCode::Char('j'));

        // On the second bubble, `z` folds just that one and returns to the line.
        preview.handle_event(&down);
        preview.handle_event(&down);
        assert!(preview.handle_event(&z));
        assert_eq!(preview.inline.folded.iter().collect::<Vec<_>>(), vec!["b"]);
        assert_eq!(preview.cursor_position(), Some((2, None)));
        assert_eq!(preview.focused_thread_id(), Some("a"));

        // On the line, `z` folds everything under it, then unfolds it all.
        assert!(preview.handle_event(&z));
        assert_eq!(preview.inline.folded.len(), 2);
        assert_eq!(preview.total_rows(), 6);
        assert!(preview.handle_event(&z));
        assert!(preview.inline.folded.is_empty());
    }

    #[test]
    fn clicking_a_bubble_focuses_it_and_clicking_code_moves_the_cursor() {
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let click = |row: u16| {
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 20,
                row,
                modifiers: KeyModifiers::NONE,
            })
        };
        let mut preview = CodePreview::new(numbered_code(6), "txt");
        preview.set_focus(true);
        render_to_buffer(&preview, 60, 20);
        preview.set_threads(vec![thread("a", 1, 1, &["one"]), thread("b", 1, 1, &["two"])]);
        let buf = render_to_buffer(&preview, 60, 20);

        let two = (0..20).find(|&y| row_text(&buf, y).contains("two")).unwrap();
        assert!(preview.handle_event(&click(two)));
        assert_eq!(preview.focused_thread_id(), Some("b"));
        assert_eq!(preview.cursor_position(), Some((1, None)));

        let line4 = (0..20).find(|&y| row_text(&buf, y).contains("line4")).unwrap();
        assert!(preview.handle_event(&click(line4)));
        assert_eq!(preview.cursor_position(), Some((4, None)));
        assert_eq!(preview.focused_thread_id(), None);
    }

    #[test]
    fn n_and_shift_n_cycle_through_threads_and_unfold_their_target() {
        let mut preview = CodePreview::new(numbered_code(20), "txt");
        preview.set_focus(true);
        render_to_buffer(&preview, 60, 10);
        preview.set_threads(vec![
            thread("a", 2, 2, &["one"]),
            thread("b", 9, 9, &["two"]),
            thread("c", 15, 15, &["three"]),
        ]);
        preview.jump_to_line(5);
        let n = key(ratatui::crossterm::event::KeyCode::Char('n'));
        let shift_n = key(ratatui::crossterm::event::KeyCode::Char('N'));

        assert!(preview.handle_event(&n));
        assert_eq!(preview.focused_thread_id(), Some("b"), "next thread below the cursor");
        assert!(preview.handle_event(&n));
        assert_eq!(preview.focused_thread_id(), Some("c"));
        assert!(preview.handle_event(&n));
        assert_eq!(preview.focused_thread_id(), Some("a"), "wraps around");
        assert!(preview.handle_event(&shift_n));
        assert_eq!(preview.focused_thread_id(), Some("c"));

        // A folded thread is opened when n lands on it, and scrolled into view.
        preview.inline.folded.insert("a".into());
        preview.invalidate_layout();
        assert!(preview.handle_event(&n));
        assert_eq!(preview.focused_thread_id(), Some("a"));
        assert!(preview.inline.folded.is_empty());
        let (first, _) = preview.cursor_rows().unwrap();
        assert!(
            first >= preview.scroll_offset.get() as usize
                && first < preview.scroll_offset.get() as usize + 10
        );
    }

    #[test]
    fn shift_z_folds_everything_then_unfolds_everything() {
        let mut preview = CodePreview::new(numbered_code(8), "txt");
        preview.set_focus(true);
        render_to_buffer(&preview, 60, 30);
        preview.set_threads(vec![thread("a", 1, 1, &["one"]), thread("b", 4, 4, &["two"])]);
        let shift_z = key(ratatui::crossterm::event::KeyCode::Char('Z'));

        assert!(preview.handle_event(&shift_z));
        assert_eq!(preview.total_rows(), 8, "all bubbles folded");
        assert!(preview.handle_event(&shift_z));
        assert!(preview.inline.folded.is_empty());

        // A partially folded preview folds the rest first.
        preview.inline.folded.insert("a".into());
        preview.invalidate_layout();
        assert!(preview.handle_event(&shift_z));
        assert_eq!(preview.inline.folded.len(), 2);
    }

    /// Text of the rendered preview plus a mask of what is highlighted:
    /// `^` the focused comment's node, `=` the cursor line, `*` a focused
    /// (marker-coloured) bubble border. Catches layout and focus regressions
    /// that a text-only snapshot would miss.
    fn snapshot(preview: &CodePreview, width: u16, height: u16) -> String {
        let buf = render_to_buffer(preview, width, height);
        let marker = crate::theme::palette().marker;
        let mut out = String::new();
        for y in 0..height {
            out.push_str(row_text(&buf, y).trim_end());
            out.push('\n');
            let mask: String = (0..width)
                .map(|x| {
                    let cell = &buf[(x, y)];
                    if cell.bg == crate::theme::COMMENT_ANCHOR_BG {
                        '^'
                    } else if cell.bg == crate::theme::SELECTION_BG {
                        '='
                    } else if cell.fg == marker && "┌┐└┘│─".contains(cell.symbol()) {
                        '*'
                    } else {
                        ' '
                    }
                })
                .collect();
            if !mask.trim().is_empty() {
                out.push_str(&format!("{}\n", mask.trim_end()));
            }
        }
        out
    }

    fn snapshot_preview() -> CodePreview {
        let code = "fn parse(input: &str) -> usize {\n    let tokens = lex(input);\n    match tokens {\n        0 => helper(1),\n        _ => helper(2),\n    }\n}\n";
        let mut preview = CodePreview::new(code.to_string(), "txt");
        preview.set_focus(true);
        render_to_buffer(&preview, 64, 24);
        let mut lex = thread("lex", 1, 1, &["lex runs on every call", "agreed, cache it"]);
        (lex.start_col, lex.end_col) = (4, 28);
        let mut arm = thread(
            "arm",
            4,
            4,
            &["the fallback arm hides unknown tokens; log them before defaulting"],
        );
        (arm.start_col, arm.end_col) = (8, 22);
        let mut second = thread("second", 4, 4, &["second thread on the same arm"]);
        (second.start_col, second.end_col) = (8, 22);
        preview.set_threads(vec![lex, arm, second]);
        preview
    }

    #[test]
    fn snapshot_threads_expanded() {
        let preview = snapshot_preview();
        insta::assert_snapshot!(snapshot(&preview, 64, 24));
    }

    #[test]
    fn snapshot_cursor_on_a_node_line() {
        let mut preview = snapshot_preview();
        preview.jump_to_line(1);
        insta::assert_snapshot!(snapshot(&preview, 64, 24));
    }

    #[test]
    fn snapshot_second_bubble_focused() {
        let mut preview = snapshot_preview();
        preview.jump_to_line(4);
        for _ in 0..2 {
            preview.handle_event(&key(ratatui::crossterm::event::KeyCode::Char('j')));
        }
        insta::assert_snapshot!(snapshot(&preview, 64, 24));
    }

    #[test]
    fn snapshot_line_folded_and_all_folded() {
        let mut preview = snapshot_preview();
        preview.jump_to_line(4);
        preview.handle_event(&key(ratatui::crossterm::event::KeyCode::Char('z')));
        let line_folded = snapshot(&preview, 64, 24);
        preview.handle_event(&key(ratatui::crossterm::event::KeyCode::Char('Z')));
        let all_folded = snapshot(&preview, 64, 24);
        insta::assert_snapshot!(format!("{line_folded}--- Z ---\n{all_folded}"));
    }

    #[test]
    fn narrowing_the_pane_keeps_the_cursor_line_on_screen() {
        let mut preview = CodePreview::new(numbered_code(30), "txt");
        preview.set_focus(true);
        render_to_buffer(&preview, 80, 12);
        let long = "a long comment body that wraps onto many more rows once the pane gets narrow";
        preview.set_threads(vec![thread("t1", 2, 2, &[long, long])]);
        preview.jump_to_line(9);
        render_to_buffer(&preview, 80, 12);
        let screen_row = preview.row_of(9) - preview.scroll_offset.get() as usize;

        // Narrower: the bubble above line 9 grows, but line 9 stays put.
        let buf = render_to_buffer(&preview, 40, 12);
        assert_eq!(preview.row_of(9) - preview.scroll_offset.get() as usize, screen_row);
        assert!(
            row_text(&buf, screen_row as u16).contains("line9"),
            "{}",
            row_text(&buf, screen_row as u16)
        );
    }

    #[test]
    fn node_highlight_counts_wide_characters_as_two_cells() {
        // `界` is one char but two terminal cells, so `call()` starts at cell 8.
        let mut preview = CodePreview::new("x\n界 = 1; call();\n".to_string(), "txt");
        preview.set_focus(true);
        render_to_buffer(&preview, 60, 10);
        let mut t = thread("t1", 1, 1, &["c"]);
        // Char columns of `call()`: after "界 = 1; " (7 chars).
        (t.start_col, t.end_col) = (7, 13);
        preview.set_threads(vec![t]);
        preview.jump_to_line(1);
        let buf = render_to_buffer(&preview, 60, 10);
        let gutter = preview.gutter_width();
        let node: String = (0..60)
            .filter(|&x| buf[(x, 1)].bg == crate::theme::COMMENT_ANCHOR_BG)
            .map(|x| buf[(x, 1)].symbol().to_string())
            .collect();
        assert_eq!(node, "call()");
        assert_eq!(buf[(gutter + 8, 1)].symbol(), "c", "highlight starts on the `c`");
    }
}
