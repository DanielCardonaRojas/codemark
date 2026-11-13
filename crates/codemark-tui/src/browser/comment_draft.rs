//! Writing a comment from the code preview: `c` comments on the code under
//! the cursor, `r` replies to the comment the cursor is on, `d` deletes the
//! thread the cursor is on (after a confirmation).
//!
//! The anchor is generated when the draft opens, so a cursor outside the
//! bookmark's deepest landmark is refused right away (with the landmark's
//! lines) instead of after the user has typed a comment.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget, Wrap};

use codemark_core::engine::bookmark::BookmarkComment;
use codemark_core::engine::comment_anchor::{self, CommentAnchor};

use super::BrowserLayout;
use super::inline_threads::{bookmark_span, trimmed_line_span};
use super::text_area::TextArea;

/// What saving the draft will create.
#[derive(Debug, Clone)]
pub(crate) enum DraftKind {
    /// A new comment anchored to the code under the cursor.
    Anchored(CommentAnchor),
    /// A reply to the thread with this root comment id.
    Reply { root_id: String },
}

/// A comment being typed in the modal.
#[derive(Debug, Clone)]
pub(crate) struct CommentDraft {
    bookmark_id: String,
    repo_root: Option<String>,
    kind: DraftKind,
    body: TextArea,
}

impl CommentDraft {
    fn title(&self) -> &'static str {
        match self.kind {
            DraftKind::Anchored(_) => " New comment ",
            DraftKind::Reply { .. } => " Reply ",
        }
    }

    fn context(&self) -> String {
        match &self.kind {
            DraftKind::Anchored(anchor) => format!("On {}", anchor.anchor_hint),
            DraftKind::Reply { .. } => "Replying to the comment under the cursor".to_string(),
        }
    }
}

impl BrowserLayout {
    /// `c` in the code preview: open a draft anchored to the code under the
    /// cursor: the visual (`v`) selection, else the node at the cursor column
    /// in selection mode, else the whole line. Returns whether the key was
    /// used.
    pub(super) fn open_comment_draft(&mut self) -> bool {
        self.flush_pending_step_preview();
        let Some(step) = self.right_pane.current_step() else { return false };
        if !step.resolved {
            self.notify(
                false,
                "This bookmark doesn't resolve right now; heal it before commenting".into(),
            );
            return true;
        }
        let bookmark = step.bookmark.clone();
        let bookmark_lines = (step.line_number, step.line_end.unwrap_or(step.line_number));
        let bookmark_bytes = step.byte_range;
        let bookmark_bytes_source = step.byte_range_source;
        let Some(preview) = self.right_pane.steps.get_step_preview_mut() else { return false };
        let Some((line, col)) = preview.cursor_position() else { return false };
        let visual = preview.visual_selection();
        let code = preview.code().to_string();
        let bookmark_bytes =
            super::inline_threads::trusted_range(&code, bookmark_bytes, bookmark_bytes_source);

        let anchor = (|| {
            let language = bookmark.grammar()?;
            let bookmark_range = bookmark_span(&code, bookmark_lines, bookmark_bytes)
                .ok_or_else(|| codemark_core::error::Error::Input("empty bookmark range".into()))?;
            let comment_range = if let Some(((l1, c1), (l2, c2))) = visual {
                selection_span(&code, (l1, c1), (l2, c2)).ok_or_else(|| {
                    codemark_core::error::Error::Input("the selection is outside the code".into())
                })?
            } else {
                match col.and_then(|c| point_offset(&code, line, c)) {
                    Some(offset) => (offset, offset),
                    None => trimmed_line_span(&code, line, line).ok_or_else(|| {
                        codemark_core::error::Error::Input("the cursor line is blank".into())
                    })?,
                }
            };
            comment_anchor::generate_comment_anchor_from_source(
                &code,
                &language,
                bookmark_range,
                comment_range,
            )
        })();

        match anchor {
            Ok(anchor) => {
                if let Some(preview) = self.right_pane.steps.get_step_preview_mut() {
                    preview.exit_visual();
                }
                self.comment_draft = Some(CommentDraft {
                    bookmark_id: bookmark.id,
                    repo_root: self.right_pane.active_repo_root.clone(),
                    kind: DraftKind::Anchored(anchor),
                    body: TextArea::default(),
                });
            }
            Err(e) => self.notify(false, e.to_string()),
        }
        true
    }

    /// `r` in the code preview: reply to the thread under the cursor. Returns
    /// `false` when the cursor is on no thread, so the key falls through.
    pub(super) fn open_reply_draft(&mut self) -> bool {
        self.flush_pending_step_preview();
        let Some(step) = self.right_pane.current_step() else { return false };
        let bookmark_id = step.bookmark.id.clone();
        let Some(preview) = self.right_pane.steps.get_step_preview_mut() else { return false };
        let Some(root_id) = preview.focused_thread_id().map(str::to_string) else { return false };
        self.comment_draft = Some(CommentDraft {
            bookmark_id,
            repo_root: self.right_pane.active_repo_root.clone(),
            kind: DraftKind::Reply { root_id },
            body: TextArea::default(),
        });
        true
    }

    /// `d` in the code preview: ask to delete the thread under the cursor.
    /// Returns `false` when the cursor is on no thread, so the key falls
    /// through.
    pub(super) fn confirm_delete_thread(&mut self) -> bool {
        self.flush_pending_step_preview();
        let Some(step) = self.right_pane.current_step() else { return false };
        let bookmark_id = step.bookmark.id.clone();
        let Some(preview) = self.right_pane.steps.get_step_preview() else { return false };
        let Some(thread) = preview.focused_inline_thread() else { return false };
        let root_id = thread.root_id.clone();
        let Some(root) = thread.entries.first() else { return false };
        let excerpt = excerpt(&root.body, 60);
        let replies = match thread.entries.len() - 1 {
            0 => String::new(),
            1 => "\n1 reply will be deleted too.".to_string(),
            n => format!("\n{n} replies will be deleted too."),
        };
        let message = format!("Delete {}'s comment \"{excerpt}\"?{replies}", root.author);
        self.request_confirmation(super::ConfirmDialog::new(
            "Delete Comment",
            message,
            super::DialogAction::DeleteCommentThread {
                root_id,
                bookmark_id,
                repo_root: self.right_pane.active_repo_root.clone(),
            },
        ));
        true
    }

    /// Delete a confirmed thread and refresh the preview's comments.
    pub(super) fn delete_comment_thread(
        &mut self,
        root_id: &str,
        bookmark_id: &str,
        repo_root: Option<&str>,
    ) {
        let db = self.workspace.db_for(repo_root);
        match db.delete_comment_thread(root_id) {
            Ok(removed) => {
                tracing::debug!(target: "codemark::comments", %root_id, removed, "thread deleted from TUI");
                // The dialog is modal, so the step is normally unchanged; if it
                // isn't, the other bookmark's comments are fresh on its next load.
                if self.right_pane.current_step().is_some_and(|s| s.bookmark.id == bookmark_id) {
                    self.right_pane.reload_step_comments(db);
                }
                let message = match removed {
                    0 => "Comment was already gone".to_string(),
                    1 => "Comment deleted".to_string(),
                    n => format!("Comment and {} replies deleted", n - 1),
                };
                self.notify(true, message);
            }
            Err(e) => {
                tracing::warn!(target: "codemark::comments", error = %e, "failed to delete comment thread");
                self.notify(false, format!("Delete failed: {e}"));
            }
        }
    }

    /// Keys while the draft is open: type, Enter for a new line, arrows /
    /// Home / End to move the cursor, Ctrl+S to save, Esc to cancel. Always
    /// handled (the modal swallows input).
    pub(super) fn handle_comment_draft_key(&mut self, key: &ratatui::crossterm::event::KeyEvent) {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};
        let Some(draft) = self.comment_draft.as_mut() else { return };
        match key.code {
            KeyCode::Esc => self.comment_draft = None,
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.save_comment_draft()
            }
            _ => {
                draft.body.handle_key(key);
            }
        }
    }

    /// Save the draft. The modal closes only once the comment is written:
    /// an empty body or a failed write (say, a locked database) keeps it open
    /// with the text intact so nothing typed is lost.
    fn save_comment_draft(&mut self) {
        let Some(draft) = self.comment_draft.as_ref() else { return };
        let body = draft.body.text().trim().to_string();
        if body.is_empty() {
            return;
        }
        let comment = BookmarkComment::new(&draft.bookmark_id, comment_author(), body);
        let comment = match &draft.kind {
            DraftKind::Anchored(anchor) => comment.with_anchor(anchor.clone()),
            DraftKind::Reply { root_id } => {
                BookmarkComment { parent_id: Some(root_id.clone()), ..comment }
            }
        };
        let db = self.workspace.db_for(draft.repo_root.as_deref());
        match db.insert_comment(&comment) {
            Ok(()) => {
                tracing::debug!(target: "codemark::comments", id = %comment.id, "comment saved from TUI");
                self.right_pane.reload_step_comments(db);
                self.comment_draft = None;
                self.notify(true, "Comment added".into());
            }
            Err(e) => {
                tracing::warn!(target: "codemark::comments", error = %e, "failed to save comment");
                self.notify(false, format!("Failed to save comment (draft kept): {e}"));
            }
        }
    }

    /// Bracketed paste while the draft is open goes into its body.
    pub(super) fn paste_into_comment_draft(&mut self, text: &str) -> bool {
        let Some(draft) = self.comment_draft.as_mut() else { return false };
        draft.body.insert_str(&text.replace("\r\n", "\n").replace('\r', "\n"));
        true
    }

    /// Draw the draft modal centred over `area`.
    pub(super) fn render_comment_draft(&self, area: Rect, buf: &mut Buffer) {
        let Some(draft) = &self.comment_draft else { return };
        let palette = crate::theme::palette();
        let width = (area.width.saturating_mul(4) / 5).clamp(30.min(area.width), 76);
        let height = area.height.saturating_sub(2).clamp(6.min(area.height), 14);
        let popup = Rect {
            x: area.x + area.width.saturating_sub(width) / 2,
            y: area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        };
        Widget::render(Clear, popup, buf);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(palette.accent))
            .title(Span::styled(draft.title(), Style::default().add_modifier(Modifier::BOLD)))
            .title_bottom(Line::from(Span::styled(
                " Enter new line · Ctrl+S save · Esc cancel ",
                Style::default().fg(palette.dim),
            )));
        let inner = block.inner(popup);
        block.render(popup, buf);

        // The body is wrapped here, by display width, rather than by
        // `Paragraph::wrap`, so the cursor's row is known exactly and the
        // caret cell past a full line's end isn't dropped at the wrap.
        let width = inner.width as usize;
        let header = Paragraph::new(Line::from(Span::styled(
            draft.context(),
            Style::default().fg(palette.dim),
        )))
        .wrap(Wrap { trim: false });
        let header_rows = (header.line_count(inner.width) + 1).min(inner.height as usize);
        header.render(Rect { height: header_rows as u16, ..inner }, buf);
        let body_area = Rect {
            y: inner.y + header_rows as u16,
            height: inner.height - header_rows as u16,
            ..inner
        };

        let text_style = Style::default().fg(palette.emphasis);
        let (cursor_line, cursor_col) = draft.body.cursor_line_col();
        let mut rows = Vec::new();
        let mut cursor_row = 0;
        for (i, text) in draft.body.text().split('\n').enumerate() {
            let cursor = (i == cursor_line).then_some(cursor_col);
            if let Some(row) = wrap_draft_line(text, cursor, width, text_style, &mut rows) {
                cursor_row = row;
            }
        }
        // Scroll just enough to keep the cursor's row in view.
        let scroll = (cursor_row + 1).saturating_sub(body_area.height as usize);
        Paragraph::new(rows).scroll((scroll as u16, 0)).render(body_area, buf);
    }
}

/// Append `text`'s rows, wrapped at `width` display columns, to `rows`. With
/// `cursor` (a char column), the char there is drawn reversed as the caret (a
/// reversed space past the end, which gets its own cell like any char), and
/// the caret's index in `rows` is returned.
fn wrap_draft_line(
    text: &str,
    cursor: Option<usize>,
    width: usize,
    style: Style,
    rows: &mut Vec<Line<'static>>,
) -> Option<usize> {
    use unicode_width::UnicodeWidthChar;
    let width = width.max(1);
    let mut chars: Vec<char> = text.chars().collect();
    if cursor == Some(chars.len()) {
        chars.push(' ');
    }
    let caret = style.add_modifier(Modifier::REVERSED);
    let mut cursor_row = None;
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut used = 0;
    let flush = |run: &mut String, spans: &mut Vec<Span<'static>>| {
        if !run.is_empty() {
            spans.push(Span::styled(std::mem::take(run), style));
        }
    };
    for (i, c) in chars.into_iter().enumerate() {
        let w = c.width().unwrap_or(0);
        if used + w > width && used > 0 {
            flush(&mut run, &mut spans);
            rows.push(Line::from(std::mem::take(&mut spans)));
            used = 0;
        }
        if cursor == Some(i) {
            flush(&mut run, &mut spans);
            spans.push(Span::styled(c.to_string(), caret));
            cursor_row = Some(rows.len());
        } else {
            run.push(c);
        }
        used += w;
    }
    flush(&mut run, &mut spans);
    rows.push(Line::from(spans));
    cursor_row
}

/// The first line of `body`, cut to `max` chars with an ellipsis.
fn excerpt(body: &str, max: usize) -> String {
    let first = body.trim().lines().next().unwrap_or("");
    let mut out: String = first.chars().take(max).collect();
    if first.chars().count() > max || body.trim().lines().nth(1).is_some() {
        out.push('…');
    }
    out
}

/// Byte offset of `(line, col)` (0-indexed, `col` in chars) in `code`.
fn point_offset(code: &str, line: usize, col: usize) -> Option<usize> {
    let mut offset = 0;
    for (i, text) in code.split_inclusive('\n').enumerate() {
        if i == line {
            let within = text.char_indices().nth(col).map(|(b, _)| b)?;
            return Some(offset + within);
        }
        offset += text.len();
    }
    None
}

/// Byte span of a visual selection from `start` to `end` (`(line, col)`,
/// both characters included).
fn selection_span(
    code: &str,
    start: (usize, usize),
    end: (usize, usize),
) -> Option<(usize, usize)> {
    let from = point_offset(code, start.0, start.1)?;
    let last = point_offset(code, end.0, end.1)?;
    let width = code[last..].chars().next().map_or(0, char::len_utf8);
    Some((from, last + width))
}

/// Who a TUI comment is from: `CODEMARK_IDENTITY_NAME`, else git's
/// `user.name`, else `user` (the CLI's default author).
fn comment_author() -> String {
    if let Ok(name) = std::env::var("CODEMARK_IDENTITY_NAME")
        && !name.trim().is_empty()
    {
        return name;
    }
    std::process::Command::new("git")
        .args(["config", "user.name"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "user".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_span_includes_both_ends() {
        let code = "let a = 1;\nlet é = f(x);\n";
        assert_eq!(selection_span(code, (0, 4), (0, 4)), Some((4, 5)), "one char");
        let (s, e) = selection_span(code, (1, 4), (1, 4)).unwrap();
        assert_eq!(&code[s..e], "é", "a multi-byte char is whole");
        let (s, e) = selection_span(code, (0, 8), (1, 11)).unwrap();
        assert_eq!(&code[s..e], "1;\nlet é = f(x)");
        assert_eq!(selection_span(code, (5, 0), (5, 1)), None, "past the end");
    }

    #[test]
    fn point_offset_maps_line_and_char_column() {
        let code = "ab\ncdé f\n";
        assert_eq!(point_offset(code, 0, 1), Some(1));
        assert_eq!(point_offset(code, 1, 0), Some(3));
        assert_eq!(point_offset(code, 1, 3), Some(3 + "cdé".len()));
        assert_eq!(point_offset(code, 5, 0), None);
    }

    fn layout_with_draft() -> (BrowserLayout, tempfile::TempDir) {
        use crate::event::{EventHandler, EventHandlerConfig};
        let dir = tempfile::tempdir().expect("tempdir");
        let db = codemark_core::storage::db::Database::open(&dir.path().join("test.db")).unwrap();
        let handler = EventHandler::new(EventHandlerConfig::default()).unwrap();
        let mut layout = BrowserLayout::new(db, handler);
        layout.comment_draft = Some(CommentDraft {
            bookmark_id: "missing-bookmark".into(),
            repo_root: None,
            kind: DraftKind::Reply { root_id: "missing-root".into() },
            body: TextArea::new("typed "),
        });
        (layout, dir)
    }

    #[test]
    fn pasted_text_goes_into_the_draft() {
        let (mut layout, _dir) = layout_with_draft();
        layout.handle_event(&crate::event::Event::Paste("pasted\r\nlines".into()));
        assert_eq!(layout.comment_draft.as_ref().unwrap().body.text(), "typed pasted\nlines");
    }

    #[test]
    fn arrows_move_the_cursor_and_typing_inserts_there() {
        use crate::event::Event;
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (mut layout, _dir) = layout_with_draft();
        let key = |code| Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
        // "typed " → cursor after "typ"
        for _ in 0..3 {
            layout.handle_event(&key(KeyCode::Left));
        }
        layout.handle_event(&key(KeyCode::Char('X')));
        assert_eq!(layout.comment_draft.as_ref().unwrap().body.text(), "typXed ");

        // The caret is drawn on the char after the cursor, mid-line.
        let area = Rect::new(0, 0, 60, 12);
        let mut buf = Buffer::empty(area);
        layout.render_comment_draft(area, &mut buf);
        let cell = (0..area.height)
            .flat_map(|y| (0..area.width).map(move |x| (x, y)))
            .find(|&(x, y)| buf[(x, y)].modifier.contains(Modifier::REVERSED))
            .expect("a caret cell");
        assert_eq!(buf[cell].symbol(), "e");
    }

    fn caret_rows(text: &str, cursor: usize, width: usize) -> (Vec<String>, Option<usize>) {
        let mut rows = Vec::new();
        let row = wrap_draft_line(text, Some(cursor), width, Style::default(), &mut rows);
        (rows.iter().map(|l| l.to_string()).collect(), row)
    }

    #[test]
    fn wrapping_tracks_the_caret_row() {
        assert_eq!(caret_rows("abcdef", 4, 3), (vec!["abc".into(), "def".into()], Some(1)));
        // A full line with the cursor at its end puts the caret on a new row.
        assert_eq!(caret_rows("abc", 3, 3), (vec!["abc".into(), " ".into()], Some(1)));
        assert_eq!(caret_rows("", 0, 3), (vec![" ".into()], Some(0)));
        // Wide chars take two cells: 3 per 6-wide row.
        let (rows, row) = caret_rows(&"界".repeat(7), 6, 6);
        assert_eq!(rows.len(), 3);
        assert_eq!(row, Some(2));
    }

    #[test]
    fn a_long_draft_scrolls_to_keep_the_caret_visible() {
        let (mut layout, _dir) = layout_with_draft();
        layout.comment_draft.as_mut().unwrap().body = TextArea::new("界".repeat(200));
        let area = Rect::new(0, 0, 60, 12);
        let mut buf = Buffer::empty(area);
        layout.render_comment_draft(area, &mut buf);
        let caret = (0..area.height)
            .flat_map(|y| (0..area.width).map(move |x| (x, y)))
            .find(|&(x, y)| buf[(x, y)].modifier.contains(Modifier::REVERSED));
        assert!(caret.is_some(), "the caret is on screen");
    }

    #[test]
    fn a_failed_save_keeps_the_draft_open_with_its_text() {
        // The bookmark doesn't exist, so the foreign key rejects the insert.
        let (mut layout, _dir) = layout_with_draft();
        layout.save_comment_draft();
        let draft = layout.comment_draft.as_ref().expect("draft survives a failed save");
        assert_eq!(draft.body.text(), "typed ");
        assert!(layout.pending_notification.as_ref().is_some_and(|n| !n.success));
    }

    #[test]
    fn d_on_a_thread_deletes_it_with_its_replies_after_confirming() {
        use crate::component::{InlineComment, InlineThread};
        use crate::event::Event;
        use codemark_core::engine::bookmark::{Bookmark, BookmarkHealth};
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let (mut layout, _dir) = layout_with_draft();
        layout.comment_draft = None;
        let bookmark = Bookmark {
            id: "bm".into(),
            query: "(function_item) @target".into(),
            language: "rust".into(),
            grammar_source: None,
            forked_from: None,
            branch: None,
            file_path: "lib.rs".into(),
            content_hash: None,
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            created_by: None,
            current_resolution_id: None,
            repo_id: None,
            tags: Vec::new(),
            annotations: Vec::new(),
            comments: Vec::new(),
        };
        let comment = |id: &str, parent: Option<&str>| BookmarkComment {
            id: id.into(),
            parent_id: parent.map(str::to_string),
            ..BookmarkComment::new("bm", "ana", format!("{id} body"))
        };
        {
            let db = layout.workspace.db_for(None);
            db.insert_bookmark(&bookmark).unwrap();
            db.insert_comments(&[
                comment("root", None),
                comment("reply", Some("root")),
                comment("other", None),
            ])
            .unwrap();
        }
        layout.right_pane.steps_data = vec![crate::browser::StepData {
            file_path: "lib.rs".into(),
            line_number: 0,
            line_end: Some(3),
            bookmark,
            resolution: None,
            resolutions: Vec::new(),
            health: crate::component::HealthStatus::Healthy,
            resolved: true,
            byte_range: None,
            byte_range_source: None,
        }];
        let entry = |body: &str| InlineComment {
            author: "ana".into(),
            when: "today".into(),
            body: body.into(),
        };
        let preview = layout.right_pane.steps.get_step_preview_mut().unwrap();
        preview.set_code("line0\nline1\nline2\nline3\n".into());
        preview.set_threads(vec![InlineThread {
            root_id: "root".into(),
            anchor_start: 1,
            anchor_end: 1,
            start_col: 0,
            end_col: 4,
            entries: vec![entry("root body"), entry("reply body")],
        }]);
        layout.set_focus(super::super::FocusArea::Main);
        layout.right_pane.steps.get_step_preview_mut().unwrap().jump_to_line(1);

        let key = |code| Event::Key(KeyEvent::new(code, KeyModifiers::NONE));
        layout.handle_event(&key(KeyCode::Char('d')));
        let dialog = layout.dialog.as_ref().expect("d on a thread asks first");
        assert!(dialog.message.contains("root body"), "{}", dialog.message);
        assert!(dialog.message.contains("1 reply"), "{}", dialog.message);

        layout.handle_event(&key(KeyCode::Tab));
        layout.handle_event(&key(KeyCode::Enter));
        assert!(layout.dialog.is_none());
        let left: Vec<String> = layout
            .workspace
            .db_for(None)
            .list_comments_for_bookmark("bm")
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(left, vec!["other".to_string()]);
        assert!(layout.pending_notification.as_ref().is_some_and(|n| n.success));
    }

    #[test]
    fn excerpt_keeps_the_first_line_and_marks_cuts() {
        assert_eq!(excerpt("short", 10), "short");
        assert_eq!(excerpt("first\nsecond", 10), "first…");
        assert_eq!(excerpt("abcdefghij", 4), "abcd…");
    }
}
