//! A component for rendering simple Markdown-like text for bookmark info.

use crate::component::Component;
use crate::event::Event;
use pulldown_cmark::{Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget, Wrap,
    },
};

/// The location and target of a rendered link, used to highlight the focused
/// link and open it. `line_idx` indexes [`MarkdownPanel::cached_text`]'s lines;
/// `span_range` is the half-open range of spans on that line that make up the
/// link's visible text.
#[derive(Debug, Clone)]
struct LinkSpan {
    /// Destination URL to open.
    url: String,
    /// Index of the line (in the parsed `Text`) the link sits on.
    line_idx: usize,
    /// Half-open `[start, end)` range of span indices forming the link text.
    span_range: std::ops::Range<usize>,
}

/// Open a URL in the system default browser. The default `open_link` action;
/// mirrors how `codemark-cli` opens auth URLs (errors are ignored — there is no
/// useful recovery in a TUI redraw).
fn open_in_browser(url: &str) {
    let _ = open::that(url);
}

/// A component that renders simple markdown-style text.
#[derive(Debug, Clone)]
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
    /// Links discovered in the content, in document order. Rebuilt whenever the
    /// content changes (alongside `cached_text`).
    links: std::cell::RefCell<Vec<LinkSpan>>,
    /// The currently highlighted link, if any. Driven by `n`/`N` navigation and
    /// opened with `Enter`.
    focused_link_index: Option<usize>,
    /// How to open a focused link's URL. A function pointer (so the panel stays
    /// `Clone`/`Debug`) defaulting to the system browser; tests swap in a probe
    /// to capture the dispatched URL without launching anything.
    open_link: fn(&str),
}

impl Default for MarkdownPanel {
    fn default() -> Self {
        Self {
            content: String::new(),
            scroll_offset: 0,
            focused: false,
            last_area: std::cell::Cell::default(),
            cached_text: std::cell::RefCell::default(),
            cached_content_hash: std::cell::Cell::default(),
            links: std::cell::RefCell::default(),
            focused_link_index: None,
            open_link: open_in_browser,
        }
    }
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
        self.focused_link_index = None;
        // Invalidate cache by setting hash to 0
        self.cached_content_hash.set(0);
    }

    /// Get the current markdown content.
    pub fn markdown(&self) -> &str {
        &self.content
    }

    /// Invalidate the cached hit-test area.
    ///
    /// Mouse handling treats an empty area as "not hovered", so clearing it when
    /// the panel isn't rendered keeps a stale area from capturing scroll input.
    pub fn invalidate_area(&self) {
        self.last_area.set(Rect::default());
    }

    /// Move the viewport by `delta` lines (positive scrolls down, negative up),
    /// clamped to the content bounds. Returns true if the offset changed.
    ///
    /// Unlike the focus-gated key handling, this is a plain viewport move that
    /// callers can drive regardless of focus — e.g. scrolling the preview with
    /// `J`/`K` while focus stays on another pane.
    pub fn scroll_by(&mut self, delta: i32) -> bool {
        let height = self.last_area.get().height as usize;
        let line_count = self.line_count();
        let max_offset = line_count.saturating_sub(height) as i32;
        let old_offset = self.scroll_offset;
        self.scroll_offset = (self.scroll_offset as i32 + delta).clamp(0, max_offset) as u16;
        old_offset != self.scroll_offset
    }

    /// Convert the markdown string into Ratatui `Text` by walking the
    /// `pulldown-cmark` event stream.
    ///
    /// `pulldown-cmark` handles CommonMark parsing (including backslash escapes
    /// like `\_` and `\*`, which the templates emit via `escape_markdown`) so
    /// we no longer scan for inline markers by hand. We translate the linear
    /// event stream into styled [`Line`]s, maintaining a [`Style`] stack so
    /// nested inline formatting (`**bold**`, `_italic_`, `` `code` ``) composes
    /// correctly, plus a little context to inject blockquote/list prefixes and
    /// emulate the prior table layout.
    fn parse_to_text(&self) -> (Text<'static>, Vec<LinkSpan>) {
        /// Visual prefix prepended to every line inside a blockquote.
        const QUOTE_PREFIX: &str = "┃ ";

        let palette = crate::theme::palette();

        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut current_spans: Vec<Span<'static>> = Vec::new();
        // Text inherits the style at the top of this stack.
        let mut style_stack: Vec<Style> = vec![Style::default()];

        // Links discovered so far, plus the in-progress link (if any). When a
        // `Tag::Link` opens we record its URL, the span index where its text
        // begins, and the line it begins on (`lines.len()`); when it closes we
        // capture the span range. Link text is single-line in practice, but a
        // soft break inside it would flush the line and invalidate the start span
        // index — the recorded start line lets us detect that and recover (see
        // `TagEnd::Link`) instead of silently dropping the link.
        let mut links: Vec<LinkSpan> = Vec::new();
        let mut pending_link: Option<(String, usize, usize)> = None;

        // Context flags/counters for prefixes and the table layout hack.
        let mut blockquote_depth: usize = 0;
        let mut in_table = false;
        let mut cell_index: usize = 0;
        // The first ("key") table column is padded to a fixed width. Inline
        // formatting splits a cell into several text/code events, so accumulate
        // the cell's visible text here and pad it exactly once on cell close —
        // padding each fragment would inflate the column far past the target.
        let mut key_buf = String::new();

        // Push `current_spans` as a finished line (even when empty, so blank
        // lines and paragraph spacing survive), then start a fresh line. When
        // inside a blockquote, seed the new line with the `┃ ` prefix.
        macro_rules! flush_line {
            () => {{
                lines.push(Line::from(std::mem::take(&mut current_spans)));
                if blockquote_depth > 0 {
                    current_spans
                        .push(Span::styled(QUOTE_PREFIX, Style::default().fg(palette.dim)));
                }
            }};
        }

        // Ensure a blank separator line precedes the next top-level block.
        // `pulldown-cmark` collapses the blank lines between blocks into block
        // structure, so we re-synthesize one space between adjacent blocks
        // (table → heading, paragraph → heading, etc.) — but never at the very
        // top and never two in a row. Nested blocks (list items, blockquote
        // bodies) opt out so their lines stay tight.
        macro_rules! ensure_blank {
            () => {{
                let has_pending = !current_spans.iter().all(|s| s.content.is_empty());
                let last_blank =
                    lines.last().is_none_or(|l| l.spans.iter().all(|s| s.content.is_empty()));
                if !has_pending && !lines.is_empty() && !last_blank {
                    lines.push(Line::from(""));
                }
            }};
        }

        let parser = Parser::new_ext(&self.content, Options::ENABLE_TABLES);

        for event in parser {
            match event {
                MdEvent::Start(tag) => match tag {
                    Tag::Heading { level, .. } => {
                        // Headings always stand apart from the preceding block.
                        ensure_blank!();
                        let style = match level {
                            HeadingLevel::H1 => Style::default()
                                .fg(palette.warning)
                                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                            _ => Style::default().fg(palette.accent).add_modifier(Modifier::BOLD),
                        };
                        style_stack.push(style);
                    }
                    Tag::Paragraph => {
                        ensure_blank!();
                    }
                    Tag::BlockQuote(_) => {
                        ensure_blank!();
                        blockquote_depth += 1;
                        // Blockquote body renders gray + italic, prefixed `┃ `.
                        style_stack
                            .push(Style::default().fg(palette.gray).add_modifier(Modifier::ITALIC));
                        current_spans
                            .push(Span::styled(QUOTE_PREFIX, Style::default().fg(palette.dim)));
                    }
                    Tag::Table(_) => {
                        ensure_blank!();
                        in_table = true;
                    }
                    Tag::List(_) => {
                        // Separate the list from the preceding block, but keep
                        // items themselves tight (no blank between them).
                        ensure_blank!();
                    }
                    Tag::Item => {
                        current_spans.push(Span::styled("• ", Style::default().fg(palette.accent)));
                    }
                    Tag::Strong => {
                        let style = self.top_style(&style_stack).add_modifier(Modifier::BOLD);
                        style_stack.push(style);
                    }
                    Tag::Emphasis => {
                        let style = self.top_style(&style_stack).add_modifier(Modifier::ITALIC);
                        style_stack.push(style);
                    }
                    Tag::Link { dest_url, .. } => {
                        // Link text renders underlined in the informational color
                        // so it reads as a link; the spans it wraps inherit this
                        // via `top_style`. Remember where the text begins so the
                        // close can record the span range for highlighting/opening.
                        let style =
                            Style::default().fg(palette.info).add_modifier(Modifier::UNDERLINED);
                        style_stack.push(style);
                        pending_link =
                            Some((dest_url.into_string(), current_spans.len(), lines.len()));
                    }
                    Tag::TableHead => {
                        cell_index = 0;
                    }
                    Tag::TableRow => {
                        cell_index = 0;
                    }
                    Tag::TableCell => {
                        // First column ("key") renders dim, like the prior layout.
                        let style = if cell_index == 0 {
                            Style::default().fg(palette.dim)
                        } else {
                            self.top_style(&style_stack)
                        };
                        style_stack.push(style);
                    }
                    _ => {}
                },
                MdEvent::End(tag) => match tag {
                    TagEnd::Heading(_) => {
                        style_stack.pop();
                        flush_line!();
                        // H1 keeps a trailing blank line so the title stands out
                        // even when content follows immediately.
                        if matches!(tag, TagEnd::Heading(HeadingLevel::H1)) {
                            flush_line!();
                        }
                    }
                    TagEnd::Paragraph => {
                        flush_line!();
                    }
                    TagEnd::BlockQuote(_) => {
                        style_stack.pop();
                        blockquote_depth = blockquote_depth.saturating_sub(1);
                        // The inner paragraph's flush already emitted the quote's
                        // last line and speculatively re-seeded `current_spans`
                        // with a "┃ " prefix. If only that prefix (or nothing) is
                        // pending, drop it so the closed quote leaves no stray bar
                        // line; otherwise flush any genuine trailing content.
                        let only_prefix = current_spans
                            .iter()
                            .all(|s| s.content.is_empty() || s.content.as_ref() == QUOTE_PREFIX);
                        if only_prefix {
                            current_spans.clear();
                        } else {
                            flush_line!();
                        }
                    }
                    TagEnd::Item => {
                        flush_line!();
                    }
                    TagEnd::Strong | TagEnd::Emphasis => {
                        style_stack.pop();
                    }
                    TagEnd::Link => {
                        style_stack.pop();
                        // Record the link's text span range on the line it will
                        // occupy once flushed. If a soft break flushed the line
                        // mid-link (`start_line != lines.len()`), the recorded
                        // `start` indexes a now-emitted line, so anchor to the
                        // final line's spans (`start = 0`) rather than dropping the
                        // link. If the link emitted no spans (empty text), skip it.
                        if let Some((url, start, start_line)) = pending_link.take() {
                            let start = if start_line == lines.len() { start } else { 0 };
                            let end = current_spans.len();
                            if end > start {
                                links.push(LinkSpan {
                                    url,
                                    line_idx: lines.len(),
                                    span_range: start..end,
                                });
                            }
                        }
                    }
                    TagEnd::Table => {
                        in_table = false;
                    }
                    TagEnd::TableHead => {
                        flush_line!();
                    }
                    TagEnd::TableRow => {
                        flush_line!();
                    }
                    TagEnd::TableCell => {
                        if cell_index == 0 {
                            // Pad the fully-accumulated key once, so inline
                            // formatting in the cell can't inflate the width.
                            let style = self.top_style(&style_stack);
                            current_spans.push(Span::styled(format!("{:<15}", key_buf), style));
                            key_buf.clear();
                        }
                        style_stack.pop();
                        cell_index += 1;
                    }
                    _ => {}
                },
                MdEvent::Text(text) => {
                    if in_table && cell_index == 0 {
                        // Buffer the key column; padded once at TagEnd::TableCell.
                        key_buf.push_str(text.as_ref());
                    } else {
                        let style = self.top_style(&style_stack);
                        current_spans.push(Span::styled(text.into_string(), style));
                    }
                }
                MdEvent::Code(text) => {
                    if in_table && cell_index == 0 {
                        // Inline code in a key cell still counts toward its width.
                        key_buf.push_str(text.as_ref());
                    } else {
                        let style = self.top_style(&style_stack).fg(palette.warning);
                        current_spans.push(Span::styled(text.into_string(), style));
                    }
                }
                MdEvent::SoftBreak | MdEvent::HardBreak => {
                    flush_line!();
                }
                MdEvent::Rule => {
                    flush_line!();
                    lines.push(Line::from(Span::styled(
                        "─".repeat(20),
                        Style::default().fg(palette.dim),
                    )));
                }
                _ => {}
            }
        }

        // Emit any trailing spans that weren't terminated by a block end.
        if !current_spans.is_empty() {
            lines.push(Line::from(std::mem::take(&mut current_spans)));
        }

        // Drop trailing blank lines so block terminators (paragraphs, the H1
        // spacer) don't leave dangling empty rows at the end of the panel.
        // Links always live on non-blank lines, so trimming never invalidates a
        // recorded `line_idx`.
        while lines.last().is_some_and(|l| l.spans.iter().all(|s| s.content.is_empty())) {
            lines.pop();
        }

        (Text::from(lines), links)
    }

    /// The style currently at the top of the stack (defaulting if somehow empty).
    fn top_style(&self, style_stack: &[Style]) -> Style {
        style_stack.last().copied().unwrap_or_default()
    }

    /// Advance the focused link by `delta` (`+1` next, `-1` previous), wrapping
    /// around the link list. With no links this is a no-op. From no selection,
    /// `+1` focuses the first link and `-1` the last. Scrolls the newly focused
    /// link into view. Returns whether anything changed (so the caller can mark
    /// the frame dirty).
    fn focus_link(&mut self, delta: isize) -> bool {
        self.refresh_cache();
        let count = self.links.borrow().len();
        if count == 0 {
            return false;
        }
        let next = match self.focused_link_index {
            None if delta > 0 => 0,
            None => count - 1,
            Some(cur) => (cur as isize + delta).rem_euclid(count as isize) as usize,
        };
        if self.focused_link_index == Some(next) {
            return false;
        }
        self.focused_link_index = Some(next);
        self.scroll_focused_link_into_view();
        true
    }

    /// Open the currently focused link in the system browser. Returns whether a
    /// link was opened.
    fn open_focused_link(&mut self) -> bool {
        let url = self
            .focused_link_index
            .and_then(|idx| self.links.borrow().get(idx).map(|l| l.url.clone()));
        match url {
            Some(url) => {
                (self.open_link)(&url);
                true
            }
            None => false,
        }
    }

    /// Scroll so the focused link's line sits within the viewport. Uses the
    /// recorded `line_idx`; treated as an unwrapped row, which is exact for the
    /// short, single-line links the templates emit.
    fn scroll_focused_link_into_view(&mut self) {
        let height = self.last_area.get().height as usize;
        if height == 0 {
            return;
        }
        let Some(line_idx) = self
            .focused_link_index
            .and_then(|idx| self.links.borrow().get(idx).map(|l| l.line_idx))
        else {
            return;
        };
        let top = self.scroll_offset as usize;
        if line_idx < top {
            self.scroll_offset = line_idx as u16;
        } else if line_idx >= top + height {
            self.scroll_offset = (line_idx + 1 - height) as u16;
        }
    }

    /// Refresh the cached text if content has changed.
    fn refresh_cache(&self) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // Calculate hash of current content
        let mut hasher = DefaultHasher::new();
        self.content.hash(&mut hasher);
        let current_hash = hasher.finish();

        // If content changed, re-parse and cache. The cache holds the *base*
        // (un-highlighted) text and the link table; the focused-link highlight is
        // a cheap render-time post-pass keyed on `focused_link_index`, so it
        // doesn't force a re-parse on every `n`/`N` press.
        if self.cached_content_hash.get() != current_hash {
            let (text, links) = self.parse_to_text();
            *self.cached_text.borrow_mut() = text;
            *self.links.borrow_mut() = links;
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
}

impl Component for MarkdownPanel {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.last_area.set(area);
        self.refresh_cache();
        let mut text = self.cached_text.borrow().clone();
        // Highlight the focused link by reversing its spans. Done here (not in
        // the cache) so navigation doesn't re-parse; the link table indexes into
        // the same line/span layout `cached_text` holds.
        if let Some(idx) = self.focused_link_index
            && let Some(link) = self.links.borrow().get(idx)
            && let Some(line) = text.lines.get_mut(link.line_idx)
        {
            for span in line.spans.get_mut(link.span_range.clone()).into_iter().flatten() {
                span.style = span.style.add_modifier(Modifier::REVERSED);
            }
        }
        let paragraph =
            Paragraph::new(text).wrap(Wrap { trim: false }).scroll((self.scroll_offset, 0));

        paragraph.render(area, buf);

        // Render a scrollbar on the right edge when content overflows the
        // viewport, mirroring the code preview. `line_count()` already returns
        // the wrap-aware total, so the same overflow gating and thumb math apply.
        let height = area.height as usize;
        let total = self.line_count();
        if total > height && height > 0 {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(None)
                .thumb_symbol("┃");

            // Model `content_length` as the number of scroll positions
            // (`total - height + 1`) so Ratatui 0.29's thumb math yields the
            // correct visible/total ratio and lets the thumb reach the bottom on
            // the last page. See the code preview for the derivation.
            let scroll_positions = total - height + 1;
            let mut scrollbar_state =
                ScrollbarState::new(scroll_positions).position(self.scroll_offset as usize);

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
                    | ratatui::crossterm::event::KeyCode::Char('j') => self.scroll_by(1),
                    ratatui::crossterm::event::KeyCode::Up
                    | ratatui::crossterm::event::KeyCode::Char('k') => self.scroll_by(-1),
                    // Link navigation: `n` focuses the next link, `N` the previous
                    // one (wrapping). `n`/`N` reach the panel only when the right
                    // pane is focused. `Tab` (focus cycling) and `[`/`]` (tab
                    // switching in `TabbedPanel`) are both claimed upstream, so
                    // neither can drive link navigation here.
                    ratatui::crossterm::event::KeyCode::Char('n') => self.focus_link(1),
                    ratatui::crossterm::event::KeyCode::Char('N') => self.focus_link(-1),
                    // Open the focused link in the system browser.
                    ratatui::crossterm::event::KeyCode::Enter => self.open_focused_link(),
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
        // Clear the focused link when focus leaves so the REVERSED highlight,
        // which `render` applies unconditionally, doesn't linger on an unfocused
        // panel after the user tabs away.
        if !focused {
            self.focused_link_index = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render `input` through the full markdown pipeline and collapse the
    /// result into the visible text it produces, one entry per line.
    fn rendered_lines(input: &str) -> Vec<String> {
        let mut panel = MarkdownPanel::new();
        panel.set_markdown(input);
        panel
            .parse_to_text()
            .0
            .lines
            .into_iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect()
    }

    /// Render `input` and join every line's visible text into one string.
    fn rendered_text(input: &str) -> String {
        rendered_lines(input).join("\n")
    }

    /// The index of the line whose visible text equals `needle`.
    fn line_index(lines: &[String], needle: &str) -> usize {
        lines.iter().position(|l| l == needle).unwrap_or_else(|| panic!("missing {needle:?}"))
    }

    #[test]
    fn adjacent_blocks_are_separated_by_a_blank_line() {
        // pulldown-cmark collapses the blank lines between blocks into block
        // structure, so the renderer must re-synthesize a separator. A heading
        // following a table (as in `codemark_show.md`'s Metadata → Resolution
        // History) previously butted right up against the table's last row.
        let md = "## Metadata\n| Property | Value |\n|---|---|\n| **Commit** | abc |\n\n## Resolution History\n\nbody";
        let lines = rendered_lines(md);

        let commit = line_index(&lines, "Commit         abc");
        let history = line_index(&lines, "Resolution History");
        assert!(history > commit + 1, "expected blank line between table and heading: {lines:?}");
        assert!(lines[commit + 1].is_empty(), "line after table row should be blank: {lines:?}");

        // The heading is likewise separated from the paragraph that follows it.
        let body = line_index(&lines, "body");
        assert!(body > history + 1, "expected blank line between heading and body: {lines:?}");
    }

    #[test]
    fn consecutive_label_lines_stay_tight() {
        // Resolution History emits `**ID:** ...\n**Date:** ...` with single
        // newlines (soft breaks); those must remain adjacent, not blank-separated.
        let lines = rendered_lines("**ID:** 56c2fc13\n**Date:** 2026");
        let id = line_index(&lines, "ID: 56c2fc13");
        assert_eq!(lines.get(id + 1).map(String::as_str), Some("Date: 2026"), "{lines:?}");
    }

    #[test]
    fn no_leading_blank_line() {
        // The first block never gets a synthesized blank line above it.
        let lines = rendered_lines("# Title\n\nbody");
        assert_eq!(lines.first().map(String::as_str), Some("Title"));
    }

    /// Find the first span whose content matches `needle`, returning its style.
    fn style_of(panel: &MarkdownPanel, needle: &str) -> Option<Style> {
        panel
            .parse_to_text()
            .0
            .lines
            .into_iter()
            .flat_map(|line| line.spans.into_iter())
            .find(|s| s.content.contains(needle))
            .map(|s| s.style)
    }

    #[test]
    fn backslash_escapes_are_consumed() {
        // Underscores escaped by the templates (via `escape_markdown`) must not
        // show their backslash. pulldown-cmark unescapes them per CommonMark.
        assert_eq!(rendered_text("function upsert\\_repo"), "function upsert_repo");
        assert_eq!(rendered_text("resolve\\_server\\_and\\_token"), "resolve_server_and_token");
        // Other escaped punctuation is likewise unescaped.
        assert_eq!(rendered_text("a\\*b\\#c"), "a*b#c");
        // Escaped double quotes (e.g. from a query predicate's string literal)
        // are unescaped too. CommonMark unescapes any backslashed ASCII
        // punctuation, so `\"` collapses to a literal `"`.
        assert_eq!(rendered_text("match \\\"create_session\\\""), "match \"create_session\"");
    }

    #[test]
    fn escaped_formatting_chars_are_literal() {
        // An escaped backtick stays literal rather than opening a code span.
        assert_eq!(rendered_text("use \\`code\\` here"), "use `code` here");
        // A trailing backslash with nothing to escape is preserved.
        assert_eq!(rendered_text("ends with\\"), "ends with\\");
    }

    #[test]
    fn unescaped_formatting_still_applies() {
        // Code spans and bold/italic markers are stripped, leaving plain text.
        assert_eq!(rendered_text("see `registry.rs` now"), "see registry.rs now");
        assert_eq!(rendered_text("this is **bold** text"), "this is bold text");
        assert_eq!(rendered_text("this is _italic_ text"), "this is italic text");
    }

    #[test]
    fn inline_styles_carry_their_modifiers() {
        let mut panel = MarkdownPanel::new();
        panel.set_markdown("a **bold** and `code` and _em_ word");

        let bold = style_of(&panel, "bold").expect("bold span");
        assert!(bold.add_modifier.contains(Modifier::BOLD));

        let em = style_of(&panel, "em").expect("italic span");
        assert!(em.add_modifier.contains(Modifier::ITALIC));

        let code = style_of(&panel, "code").expect("code span");
        assert_eq!(code.fg, Some(crate::theme::palette().warning));
    }

    #[test]
    fn headings_get_their_styles_and_h1_spacing() {
        let lines = rendered_lines("# Title\n\nbody");
        // H1 keeps the trailing blank line the prior renderer emitted.
        assert_eq!(lines.first().map(String::as_str), Some("Title"));
        assert_eq!(lines.get(1).map(String::as_str), Some(""));

        let mut panel = MarkdownPanel::new();
        panel.set_markdown("# Title");
        let h1 = style_of(&panel, "Title").expect("h1 span");
        assert_eq!(h1.fg, Some(crate::theme::palette().warning));
        assert!(h1.add_modifier.contains(Modifier::BOLD | Modifier::UNDERLINED));

        panel.set_markdown("## Sub");
        let h2 = style_of(&panel, "Sub").expect("h2 span");
        assert_eq!(h2.fg, Some(crate::theme::palette().accent));
        assert!(h2.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn list_items_get_bullet_prefix() {
        let lines = rendered_lines("- one\n- two");
        assert!(lines.iter().any(|l| l == "• one"));
        assert!(lines.iter().any(|l| l == "• two"));
    }

    #[test]
    fn blockquotes_get_bar_prefix() {
        let lines = rendered_lines("> quoted text");
        assert!(lines.iter().any(|l| l.starts_with("┃ ") && l.contains("quoted text")));
        // The quote must not leave a stray prefix-only "┃ " line behind it: the
        // flush at `End(BlockQuote)` should not emit the seeded prefix as a line.
        assert!(!lines.iter().any(|l| l.trim_end() == "┃"), "stray blockquote bar line: {lines:?}");
    }

    #[test]
    fn multiline_blockquote_has_no_phantom_lines() {
        // Each quoted line keeps its prefix; no extra bar line trails the block.
        let lines = rendered_lines("> line one\n> line two\n\nafter");
        let bar_lines: Vec<&String> = lines.iter().filter(|l| l.starts_with("┃ ")).collect();
        assert_eq!(bar_lines.len(), 2, "expected exactly two quoted lines: {lines:?}");
        assert!(lines.iter().any(|l| l == "after"));
    }

    #[test]
    fn tag_line_and_thematic_break_render_sanely() {
        // The `## Tags` body in the templates renders ` #foo #bar ` — the
        // leading space means it is plain text, not an ATX heading.
        assert_eq!(rendered_lines(" #foo #bar "), vec!["#foo #bar".to_string()]);

        // The `---` separator between comments is a thematic break, drawn as a
        // dim horizontal rule rather than literal dashes.
        let lines = rendered_lines("first\n\n---\n\nsecond");
        assert!(lines.iter().any(|l| l.contains('─')), "expected rule, got {lines:?}");
        assert!(!lines.iter().any(|l| l == "---"), "raw dashes leaked: {lines:?}");
    }

    #[test]
    fn tables_pad_the_key_column_and_drop_separator() {
        let table = "| Property | Value |\n|----------|-------|\n| **File** | src/lib.rs |";
        let lines = rendered_lines(table);
        // Header row renders, padded to the fixed key width.
        assert!(lines.iter().any(|l| l.starts_with("Property") && l.contains("Value")));
        // The `---` separator row is consumed, not rendered.
        assert!(!lines.iter().any(|l| l.contains("---")));
        // The key column is left-padded to 15 columns.
        let file_row = lines.iter().find(|l| l.contains("src/lib.rs")).expect("value row");
        assert!(file_row.starts_with("File           "), "got: {file_row:?}");
    }

    #[test]
    fn table_key_with_inline_formatting_pads_once() {
        // A key cell split into several events by inline formatting must be
        // padded as a single 15-wide field, not per fragment.
        let table = "| Key **X** | val |\n|---|---|\n| Row | v |";
        let lines = rendered_lines(table);
        let header = lines.iter().find(|l| l.contains("val")).expect("header row");
        // The cell text ("Key X") is padded to a single 15-wide field, then the
        // value follows — not each fragment padded separately.
        assert!(header.starts_with("Key X          val"), "got: {header:?}");
        // The value sits at column 15, proving the key was padded exactly once.
        assert_eq!(header.find("val"), Some(15), "value not at column 15: {header:?}");
    }

    // ── Links ────────────────────────────────────────────────────────────

    use ratatui::crossterm::event::{KeyCode, KeyEvent};

    // Captures the last URL passed to `open_link`, so a test can assert which
    // link `Enter` dispatched without launching a browser. `fn` pointers can't
    // close over locals, so the probe writes through this thread-local.
    thread_local! {
        static OPENED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    fn record_open(url: &str) {
        OPENED.with(|o| o.borrow_mut().push(url.to_string()));
    }

    /// A focused panel whose `Enter` action records the URL instead of opening it.
    fn link_panel(input: &str) -> MarkdownPanel {
        OPENED.with(|o| o.borrow_mut().clear());
        let mut panel = MarkdownPanel::new();
        panel.open_link = record_open;
        panel.set_markdown(input);
        panel.set_focus(true);
        // Give the panel a viewport so scroll-into-view math has a height.
        panel.last_area.set(Rect::new(0, 0, 80, 24));
        panel
    }

    fn press(panel: &mut MarkdownPanel, code: KeyCode) -> bool {
        panel.handle_event(&Event::Key(KeyEvent::from(code)))
    }

    #[test]
    fn link_text_renders_styled_without_markup() {
        // `[text](url)` shows just `text`, underlined in the info color — no
        // brackets or parenthesized URL leak into the visible output.
        let panel = link_panel("see [the docs](https://example.com/docs) now");
        let lines: Vec<String> = panel
            .parse_to_text()
            .0
            .lines
            .into_iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect();
        assert_eq!(lines, vec!["see the docs now".to_string()]);

        let style = style_of(&panel, "the docs").expect("link span");
        assert_eq!(style.fg, Some(crate::theme::palette().info));
        assert!(style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn links_are_parsed_in_document_order() {
        // The link table captures each destination URL, in order.
        let panel = link_panel("[a](https://a.test) and [b](https://b.test)");
        panel.refresh_cache();
        let urls: Vec<String> = panel.links.borrow().iter().map(|l| l.url.clone()).collect();
        assert_eq!(urls, vec!["https://a.test".to_string(), "https://b.test".to_string()]);
    }

    #[test]
    fn n_and_shift_n_cycle_focused_link_with_wraparound() {
        // `n` advances through links and wraps; `N` steps back and wraps. The
        // focused link's spans pick up the REVERSED highlight on render.
        let mut panel = link_panel("[a](https://a.test) [b](https://b.test)");
        assert_eq!(panel.focused_link_index, None);

        assert!(press(&mut panel, KeyCode::Char('n')));
        assert_eq!(panel.focused_link_index, Some(0));
        assert!(press(&mut panel, KeyCode::Char('n')));
        assert_eq!(panel.focused_link_index, Some(1));
        // Wrap forward back to the first link.
        assert!(press(&mut panel, KeyCode::Char('n')));
        assert_eq!(panel.focused_link_index, Some(0));
        // `N` wraps backward to the last link.
        assert!(press(&mut panel, KeyCode::Char('N')));
        assert_eq!(panel.focused_link_index, Some(1));
    }

    #[test]
    fn navigation_keys_are_inert_without_links() {
        // With no links, `n`/`N` don't claim the key (return false) so they can
        // fall through to any other handler.
        let mut panel = link_panel("plain text, no links");
        assert!(!press(&mut panel, KeyCode::Char('n')));
        assert!(!press(&mut panel, KeyCode::Char('N')));
        assert_eq!(panel.focused_link_index, None);
    }

    #[test]
    fn focused_link_is_highlighted_on_render() {
        // The rendered (post-pass) text reverses exactly the focused link's spans.
        let mut panel = link_panel("[a](https://a.test) [b](https://b.test)");
        press(&mut panel, KeyCode::Char('n')); // focus link 0

        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 4));
        panel.render(Rect::new(0, 0, 80, 4), &mut buf);

        // The first cell of "a" should be reversed.
        let a_cell = buf.cell((0, 0)).expect("cell at link a");
        assert!(a_cell.modifier.contains(Modifier::REVERSED), "link a not highlighted");
    }

    #[test]
    fn enter_opens_the_focused_link() {
        let mut panel = link_panel("go to [home](https://home.test)");
        // Enter with no focus opens nothing.
        assert!(!press(&mut panel, KeyCode::Enter));
        OPENED.with(|o| assert!(o.borrow().is_empty()));

        press(&mut panel, KeyCode::Char('n')); // focus the link
        assert!(press(&mut panel, KeyCode::Enter));
        OPENED.with(|o| assert_eq!(o.borrow().as_slice(), ["https://home.test".to_string()]));
    }

    #[test]
    fn losing_focus_clears_the_highlight() {
        // The REVERSED highlight must not linger after the panel is unfocused:
        // `set_focus(false)` clears the focused link so render adds no highlight.
        let mut panel = link_panel("[a](https://a.test) [b](https://b.test)");
        press(&mut panel, KeyCode::Char('n')); // focus link 0
        assert_eq!(panel.focused_link_index, Some(0));

        panel.set_focus(false);
        assert_eq!(panel.focused_link_index, None, "focus loss must clear the focused link");

        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 4));
        panel.render(Rect::new(0, 0, 80, 4), &mut buf);
        let a_cell = buf.cell((0, 0)).expect("cell at link a");
        assert!(
            !a_cell.modifier.contains(Modifier::REVERSED),
            "highlight should not persist on an unfocused panel"
        );
    }

    #[test]
    fn link_text_split_across_a_soft_break_is_still_tracked() {
        // A soft break inside the link text flushes the line mid-link, so the
        // span-start index recorded at link open no longer indexes the final
        // line. The link must anchor to its last line rather than being dropped.
        let panel = link_panel("[multi\nline link](https://wrapped.test)");
        panel.refresh_cache();
        let links = panel.links.borrow();
        assert_eq!(links.len(), 1, "wrapped link was dropped: {links:?}");
        assert_eq!(links[0].url, "https://wrapped.test");
        // It anchors to the final line of the link text, from span 0.
        assert_eq!(links[0].span_range.start, 0);
    }

    // ── Scrollbar ────────────────────────────────────────────────────────

    /// Whether the right-edge column of `buf` contains the scrollbar thumb.
    fn has_scrollbar(buf: &Buffer, area: Rect) -> bool {
        let x = area.right() - 1;
        (area.top()..area.bottom()).any(|y| buf.cell((x, y)).is_some_and(|c| c.symbol() == "┃"))
    }

    #[test]
    fn scrollbar_appears_only_when_content_overflows() {
        // Content taller than the viewport draws a scrollbar thumb on the right
        // edge; content that fits does not.
        let many = (0..40).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n\n");
        let mut panel = MarkdownPanel::new();
        panel.set_markdown(many);
        let area = Rect::new(0, 0, 20, 6);
        let mut buf = Buffer::empty(area);
        panel.render(area, &mut buf);
        assert!(has_scrollbar(&buf, area), "overflowing content should show a scrollbar");

        let mut small = MarkdownPanel::new();
        small.set_markdown("just one line");
        let mut buf2 = Buffer::empty(area);
        small.render(area, &mut buf2);
        assert!(!has_scrollbar(&buf2, area), "content that fits should not show a scrollbar");
    }

    #[test]
    fn scroll_by_moves_the_viewport_and_clamps() {
        // `scroll_by` drives the viewport without requiring focus, so the preview
        // can be scrolled with `J`/`K` from another pane.
        let many = (0..40).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n\n");
        let mut panel = MarkdownPanel::new();
        panel.set_markdown(many);
        let area = Rect::new(0, 0, 20, 6);
        let mut buf = Buffer::empty(area);
        panel.render(area, &mut buf);

        assert!(!panel.focused, "scroll_by must work even while unfocused");
        assert!(panel.scroll_by(5));
        assert_eq!(panel.scroll_offset, 5);

        // Scrolling up clamps at the top and reports no movement past it.
        assert!(panel.scroll_by(-100));
        assert_eq!(panel.scroll_offset, 0);
        assert!(!panel.scroll_by(-1));

        // Scrolling down clamps at the last page (line_count - height).
        let max = panel.line_count().saturating_sub(area.height as usize) as u16;
        assert!(panel.scroll_by(10_000));
        assert_eq!(panel.scroll_offset, max);
        assert!(!panel.scroll_by(1));
    }
}
