//! A multi-line text buffer with a cursor, for the comment draft modal.
//!
//! The cursor is a byte offset into the text, always on a grapheme-cluster
//! boundary, so a joined emoji or an accented letter moves and deletes as one.
//! Up/Down keep a "wanted" column (in graphemes) so moving through a short
//! line and back lands where the cursor started, as in most editors.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Default)]
pub(crate) struct TextArea {
    text: String,
    /// Byte offset of the cursor, on a grapheme boundary.
    cursor: usize,
    /// Column (in graphemes) Up/Down aim for; reset by any horizontal move or edit.
    wanted_col: Option<usize>,
}

impl TextArea {
    /// A buffer holding `text` with the cursor at its end.
    #[cfg(test)]
    pub(crate) fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.len();
        Self { text, cursor, wanted_col: None }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// The cursor as `(line, column)`, 0-indexed, column in graphemes.
    pub(crate) fn cursor_line_col(&self) -> (usize, usize) {
        let before = &self.text[..self.cursor];
        let line = before.matches('\n').count();
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        (line, before[line_start..].graphemes(true).count())
    }

    /// Edits can join chars into one cluster (deleting the newline before a
    /// combining accent, say), leaving the cursor inside it; move it back to
    /// the cluster's start.
    fn snap_to_grapheme(&mut self) {
        if let Some((start, _)) = self
            .text
            .grapheme_indices(true)
            .find(|&(start, g)| start < self.cursor && self.cursor < start + g.len())
        {
            self.cursor = start;
        }
    }

    /// Length of the grapheme before / after the cursor (0 at the ends).
    fn prev_len(&self) -> usize {
        self.text[..self.cursor].graphemes(true).next_back().map_or(0, str::len)
    }

    fn next_len(&self) -> usize {
        self.text[self.cursor..].graphemes(true).next().map_or(0, str::len)
    }

    /// Apply an editing or movement key. Returns whether the key was used.
    pub(crate) fn handle_key(&mut self, key: &KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_str(c.encode_utf8(&mut [0; 4]))
            }
            KeyCode::Enter => self.insert_str("\n"),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.left(),
            KeyCode::Right => self.right(),
            KeyCode::Up => self.vertical(-1),
            KeyCode::Down => self.vertical(1),
            KeyCode::Home => self.home(),
            KeyCode::End => self.end(),
            _ => return false,
        }
        true
    }

    /// Insert `s` at the cursor and move past it.
    pub(crate) fn insert_str(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
        self.snap_to_grapheme();
        self.wanted_col = None;
    }

    fn backspace(&mut self) {
        let len = self.prev_len();
        self.text.replace_range(self.cursor - len..self.cursor, "");
        self.cursor -= len;
        self.snap_to_grapheme();
        self.wanted_col = None;
    }

    fn delete(&mut self) {
        let len = self.next_len();
        self.text.replace_range(self.cursor..self.cursor + len, "");
        self.snap_to_grapheme();
        self.wanted_col = None;
    }

    fn left(&mut self) {
        self.cursor -= self.prev_len();
        self.wanted_col = None;
    }

    fn right(&mut self) {
        self.cursor += self.next_len();
        self.wanted_col = None;
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..].find('\n').map_or(self.text.len(), |i| self.cursor + i)
    }

    fn home(&mut self) {
        self.cursor = self.line_start();
        self.wanted_col = None;
    }

    fn end(&mut self) {
        self.cursor = self.line_end();
        self.wanted_col = None;
    }

    /// Move one line up (`-1`) or down (`1`), aiming for the wanted column.
    /// On the first or last line, go to its start or end instead.
    fn vertical(&mut self, delta: isize) {
        let (line, col) = self.cursor_line_col();
        let want = *self.wanted_col.get_or_insert(col);
        let target = line as isize + delta;
        let line_count = self.text.matches('\n').count() + 1;
        if target < 0 {
            self.cursor = 0;
            return;
        }
        if target as usize >= line_count {
            self.cursor = self.text.len();
            return;
        }
        let start: usize =
            self.text.split_inclusive('\n').take(target as usize).map(str::len).sum();
        let rest = &self.text[start..];
        let line_text = rest.split('\n').next().unwrap_or("");
        let within = line_text.grapheme_indices(true).nth(want).map_or(line_text.len(), |(b, _)| b);
        self.cursor = start + within;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(area: &mut TextArea, code: KeyCode) {
        area.handle_key(&KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn type_str(area: &mut TextArea, s: &str) {
        for c in s.chars() {
            press(area, if c == '\n' { KeyCode::Enter } else { KeyCode::Char(c) });
        }
    }

    #[test]
    fn typing_after_moving_left_inserts_mid_text() {
        let mut area = TextArea::new("helo");
        press(&mut area, KeyCode::Left);
        type_str(&mut area, "l");
        assert_eq!(area.text(), "hello");
        assert_eq!(area.cursor_line_col(), (0, 4));
    }

    #[test]
    fn backspace_and_delete_act_at_the_cursor() {
        let mut area = TextArea::new("abcd");
        press(&mut area, KeyCode::Left);
        press(&mut area, KeyCode::Left);
        press(&mut area, KeyCode::Backspace);
        assert_eq!(area.text(), "acd");
        press(&mut area, KeyCode::Delete);
        assert_eq!(area.text(), "ad");
        assert_eq!(area.cursor_line_col(), (0, 1));
    }

    #[test]
    fn edits_at_the_ends_are_no_ops() {
        let mut area = TextArea::new("ab");
        press(&mut area, KeyCode::Delete);
        press(&mut area, KeyCode::Right);
        assert_eq!((area.text(), area.cursor_line_col()), ("ab", (0, 2)));
        press(&mut area, KeyCode::Home);
        press(&mut area, KeyCode::Backspace);
        press(&mut area, KeyCode::Left);
        assert_eq!((area.text(), area.cursor_line_col()), ("ab", (0, 0)));
    }

    #[test]
    fn multi_byte_chars_move_and_delete_whole() {
        let mut area = TextArea::new("aéz");
        press(&mut area, KeyCode::Left);
        press(&mut area, KeyCode::Left);
        assert_eq!(area.cursor_line_col(), (0, 1));
        press(&mut area, KeyCode::Delete);
        assert_eq!(area.text(), "az");
        press(&mut area, KeyCode::Right);
        press(&mut area, KeyCode::Backspace);
        assert_eq!(area.text(), "a");
    }

    #[test]
    fn joined_emoji_and_combining_marks_are_one_step() {
        let mut area = TextArea::new("ae\u{301}👩‍💻z");
        press(&mut area, KeyCode::Left);
        press(&mut area, KeyCode::Left);
        assert_eq!(area.cursor_line_col(), (0, 2), "the emoji is one column");
        press(&mut area, KeyCode::Backspace);
        assert_eq!(area.text(), "a👩‍💻z", "the accented e goes whole");
        press(&mut area, KeyCode::Delete);
        assert_eq!(area.text(), "az", "the joined emoji goes whole");
    }

    #[test]
    fn an_edit_that_joins_a_cluster_keeps_the_cursor_on_a_boundary() {
        // Deleting the newline joins the accent onto `a`.
        let mut area = TextArea::new("a\n\u{301}b");
        press(&mut area, KeyCode::Home);
        press(&mut area, KeyCode::Backspace);
        assert_eq!(area.text(), "a\u{301}b");
        type_str(&mut area, "X");
        assert_eq!(area.text(), "Xa\u{301}b", "the accent stays on its letter");
    }

    #[test]
    fn left_and_right_cross_line_breaks() {
        let mut area = TextArea::new("ab\ncd");
        press(&mut area, KeyCode::Home);
        press(&mut area, KeyCode::Left);
        assert_eq!(area.cursor_line_col(), (0, 2));
        press(&mut area, KeyCode::Right);
        assert_eq!(area.cursor_line_col(), (1, 0));
    }

    #[test]
    fn up_and_down_keep_the_column_through_short_lines() {
        let mut area = TextArea::new("long line\nab\nanother line");
        // End of the last line is col 12; go to col 5 first.
        press(&mut area, KeyCode::Home);
        for _ in 0..5 {
            press(&mut area, KeyCode::Right);
        }
        press(&mut area, KeyCode::Up);
        assert_eq!(area.cursor_line_col(), (1, 2), "clamped to the short line");
        press(&mut area, KeyCode::Up);
        assert_eq!(area.cursor_line_col(), (0, 5), "the wanted column comes back");
        press(&mut area, KeyCode::Up);
        assert_eq!(area.cursor_line_col(), (0, 0), "up on the first line goes to the start");
        press(&mut area, KeyCode::Down);
        press(&mut area, KeyCode::Down);
        press(&mut area, KeyCode::Down);
        assert_eq!(area.cursor_line_col(), (2, 12), "down on the last line goes to the end");
    }

    #[test]
    fn home_and_end_stay_on_the_line() {
        let mut area = TextArea::new("ab\ncde\nf");
        press(&mut area, KeyCode::Up);
        press(&mut area, KeyCode::Home);
        assert_eq!(area.cursor_line_col(), (1, 0));
        press(&mut area, KeyCode::End);
        assert_eq!(area.cursor_line_col(), (1, 3));
    }

    #[test]
    fn insert_str_lands_at_the_cursor() {
        let mut area = TextArea::new("ad");
        press(&mut area, KeyCode::Left);
        area.insert_str("b\nc");
        assert_eq!(area.text(), "ab\ncd");
        assert_eq!(area.cursor_line_col(), (1, 1));
    }

    #[test]
    fn control_chords_are_not_typed() {
        let mut area = TextArea::new("");
        let used = area.handle_key(&KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(!used);
        assert_eq!(area.text(), "");
    }
}
