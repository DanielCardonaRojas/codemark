//! Writing a comment from the code preview: `c` comments on the code under
//! the cursor, `r` replies to the comment the cursor is on.
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
use super::inline_threads::trimmed_line_span;

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
    body: String,
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
    /// cursor (the node at the cursor column in selection mode, else the
    /// whole line). Returns whether the key was used.
    pub(super) fn open_comment_draft(&mut self) -> bool {
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
        let Some(preview) = self.right_pane.steps.get_step_preview_mut() else { return false };
        let Some((line, col)) = preview.cursor_position() else { return false };
        let code = preview.code().to_string();

        let anchor = (|| {
            let language = bookmark.grammar()?;
            let bookmark_range = trimmed_line_span(&code, bookmark_lines.0, bookmark_lines.1)
                .ok_or_else(|| codemark_core::error::Error::Input("empty bookmark range".into()))?;
            let comment_range = match col.and_then(|c| point_offset(&code, line, c)) {
                Some(offset) => (offset, offset),
                None => trimmed_line_span(&code, line, line).ok_or_else(|| {
                    codemark_core::error::Error::Input("the cursor line is blank".into())
                })?,
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
                self.comment_draft = Some(CommentDraft {
                    bookmark_id: bookmark.id,
                    repo_root: self.right_pane.active_repo_root.clone(),
                    kind: DraftKind::Anchored(anchor),
                    body: String::new(),
                });
            }
            Err(e) => self.notify(false, e.to_string()),
        }
        true
    }

    /// `r` in the code preview: reply to the thread under the cursor. Returns
    /// `false` when the cursor is on no thread, so the key falls through.
    pub(super) fn open_reply_draft(&mut self) -> bool {
        let Some(step) = self.right_pane.current_step() else { return false };
        let bookmark_id = step.bookmark.id.clone();
        let Some(preview) = self.right_pane.steps.get_step_preview_mut() else { return false };
        let Some(root_id) = preview.focused_thread_id().map(str::to_string) else { return false };
        self.comment_draft = Some(CommentDraft {
            bookmark_id,
            repo_root: self.right_pane.active_repo_root.clone(),
            kind: DraftKind::Reply { root_id },
            body: String::new(),
        });
        true
    }

    /// Keys while the draft is open: type, Enter for a new line, Ctrl+S to
    /// save, Esc to cancel. Always handled (the modal swallows input).
    pub(super) fn handle_comment_draft_key(&mut self, key: &ratatui::crossterm::event::KeyEvent) {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};
        let Some(draft) = self.comment_draft.as_mut() else { return };
        match key.code {
            KeyCode::Esc => self.comment_draft = None,
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.save_comment_draft()
            }
            KeyCode::Enter => draft.body.push('\n'),
            KeyCode::Backspace => {
                draft.body.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                draft.body.push(c)
            }
            _ => {}
        }
    }

    fn save_comment_draft(&mut self) {
        let Some(draft) = self.comment_draft.take() else { return };
        let body = draft.body.trim().to_string();
        if body.is_empty() {
            // Keep the modal open: an empty comment is never what was meant.
            self.comment_draft = Some(draft);
            return;
        }
        let comment = BookmarkComment::new(&draft.bookmark_id, comment_author(), body);
        let comment = match draft.kind {
            DraftKind::Anchored(anchor) => comment.with_anchor(anchor),
            DraftKind::Reply { root_id } => BookmarkComment { parent_id: Some(root_id), ..comment },
        };
        let db = self.workspace.db_for(draft.repo_root.as_deref());
        match db.insert_comment(&comment) {
            Ok(()) => {
                tracing::debug!(target: "codemark::comments", id = %comment.id, "comment saved from TUI");
                self.right_pane.reload_step_comments(db);
                self.notify(true, "Comment added".into());
            }
            Err(e) => self.notify(false, format!("Failed to save comment: {e}")),
        }
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

        let mut lines = vec![
            Line::from(Span::styled(draft.context(), Style::default().fg(palette.dim))),
            Line::default(),
        ];
        let body_lines: Vec<&str> = draft.body.split('\n').collect();
        let last = body_lines.len() - 1;
        for (i, text) in body_lines.into_iter().enumerate() {
            let mut spans =
                vec![Span::styled(text.to_string(), Style::default().fg(palette.emphasis))];
            if i == last {
                spans.push(Span::styled("▏", Style::default().fg(palette.accent)));
            }
            lines.push(Line::from(spans));
        }
        // Keep the end of a long draft (where the cursor is) in view.
        let wrapped = Paragraph::new(lines.clone()).wrap(Wrap { trim: false });
        let overflow = wrapped.line_count(inner.width).saturating_sub(inner.height as usize);
        wrapped.scroll((overflow as u16, 0)).render(inner, buf);
    }
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
    fn point_offset_maps_line_and_char_column() {
        let code = "ab\ncdé f\n";
        assert_eq!(point_offset(code, 0, 1), Some(1));
        assert_eq!(point_offset(code, 1, 0), Some(3));
        assert_eq!(point_offset(code, 1, 3), Some(3 + "cdé".len()));
        assert_eq!(point_offset(code, 5, 0), None);
    }
}
