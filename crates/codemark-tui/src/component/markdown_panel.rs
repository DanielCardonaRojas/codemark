//! A component for rendering simple Markdown-like text for bookmark info.

use crate::component::Component;
use crate::event::Event;
use pulldown_cmark::{Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget, Wrap},
};

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
    /// Cached parsed text to avoid re-parsing on every frame
    cached_text: std::cell::RefCell<Text<'static>>,
    /// Cached content hash to detect when content changes
    cached_content_hash: std::cell::Cell<u64>,
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
    fn parse_to_text(&self) -> Text<'static> {
        let palette = crate::theme::palette();

        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut current_spans: Vec<Span<'static>> = Vec::new();
        // Text inherits the style at the top of this stack.
        let mut style_stack: Vec<Style> = vec![Style::default()];

        // Context flags/counters for prefixes and the table layout hack.
        let mut blockquote_depth: usize = 0;
        let mut in_table = false;
        let mut cell_index: usize = 0;

        // Push `current_spans` as a finished line (even when empty, so blank
        // lines and paragraph spacing survive), then start a fresh line. When
        // inside a blockquote, seed the new line with the `┃ ` prefix.
        macro_rules! flush_line {
            () => {{
                lines.push(Line::from(std::mem::take(&mut current_spans)));
                if blockquote_depth > 0 {
                    current_spans.push(Span::styled("┃ ", Style::default().fg(palette.dim)));
                }
            }};
        }

        let parser = Parser::new_ext(&self.content, Options::ENABLE_TABLES);

        for event in parser {
            match event {
                MdEvent::Start(tag) => match tag {
                    Tag::Heading { level, .. } => {
                        let style = match level {
                            HeadingLevel::H1 => Style::default()
                                .fg(palette.warning)
                                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                            _ => Style::default().fg(palette.accent).add_modifier(Modifier::BOLD),
                        };
                        style_stack.push(style);
                    }
                    Tag::BlockQuote(_) => {
                        blockquote_depth += 1;
                        // Blockquote body renders gray + italic, prefixed `┃ `.
                        style_stack
                            .push(Style::default().fg(palette.gray).add_modifier(Modifier::ITALIC));
                        current_spans.push(Span::styled("┃ ", Style::default().fg(palette.dim)));
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
                    Tag::Table(_) => {
                        in_table = true;
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
                        // H1 keeps the trailing blank line the old renderer added.
                        if matches!(tag, TagEnd::Heading(HeadingLevel::H1)) {
                            flush_line!();
                        }
                    }
                    TagEnd::Paragraph => {
                        flush_line!();
                        // Blank line between paragraphs (outside blockquotes/tables).
                        if blockquote_depth == 0 && !in_table {
                            flush_line!();
                        }
                    }
                    TagEnd::BlockQuote(_) => {
                        style_stack.pop();
                        blockquote_depth = blockquote_depth.saturating_sub(1);
                        flush_line!();
                    }
                    TagEnd::Item => {
                        flush_line!();
                    }
                    TagEnd::Strong | TagEnd::Emphasis => {
                        style_stack.pop();
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
                        style_stack.pop();
                        cell_index += 1;
                    }
                    _ => {}
                },
                MdEvent::Text(text) => {
                    let style = self.top_style(&style_stack);
                    if in_table && cell_index == 0 {
                        // Pad the "key" column to a fixed width, matching the
                        // previous `format!("{:<15}", key)` table layout. `text`
                        // is a `CowStr`; format it through `&str` so the width
                        // specifier is honored.
                        current_spans.push(Span::styled(format!("{:<15}", text.as_ref()), style));
                    } else {
                        current_spans.push(Span::styled(text.into_string(), style));
                    }
                }
                MdEvent::Code(text) => {
                    let style = self.top_style(&style_stack).fg(palette.warning);
                    current_spans.push(Span::styled(text.into_string(), style));
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
        while lines.last().is_some_and(|l| l.spans.iter().all(|s| s.content.is_empty())) {
            lines.pop();
        }

        Text::from(lines)
    }

    /// The style currently at the top of the stack (defaulting if somehow empty).
    fn top_style(&self, style_stack: &[Style]) -> Style {
        style_stack.last().copied().unwrap_or_default()
    }

    /// Refresh the cached text if content has changed.
    fn refresh_cache(&self) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // Calculate hash of current content
        let mut hasher = DefaultHasher::new();
        self.content.hash(&mut hasher);
        let current_hash = hasher.finish();

        // If content changed, re-parse and cache
        if self.cached_content_hash.get() != current_hash {
            let text = self.parse_to_text();
            *self.cached_text.borrow_mut() = text;
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
        let cached = self.cached_text.borrow().clone();
        let paragraph =
            Paragraph::new(cached).wrap(Wrap { trim: false }).scroll((self.scroll_offset, 0));

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

    /// Render `input` through the full markdown pipeline and collapse the
    /// result into the visible text it produces, one entry per line.
    fn rendered_lines(input: &str) -> Vec<String> {
        let mut panel = MarkdownPanel::new();
        panel.set_markdown(input);
        panel
            .parse_to_text()
            .lines
            .into_iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect()
    }

    /// Render `input` and join every line's visible text into one string.
    fn rendered_text(input: &str) -> String {
        rendered_lines(input).join("\n")
    }

    /// Find the first span whose content matches `needle`, returning its style.
    fn style_of(panel: &MarkdownPanel, needle: &str) -> Option<Style> {
        panel
            .parse_to_text()
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
}
