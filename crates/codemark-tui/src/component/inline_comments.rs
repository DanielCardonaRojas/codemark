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

/// A thread with no place in the code: a comment on the bookmark as a
/// whole, or one whose anchor no longer resolves. Listed beside the anchored
/// threads in the side-by-side layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LooseThread {
    /// Id of the thread's root comment.
    pub root_id: String,
    /// Why it has no place, shown in its header (`orphaned`, `suspended`);
    /// `None` for a whole-bookmark comment.
    pub tag: Option<&'static str>,
    /// The root comment first, then its replies in order.
    pub entries: Vec<InlineComment>,
}

/// One comment in a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineComment {
    /// The comment's id, so an action on it (edit) knows which row to update.
    pub id: String,
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
    /// A folded thread's stand-in for its replies: `↳ N replies`.
    Replies(usize),
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
        .map(|t| {
            if folded.contains(&t.root_id) {
                Vec::new()
            } else {
                bubble_lines(&t.entries, None, text_width)
            }
        })
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

/// The rows of a bubble holding `entries`, bodies wrapped to `width`. A
/// `tag` is shown before the root's time.
pub(crate) fn bubble_lines(
    entries: &[InlineComment],
    tag: Option<&str>,
    width: usize,
) -> Vec<BubbleLine> {
    let mut lines = Vec::new();
    let Some((root, replies)) = entries.split_first() else {
        return lines;
    };
    let when = match tag {
        Some(tag) => format!("{tag} · {}", root.when),
        None => root.when.clone(),
    };
    lines.push(BubbleLine::Top { author: root.author.clone(), when });
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

/// A folded thread beside the code: the root comment in full, its replies
/// collapsed to a `↳ N replies` row. A thread without replies has nothing to
/// fold and is shown whole.
pub(crate) fn folded_bubble_lines(
    entries: &[InlineComment],
    tag: Option<&str>,
    width: usize,
) -> Vec<BubbleLine> {
    let mut lines = bubble_lines(entries.get(..1).unwrap_or_default(), tag, width);
    let replies = entries.len().saturating_sub(1);
    if replies > 0 {
        lines.insert(lines.len().saturating_sub(1), BubbleLine::Replies(replies));
    }
    lines
}

/// Rows of the side-by-side comment list at which each bubble starts.
///
/// Bubble `i` wants to start at row `desired[i]` (its anchor's code row) and
/// is `heights[i]` rows tall. Bubbles keep their order and never overlap, so
/// one that would run into the bubble above is pushed down below it.
pub(crate) fn stack_rail(desired: &[usize], heights: &[usize]) -> Vec<usize> {
    let mut next_free = 0;
    desired
        .iter()
        .zip(heights)
        .map(|(&want, &height)| {
            let at = want.max(next_free);
            next_free = at + height;
            at
        })
        .collect()
}

/// Word-wrap `text` to `width` display columns; `0` means no wrapping.
/// Words longer than a line are split. Every row takes at least one
/// character, so a character wider than `width` (`界` at width 1) gets a row
/// of its own instead of stalling the wrap.
pub(crate) fn wrap(text: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    if width == 0 || text.width() <= width {
        return vec![text.to_string()];
    }
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        if line.is_empty() && word.width() <= width {
            line.push_str(word);
            continue;
        }
        if !line.is_empty() && line.width() + 1 + word.width() <= width {
            line.push(' ');
            line.push_str(word);
            continue;
        }
        if !line.is_empty() {
            out.push(std::mem::take(&mut line));
        }
        if word.width() <= width {
            line.push_str(word);
            continue;
        }
        // A word wider than a line: split it, one character at least per row.
        for c in word.chars() {
            if !line.is_empty() && line.width() + c.width().unwrap_or(0) > width {
                out.push(std::mem::take(&mut line));
            }
            line.push(c);
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
    // Italic sets comment text apart from the code around the bubble.
    let text_style = Style::default().fg(style.text).add_modifier(Modifier::ITALIC);
    // Box width from its left corner to its right corner.
    let box_width = width.saturating_sub(indent + 1).max(6);
    let inner = box_width - 4;
    let mut spans = vec![Span::raw(" ".repeat(indent + 1))];
    let pad = |used: usize| Span::raw(" ".repeat(inner.saturating_sub(used)));
    match line {
        BubbleLine::Top { author, when } => {
            // Inside the corners, keep one rule character and a space on
            // each side of the label.
            let (author, when) = fit_header(author, when, (box_width - 3).saturating_sub(2));
            let when = when.map(|w| format!(" · {w}")).unwrap_or_default();
            let label = if author.is_empty() { String::new() } else { format!(" {author}{when} ") };
            spans.push(Span::styled("┌", border));
            if !label.is_empty() {
                spans.push(Span::styled(
                    format!(" {author}"),
                    Style::default().fg(style.accent).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(format!("{when} "), Style::default().fg(style.dim)));
            }
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
            let (author, when) = fit_header(author, when, inner.saturating_sub(2));
            let when = when.map(|w| format!(" · {w}")).unwrap_or_default();
            spans.push(Span::styled("│ ", border));
            spans.push(Span::styled("↳ ", Style::default().fg(style.dim)));
            spans.push(Span::styled(author.clone(), Style::default().fg(style.accent)));
            spans.push(Span::styled(when.clone(), Style::default().fg(style.dim)));
            spans.push(pad(2 + author.width() + when.width()));
            spans.push(Span::styled(" │", border));
        }
        BubbleLine::ReplyBody(text) => {
            spans.push(Span::styled("│ ", border));
            spans.push(Span::raw(" ".repeat(REPLY_INDENT)));
            spans.push(Span::styled(text.clone(), text_style));
            spans.push(pad(REPLY_INDENT + text.width()));
            spans.push(Span::styled(" │", border));
        }
        BubbleLine::Replies(count) => {
            let label = if *count == 1 {
                "↳ 1 reply".to_string()
            } else {
                format!("↳ {count} replies")
            };
            let label = truncate(&label, inner);
            spans.push(Span::styled("│ ", border));
            spans.push(Span::styled(label.clone(), Style::default().fg(style.dim)));
            spans.push(pad(label.width()));
            spans.push(Span::styled(" │", border));
        }
        BubbleLine::Bottom => {
            // The key hint, shortened or left out when the box is too narrow.
            let room = box_width.saturating_sub(3);
            let hint = [" r reply · z fold · n next ", " r · z · n "]
                .into_iter()
                .find(|h| focused && h.width() <= room)
                .unwrap_or("");
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

/// Fit an `author · when` header into `room` columns: shorten the author
/// with `…` first, then drop the time, then shorten the author alone.
/// Returns the author to show (empty when nothing fits) and the time, if kept.
fn fit_header<'a>(author: &str, when: &'a str, room: usize) -> (String, Option<&'a str>) {
    let with_time = room.saturating_sub(" · ".width() + when.width());
    if author.width() <= with_time {
        return (author.to_string(), Some(when));
    }
    if with_time >= 2 {
        return (truncate(author, with_time), Some(when));
    }
    (truncate(author, room), None)
}

/// `text` cut to `room` columns, ending in `…` when shortened.
fn truncate(text: &str, room: usize) -> String {
    if text.width() <= room {
        return text.to_string();
    }
    if room == 0 {
        return String::new();
    }
    let mut out = String::new();
    for c in text.chars() {
        if out.width() + c.to_string().width() + 1 > room {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
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
                    id: String::new(),
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
    fn rail_bubbles_sit_at_their_anchor_unless_pushed_down() {
        // Room for both: each starts at its anchor.
        assert_eq!(stack_rail(&[2, 10], &[3, 3]), vec![2, 10]);
        // The second would overlap the first, so it starts right below it.
        assert_eq!(stack_rail(&[2, 3, 4], &[3, 2, 2]), vec![2, 5, 7]);
        assert_eq!(stack_rail(&[], &[]), Vec::<usize>::new());
    }

    #[test]
    fn tags_lead_the_header_and_folds_keep_only_the_frame() {
        let entries = vec![
            InlineComment { author: "a".into(), when: "today".into(), body: "one".into() },
            InlineComment { author: "b".into(), when: "now".into(), body: "two".into() },
        ];
        let lines = bubble_lines(&entries, Some("orphaned"), 40);
        assert_eq!(
            lines[0],
            BubbleLine::Top { author: "a".into(), when: "orphaned · today".into() }
        );
        assert_eq!(
            folded_bubble_lines(&entries, None, 40),
            vec![
                BubbleLine::Top { author: "a".into(), when: "today".into() },
                BubbleLine::Body("one".into()),
                BubbleLine::Replies(1),
                BubbleLine::Bottom,
            ],
            "the root stays, its reply collapses"
        );
        assert_eq!(
            folded_bubble_lines(&entries[..1], None, 40),
            bubble_lines(&entries[..1], None, 40),
            "nothing to fold without replies"
        );
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
        // Wide characters in a column narrower than they are still advance.
        assert_eq!(wrap("界界 a", 1), vec!["界", "界", "a"]);
        assert_eq!(wrap("a界b", 2), vec!["a", "界", "b"]);
    }

    #[test]
    fn long_authors_are_shortened_to_keep_the_box_closed() {
        let style = BubbleStyle {
            border: Color::Reset,
            text: Color::Reset,
            dim: Color::Reset,
            accent: Color::Reset,
        };
        let text = |line: &BubbleLine| -> String {
            render_bubble_line(line, 4, 40, false, &style)
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect()
        };
        let author = "Daniel Cardona Rojas the Very Long".to_string();
        let when = "today 00:26".to_string();
        let top = text(&BubbleLine::Top { author: author.clone(), when: when.clone() });
        let head = text(&BubbleLine::ReplyHead { author, when });
        assert_eq!(top.width(), 40, "{top}");
        assert!(top.ends_with("─┐") && top.contains('…') && top.contains("today 00:26"), "{top}");
        assert_eq!(head.width(), 40, "{head}");
        assert!(
            head.ends_with(" │") && head.contains('…') && head.contains("today 00:26"),
            "{head}"
        );
    }

    #[test]
    fn comment_text_is_italic_and_the_chrome_is_not() {
        let style = BubbleStyle {
            border: Color::Reset,
            text: Color::Reset,
            dim: Color::Reset,
            accent: Color::Reset,
        };
        let italic = |line: &BubbleLine, needle: &str| -> bool {
            render_bubble_line(line, 4, 40, false, &style)
                .spans
                .iter()
                .find(|s| s.content.contains(needle))
                .unwrap_or_else(|| panic!("{needle:?} not rendered"))
                .style
                .add_modifier
                .contains(Modifier::ITALIC)
        };
        assert!(italic(&BubbleLine::Body("root text".into()), "root text"));
        assert!(italic(&BubbleLine::ReplyBody("reply text".into()), "reply text"));
        let top = BubbleLine::Top { author: "claude".into(), when: "today 10:00".into() };
        assert!(!italic(&top, "claude"), "the author stays upright");
        assert!(!italic(&BubbleLine::Body("x".into()), "│"), "borders stay upright");
    }

    #[test]
    fn every_row_fits_even_very_narrow_boxes() {
        let style = BubbleStyle {
            border: Color::Reset,
            text: Color::Reset,
            dim: Color::Reset,
            accent: Color::Reset,
        };
        let thread = InlineThread {
            root_id: "t".into(),
            anchor_start: 0,
            anchor_end: 0,
            start_col: 0,
            end_col: 1,
            entries: vec![
                InlineComment {
                    id: String::new(),
                    author: "Daniel Cardona Rojas".into(),
                    when: "yesterday 23:39".into(),
                    body: "a body long enough to wrap".into(),
                },
                InlineComment {
                    id: String::new(),
                    author: "claude".into(),
                    when: "today 09:05".into(),
                    body: "ok".into(),
                },
            ],
        };
        for width in [14, 18, 24, 30] {
            // Same arithmetic as the preview: indent 4, then the box.
            let text_width = width - 4 - BUBBLE_CHROME;
            let layout =
                build_layout(1, std::slice::from_ref(&thread), &HashSet::new(), text_width);
            for line in &layout.bubbles[0] {
                let row: String = render_bubble_line(line, 4, width, true, &style)
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect();
                assert_eq!(row.width(), width, "width {width}: {row:?}");
            }
        }
    }

    #[test]
    fn headers_shorten_the_author_then_drop_the_time() {
        assert_eq!(fit_header("claude", "today", 20), ("claude".into(), Some("today")));
        assert_eq!(fit_header("Daniel Cardona", "today", 14), ("Danie…".into(), Some("today")));
        assert_eq!(
            fit_header("Daniel Cardona", "yesterday 23:39", 10),
            ("Daniel Ca…".into(), None)
        );
    }
}
