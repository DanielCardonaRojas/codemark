//! A component for rendering simple Markdown-like text for bookmark info.

use crate::component::Component;
use crate::event::Event;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use unicode_width::UnicodeWidthChar;

/// A single styled run of text within a parsed markdown line.
#[derive(Debug, Clone)]
struct MdSpan {
    content: String,
    style: Style,
    /// Target URL if this run is a clickable link.
    link: Option<String>,
}

/// A parsed (but not yet wrapped) logical line of markdown.
#[derive(Debug, Clone, Default)]
struct MdLine {
    spans: Vec<MdSpan>,
}

/// One character placed during wrapping, carrying enough info to both render
/// and hit-test it.
#[derive(Debug, Clone)]
struct WrapCell {
    ch: char,
    width: usize,
    style: Style,
    link: Option<String>,
}

/// The on-screen rectangle occupied by a clickable link, in absolute terminal
/// coordinates, recorded during render so clicks can be matched back to a URL.
#[derive(Debug, Clone)]
struct LinkRegion {
    rect: Rect,
    url: String,
}

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
    /// Cached parsed lines to avoid re-parsing on every frame
    cached_lines: std::cell::RefCell<Vec<MdLine>>,
    /// Cached content hash to detect when content changes
    cached_content_hash: std::cell::Cell<u64>,
    /// Screen rectangles of clickable links from the most recent render
    link_regions: std::cell::RefCell<Vec<LinkRegion>>,
    /// URL queued for opening after a link was clicked
    pending_open_url: Option<String>,
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

    /// Take the URL of a link that was clicked, if any.
    ///
    /// Mirrors the `needs_preview_update` polling pattern: the panel records the
    /// click during event handling and the owner drains it afterwards to trigger
    /// an external "open URL" command.
    pub fn take_pending_open_url(&mut self) -> Option<String> {
        self.pending_open_url.take()
    }

    /// Convert the simple markdown string into parsed lines.
    fn parse_to_lines(&self) -> Vec<MdLine> {
        let mut lines = Vec::new();

        for line in self.content.lines() {
            if let Some(stripped) = line.strip_prefix("# ") {
                // H1
                lines.push(MdLine {
                    spans: vec![MdSpan {
                        content: stripped.to_string(),
                        style: Style::default()
                            .fg(crate::theme::palette().warning)
                            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                        link: None,
                    }],
                });
                lines.push(MdLine::default());
            } else if let Some(stripped) = line.strip_prefix("## ") {
                // H2
                lines.push(MdLine {
                    spans: vec![MdSpan {
                        content: stripped.to_string(),
                        style: Style::default()
                            .fg(crate::theme::palette().accent)
                            .add_modifier(Modifier::BOLD),
                        link: None,
                    }],
                });
            } else if let Some(stripped) = line.strip_prefix("> ") {
                // Blockquote
                let mut spans = vec![MdSpan {
                    content: "┃ ".to_string(),
                    style: Style::default().fg(crate::theme::palette().dim),
                    link: None,
                }];
                spans.extend(self.parse_inline(
                    stripped,
                    Style::default()
                        .fg(crate::theme::palette().gray)
                        .add_modifier(Modifier::ITALIC),
                ));
                lines.push(MdLine { spans });
            } else if let Some(stripped) = line.strip_prefix("- ") {
                // List item
                let mut spans = vec![MdSpan {
                    content: "• ".to_string(),
                    style: Style::default().fg(crate::theme::palette().accent),
                    link: None,
                }];
                spans.extend(self.parse_inline(stripped, Style::default()));
                lines.push(MdLine { spans });
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
                        spans.push(MdSpan {
                            content: format!("{:<15}", clean_key),
                            style: Style::default().fg(crate::theme::palette().dim),
                            link: None,
                        });
                    } else {
                        // Value
                        spans.extend(self.parse_inline(text, Style::default()));
                        if i < parts.len() - 1 {
                            spans.push(MdSpan {
                                content: " ".to_string(),
                                style: Style::default(),
                                link: None,
                            });
                        }
                    }
                }
                lines.push(MdLine { spans });
            } else {
                // Normal text
                lines.push(MdLine { spans: self.parse_inline(line, Style::default()) });
            }
        }

        lines
    }

    /// Refresh the cached parsed lines if content has changed.
    fn refresh_cache(&self) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // Calculate hash of current content
        let mut hasher = DefaultHasher::new();
        self.content.hash(&mut hasher);
        let current_hash = hasher.finish();

        // If content changed, re-parse and cache
        if self.cached_content_hash.get() != current_hash {
            let lines = self.parse_to_lines();
            *self.cached_lines.borrow_mut() = lines;
            self.cached_content_hash.set(current_hash);
        }
    }

    /// Wrap the parsed lines to the given width, returning rows of placed cells.
    ///
    /// Rendering and link hit-testing both derive from this single source of
    /// truth, so a clicked column always maps to exactly what was drawn.
    fn wrap_rows(&self, width: usize) -> Vec<Vec<WrapCell>> {
        self.refresh_cache();
        let lines = self.cached_lines.borrow();
        let mut rows: Vec<Vec<WrapCell>> = Vec::new();

        for line in lines.iter() {
            // Flatten the logical line into a sequence of placed cells.
            let mut cells: Vec<WrapCell> = Vec::new();
            for span in &line.spans {
                for ch in span.content.chars() {
                    cells.push(WrapCell {
                        ch,
                        width: ch.width().unwrap_or(0),
                        style: span.style,
                        link: span.link.clone(),
                    });
                }
            }

            if width == 0 || cells.is_empty() {
                rows.push(cells);
                continue;
            }

            // Greedy word wrap: break at the last space that fits, else hard-break.
            let mut cur: Vec<WrapCell> = Vec::new();
            let mut cur_width = 0usize;
            let mut last_space: Option<usize> = None;
            for cell in cells {
                if cur_width + cell.width > width && !cur.is_empty() {
                    match last_space {
                        Some(idx) => {
                            let carry = cur.split_off(idx + 1);
                            rows.push(std::mem::take(&mut cur));
                            cur_width = carry.iter().map(|c| c.width).sum();
                            cur = carry;
                        }
                        None => {
                            rows.push(std::mem::take(&mut cur));
                            cur_width = 0;
                        }
                    }
                    last_space = None;
                }
                if cell.ch == ' ' {
                    last_space = Some(cur.len());
                }
                cur_width += cell.width;
                cur.push(cell);
            }
            rows.push(cur);
        }

        rows
    }

    /// Build the rendered lines and link regions from wrapped rows.
    fn layout(&self, area: Rect) -> (Vec<Line<'static>>, Vec<LinkRegion>) {
        let rows = self.wrap_rows(area.width as usize);
        let scroll = self.scroll_offset as usize;
        let height = area.height as usize;

        let mut lines: Vec<Line<'static>> = Vec::with_capacity(rows.len());
        let mut regions: Vec<LinkRegion> = Vec::new();

        for (row_idx, row) in rows.iter().enumerate() {
            // Build a ratatui Line by coalescing consecutive cells of equal style.
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut buf = String::new();
            let mut buf_style = Style::default();
            for cell in row {
                if !buf.is_empty() && cell.style != buf_style {
                    spans.push(Span::styled(std::mem::take(&mut buf), buf_style));
                }
                if buf.is_empty() {
                    buf_style = cell.style;
                }
                buf.push(cell.ch);
            }
            if !buf.is_empty() {
                spans.push(Span::styled(buf, buf_style));
            }
            lines.push(Line::from(spans));

            // Record link regions for the visible portion of this row only.
            let visible = row_idx >= scroll && row_idx < scroll + height;
            if visible {
                let screen_y = area.y + (row_idx - scroll) as u16;
                let mut col = 0usize;
                let mut link_start: Option<(usize, &String)> = None;
                for cell in row {
                    match (&cell.link, link_start.as_ref()) {
                        (Some(url), None) => link_start = Some((col, url)),
                        (Some(url), Some((_, prev))) if *prev != url => {
                            // Adjacent but different links: flush the previous one.
                            if let Some((start, prev_url)) = link_start.take() {
                                push_region(&mut regions, area, screen_y, start, col, prev_url);
                            }
                            link_start = Some((col, url));
                        }
                        (None, Some(_)) => {
                            if let Some((start, url)) = link_start.take() {
                                push_region(&mut regions, area, screen_y, start, col, url);
                            }
                        }
                        _ => {}
                    }
                    col += cell.width;
                }
                if let Some((start, url)) = link_start.take() {
                    push_region(&mut regions, area, screen_y, start, col, url);
                }
            }
        }

        (lines, regions)
    }

    /// Get the rendered (wrapped) line count for the current viewport width.
    fn line_count(&self) -> usize {
        let width = self.last_area.get().width as usize;
        self.wrap_rows(width).len()
    }

    /// Parse inline formatting like `code`, **bold**, and [label](url) links.
    fn parse_inline(&self, text: &str, base_style: Style) -> Vec<MdSpan> {
        let mut spans = Vec::new();
        let mut current = text;

        while !current.is_empty() {
            // Find the nearest inline marker.
            let next_code = current.find('`');
            let next_bold = current.find("**");
            let next_link = current.find('[');

            let markers = [next_code, next_bold, next_link];
            let earliest = markers.iter().filter_map(|m| *m).min();

            let Some(pos) = earliest else {
                spans.push(MdSpan { content: current.to_string(), style: base_style, link: None });
                break;
            };

            // Emit any plain text before the marker.
            let plain = &current[..pos];

            if Some(pos) == next_link {
                if let Some((label, url, rest)) = parse_link(&current[pos..]) {
                    if !plain.is_empty() {
                        spans.push(MdSpan {
                            content: plain.to_string(),
                            style: base_style,
                            link: None,
                        });
                    }
                    spans.push(MdSpan {
                        content: label,
                        style: base_style
                            .fg(crate::theme::palette().info)
                            .add_modifier(Modifier::UNDERLINED),
                        link: Some(url),
                    });
                    current = rest;
                    continue;
                }
                // Not a valid link: treat '[' as literal text and continue.
                spans.push(MdSpan {
                    content: current[..pos + 1].to_string(),
                    style: base_style,
                    link: None,
                });
                current = &current[pos + 1..];
                continue;
            }

            if !plain.is_empty() {
                spans.push(MdSpan { content: plain.to_string(), style: base_style, link: None });
            }

            if Some(pos) == next_bold {
                // Bold: **...**
                let after = &current[pos + 2..];
                if let Some(end) = after.find("**") {
                    spans.push(MdSpan {
                        content: after[..end].to_string(),
                        style: base_style.add_modifier(Modifier::BOLD),
                        link: None,
                    });
                    current = &after[end + 2..];
                } else {
                    spans.push(MdSpan {
                        content: "**".to_string(),
                        style: base_style,
                        link: None,
                    });
                    current = after;
                }
            } else {
                // Inline code: `...`
                let after = &current[pos + 1..];
                if let Some(end) = after.find('`') {
                    spans.push(MdSpan {
                        content: after[..end].to_string(),
                        style: base_style.fg(crate::theme::palette().warning),
                        link: None,
                    });
                    current = &after[end + 1..];
                } else {
                    spans.push(MdSpan {
                        content: "`".to_string(),
                        style: base_style,
                        link: None,
                    });
                    current = after;
                }
            }
        }

        spans
    }
}

/// Record a link region spanning display columns `[start_col, end_col)` on the
/// given screen row, clipped to the rendered area.
fn push_region(
    regions: &mut Vec<LinkRegion>,
    area: Rect,
    screen_y: u16,
    start_col: usize,
    end_col: usize,
    url: &str,
) {
    if end_col <= start_col {
        return;
    }
    let max_width = area.width as usize;
    let start = start_col.min(max_width);
    let end = end_col.min(max_width);
    if end <= start {
        return;
    }
    regions.push(LinkRegion {
        rect: Rect {
            x: area.x + start as u16,
            y: screen_y,
            width: (end - start) as u16,
            height: 1,
        },
        url: url.to_string(),
    });
}

/// Parse a `[label](url)` link at the start of `text`.
///
/// Returns the label, URL, and the remaining text after the link. Only links
/// with a safe scheme (`http`, `https`, `mailto`) are accepted; everything else
/// is rejected so the caller renders it as literal text.
fn parse_link(text: &str) -> Option<(String, String, &str)> {
    let rest = text.strip_prefix('[')?;
    let close = rest.find(']')?;
    let label = &rest[..close];
    let after_label = &rest[close + 1..];
    let after_open = after_label.strip_prefix('(')?;
    let close_paren = after_open.find(')')?;
    let url = &after_open[..close_paren];
    let remainder = &after_open[close_paren + 1..];

    if label.is_empty() || url.is_empty() || !is_safe_url(url) {
        return None;
    }

    Some((label.to_string(), url.to_string(), remainder))
}

/// Whether a URL uses a scheme we are willing to hand to the OS opener.
fn is_safe_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
}

impl Component for MarkdownPanel {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.last_area.set(area);
        let (lines, regions) = self.layout(area);
        *self.link_regions.borrow_mut() = regions;

        // Lines are already wrapped to width, so vertical scrolling is enough.
        let paragraph = Paragraph::new(lines).scroll((self.scroll_offset, 0));
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
                let position = Position::from((mouse.column, mouse.row));
                let is_hovered = area.contains(position);

                match mouse.kind {
                    ratatui::crossterm::event::MouseEventKind::Down(
                        ratatui::crossterm::event::MouseButton::Left,
                    ) => {
                        for region in self.link_regions.borrow().iter() {
                            if region.rect.contains(position) {
                                self.pending_open_url = Some(region.url.clone());
                                return true;
                            }
                        }
                        false
                    }
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

    fn click(panel: &mut MarkdownPanel, col: u16, row: u16) -> bool {
        use ratatui::crossterm::event::{
            KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
        };
        panel.handle_event(&Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        }))
    }

    fn render(panel: &MarkdownPanel, width: u16, height: u16) {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        panel.render(area, &mut buf);
    }

    #[test]
    fn parses_valid_https_link() {
        let (label, url, rest) =
            parse_link("[docs](https://example.com) trailing").expect("link");
        assert_eq!(label, "docs");
        assert_eq!(url, "https://example.com");
        assert_eq!(rest, " trailing");
    }

    #[test]
    fn rejects_unsafe_scheme() {
        assert!(parse_link("[x](file:///etc/passwd)").is_none());
        assert!(parse_link("[x](javascript:alert(1))").is_none());
        assert!(parse_link("[x](./relative/path)").is_none());
    }

    #[test]
    fn clicking_link_queues_url() {
        let mut panel = MarkdownPanel::new();
        panel.set_markdown("See [docs](https://example.com) now");
        render(&panel, 80, 10);

        // "See " is 4 chars, link label "docs" occupies columns 4..8.
        assert!(click(&mut panel, 5, 0), "click on link should be handled");
        assert_eq!(panel.take_pending_open_url().as_deref(), Some("https://example.com"));
    }

    #[test]
    fn clicking_outside_link_does_nothing() {
        let mut panel = MarkdownPanel::new();
        panel.set_markdown("See [docs](https://example.com) now");
        render(&panel, 80, 10);

        // Column 0 is plain text ("S"), not the link.
        assert!(!click(&mut panel, 0, 0));
        assert_eq!(panel.take_pending_open_url(), None);
    }

    #[test]
    fn link_region_tracks_scroll_and_wrapping() {
        let mut panel = MarkdownPanel::new();
        // Several leading lines so the link wraps onto a scrolled-away row.
        let mut content = String::new();
        for i in 0..5 {
            content.push_str(&format!("line {i}\n"));
        }
        content.push_str("go to [site](https://example.com)");
        panel.set_markdown(content);

        render(&panel, 80, 20);
        // Link is on row 5 (after 5 plain lines); label at columns 6..10.
        assert!(click(&mut panel, 7, 5));
        assert_eq!(panel.take_pending_open_url().as_deref(), Some("https://example.com"));
    }

    #[test]
    fn unparseable_link_renders_as_text() {
        let mut panel = MarkdownPanel::new();
        panel.set_markdown("array[0] access");
        render(&panel, 80, 10);
        // No link region recorded; clicking does nothing.
        assert!(!click(&mut panel, 5, 0));
        assert_eq!(panel.take_pending_open_url(), None);
    }
}
