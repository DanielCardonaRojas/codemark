//! Inline comment threads for [`CodePreview`](super::CodePreview).
//!
//! An anchored comment renders as a bubble directly under the last line of the
//! code it points at. Bubbles are extra *display rows* interleaved with the
//! source lines, so everything that scrolls (offset, scrollbar, cursor) works
//! in display rows. [`RowLayout`] is that mapping, rebuilt whenever threads are
//! added, folded or unfolded, or the pane width changes (bodies wrap).

use std::collections::HashSet;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// One comment thread shown under its anchored code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineThread {
    /// Id of the thread's root comment; identifies the thread across reloads.
    pub root_id: String,
    /// First line of the anchored node (0-indexed).
    pub anchor_start: usize,
    /// Last line of the anchored node (0-indexed). The bubble goes below it.
    pub anchor_end: usize,
    /// Char column where the node starts on `anchor_start`.
    pub start_col: usize,
    /// Char column just past the node's end on `anchor_end`.
    pub end_col: usize,
    /// The root comment first, then its replies in order.
    pub entries: Vec<InlineComment>,
}

/// One comment in a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineComment {
    pub author: String,
    /// Short, already-formatted timestamp (e.g. `2026-11-12 10:04`).
    pub when: String,
    pub body: String,
}

/// A row of the preview as displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayRow {
    /// A source line (0-indexed).
    Code(usize),
    /// Row `row` of thread `thread`'s bubble.
    Bubble { thread: usize, row: usize },
}

/// The text of one bubble row, styled at render time (the focused thread's
/// border is highlighted, so styling can't be baked into the layout).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BubbleLine {
    Top {
        author: String,
        when: String,
    },
    Body(String),
    /// A reply's header: who wrote it and when.
    ReplyHead {
        author: String,
        when: String,
    },
    /// A row of a reply's body, indented under its header.
    ReplyBody(String),
    Bottom,
}

/// Display-row mapping for the current code, threads, folds and width.
#[derive(Debug, Clone, Default)]
pub(crate) struct RowLayout {
    pub rows: Vec<DisplayRow>,
    /// Display row of each source line.
    pub line_row: Vec<usize>,
    /// Bubble rows per thread (empty while folded).
    pub bubbles: Vec<Vec<BubbleLine>>,
    /// Display row of each thread's first bubble row; `None` while folded.
    pub bubble_start: Vec<Option<usize>>,
}

impl RowLayout {
    /// Display row of source line `line`, clamped to the last line.
    pub fn row_of(&self, line: usize) -> usize {
        match self.line_row.get(line) {
            Some(row) => *row,
            None => self.line_row.last().copied().unwrap_or(0),
        }
    }

    pub fn total_rows(&self) -> usize {
        self.rows.len()
    }

    /// First and last display row of thread `thread`'s bubble, if expanded.
    pub fn bubble_rows(&self, thread: usize) -> Option<(usize, usize)> {
        let start = (*self.bubble_start.get(thread)?)?;
        Some((start, start + self.bubbles[thread].len().saturating_sub(1)))
    }
}

/// Columns a bubble row spends on chrome around its text: a space after the
/// gutter, `│ ` on the left and ` │` on the right.
pub(crate) const BUBBLE_CHROME: usize = 5;

/// Columns a reply's body is indented under its `↳ author` header.
const REPLY_INDENT: usize = 2;

/// Build the display rows for `line_count` source lines.
///
/// `text_width` is the room for bubble text; `0` (before the first render)
/// disables wrapping so every body line is one row.
pub(crate) fn build_layout(
    line_count: usize,
    threads: &[InlineThread],
    folded: &HashSet<String>,
    text_width: usize,
) -> RowLayout {
    let bubbles: Vec<Vec<BubbleLine>> = threads
        .iter()
        .map(|t| if folded.contains(&t.root_id) { Vec::new() } else { bubble_lines(t, text_width) })
        .collect();

    // Threads below each line, in the order given.
    let mut below: Vec<Vec<usize>> = vec![Vec::new(); line_count];
    for (i, t) in threads.iter().enumerate() {
        if line_count > 0 {
            below[t.anchor_end.min(line_count - 1)].push(i);
        }
    }

    let mut rows = Vec::with_capacity(line_count);
    let mut line_row = Vec::with_capacity(line_count);
    let mut bubble_start = vec![None; threads.len()];
    for (line, threads_here) in below.iter().enumerate() {
        line_row.push(rows.len());
        rows.push(DisplayRow::Code(line));
        for &thread in threads_here {
            if !bubbles[thread].is_empty() {
                bubble_start[thread] = Some(rows.len());
            }
            rows.extend((0..bubbles[thread].len()).map(|row| DisplayRow::Bubble { thread, row }));
        }
    }
    RowLayout { rows, line_row, bubbles, bubble_start }
}

fn bubble_lines(thread: &InlineThread, width: usize) -> Vec<BubbleLine> {
    let mut lines = Vec::new();
    let Some((root, replies)) = thread.entries.split_first() else {
        return lines;
    };
    lines.push(BubbleLine::Top { author: root.author.clone(), when: root.when.clone() });
    for paragraph in root.body.lines() {
        lines.extend(wrap(paragraph, width).into_iter().map(BubbleLine::Body));
    }
    for reply in replies {
        lines
            .push(BubbleLine::ReplyHead { author: reply.author.clone(), when: reply.when.clone() });
        for paragraph in reply.body.lines() {
            let room = width.saturating_sub(REPLY_INDENT);
            lines.extend(wrap(paragraph, room).into_iter().map(BubbleLine::ReplyBody));
        }
    }
    lines.push(BubbleLine::Bottom);
    lines
}

/// Word-wrap `text` to `width` display columns; `0` means no wrapping. Words
/// longer than a line are split.
pub(crate) fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.width() <= width {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let mut word = word.to_string();
        loop {
            let sep = usize::from(!line.is_empty());
            if line.width() + sep + word.width() <= width {
                if sep == 1 {
                    line.push(' ');
                }
                line.push_str(&word);
                break;
            }
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
                continue;
            }
            // A single word wider than the line: hard-split it.
            let mut head = String::new();
            let mut rest = word.chars();
            for c in rest.by_ref() {
                if head.width() + c.to_string().width() > width {
                    out.push(std::mem::take(&mut head));
                    head.push(c);
                    break;
                }
                head.push(c);
            }
            word = head + rest.as_str();
            if word.width() <= width {
                line = word;
                break;
            }
        }
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    out
}

/// Colours for one bubble row.
pub(crate) struct BubbleStyle {
    pub border: Color,
    pub text: Color,
    pub dim: Color,
    pub accent: Color,
}

/// Render one bubble row as part of a closed box that starts one column
/// after the gutter (`indent`) and ends at `width`, the room left of the
/// scrollbar. `focused` adds key hints to the bottom border.
pub(crate) fn render_bubble_line(
    line: &BubbleLine,
    indent: usize,
    width: usize,
    focused: bool,
    style: &BubbleStyle,
) -> Line<'static> {
    let border = Style::default().fg(style.border);
    let text_style = Style::default().fg(style.text);
    // Box width from its left corner to its right corner.
    let box_width = width.saturating_sub(indent + 1).max(6);
    let inner = box_width - 4;
    let mut spans = vec![Span::raw(" ".repeat(indent + 1))];
    let pad = |used: usize| Span::raw(" ".repeat(inner.saturating_sub(used)));
    match line {
        BubbleLine::Top { author, when } => {
            let label = format!(" {author} · {when} ");
            spans.push(Span::styled("┌", border));
            spans.push(Span::styled(
                format!(" {author}"),
                Style::default().fg(style.accent).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(format!(" · {when} "), Style::default().fg(style.dim)));
            spans.push(Span::styled(
                "─".repeat((box_width - 2).saturating_sub(label.width())),
                border,
            ));
            spans.push(Span::styled("┐", border));
        }
        BubbleLine::Body(text) => {
            spans.push(Span::styled("│ ", border));
            spans.push(Span::styled(text.clone(), text_style));
            spans.push(pad(text.width()));
            spans.push(Span::styled(" │", border));
        }
        BubbleLine::ReplyHead { author, when } => {
            spans.push(Span::styled("│ ", border));
            spans.push(Span::styled("↳ ", Style::default().fg(style.dim)));
            spans.push(Span::styled(author.clone(), Style::default().fg(style.accent)));
            spans.push(Span::styled(format!(" · {when}"), Style::default().fg(style.dim)));
            spans.push(pad(format!("↳ {author} · {when}").width()));
            spans.push(Span::styled(" │", border));
        }
        BubbleLine::ReplyBody(text) => {
            spans.push(Span::styled("│ ", border));
            spans.push(Span::raw(" ".repeat(REPLY_INDENT)));
            spans.push(Span::styled(text.clone(), text_style));
            spans.push(pad(REPLY_INDENT + text.width()));
            spans.push(Span::styled(" │", border));
        }
        BubbleLine::Bottom => {
            let hint = if focused { " r reply · z fold · n next " } else { "" };
            spans.push(Span::styled("└", border));
            spans.push(Span::styled(
                "─".repeat((box_width - 2).saturating_sub(hint.width())),
                border,
            ));
            spans.push(Span::styled(hint, Style::default().fg(style.dim)));
            spans.push(Span::styled("┘", border));
        }
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(id: &str, start: usize, end: usize, bodies: &[&str]) -> InlineThread {
        InlineThread {
            root_id: id.into(),
            anchor_start: start,
            anchor_end: end,
            start_col: 0,
            end_col: 1,
            entries: bodies
                .iter()
                .map(|b| InlineComment {
                    author: "a".into(),
                    when: "t".into(),
                    body: b.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn bubbles_follow_their_anchor_line() {
        let threads = vec![thread("t1", 1, 2, &["hello", "reply"])];
        let layout = build_layout(5, &threads, &HashSet::new(), 40);
        // Code 0,1,2 then 5 bubble rows (top, body, reply head, reply body,
        // bottom), then code 3,4.
        assert_eq!(layout.total_rows(), 10);
        assert_eq!(layout.row_of(2), 2);
        assert_eq!(layout.row_of(3), 8);
        assert_eq!(layout.rows[3], DisplayRow::Bubble { thread: 0, row: 0 });
        assert_eq!(layout.bubbles[0].len(), 5);
    }

    #[test]
    fn folded_threads_take_no_rows() {
        let threads = vec![thread("t1", 0, 0, &["hello"])];
        let folded: HashSet<String> = ["t1".to_string()].into();
        let layout = build_layout(3, &threads, &folded, 40);
        assert_eq!(layout.total_rows(), 3);
        assert_eq!(layout.row_of(2), 2);
    }

    #[test]
    fn narrower_panes_wrap_into_more_rows() {
        let threads = vec![thread("t1", 0, 0, &["one two three four five six"])];
        let wide = build_layout(1, &threads, &HashSet::new(), 80);
        let narrow = build_layout(1, &threads, &HashSet::new(), 10);
        assert!(narrow.total_rows() > wide.total_rows());
    }

    #[test]
    fn anchors_past_the_end_clamp_to_the_last_line() {
        let threads = vec![thread("t1", 9, 9, &["x"])];
        let layout = build_layout(2, &threads, &HashSet::new(), 40);
        assert_eq!(layout.rows[2], DisplayRow::Bubble { thread: 0, row: 0 });
    }

    #[test]
    fn wrap_breaks_on_words_and_splits_long_words() {
        assert_eq!(wrap("aa bb cc", 5), vec!["aa bb", "cc"]);
        assert_eq!(wrap("abcdefgh", 3), vec!["abc", "def", "gh"]);
        assert_eq!(wrap("short", 0), vec!["short"]);
        assert_eq!(wrap("", 10), vec![""]);
    }
}
