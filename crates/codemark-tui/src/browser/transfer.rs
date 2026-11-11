//! Copying or moving a collection onto another git branch (#306).
//!
//! The TUI front-end for `codemark tour copy` / `tour move`:
//!
//! 1. `c` / `m` on the Collections tab opens a [`BranchPicker`]: a modal,
//!    filterable list of the repository's branches with the common ones
//!    (default branch, `main`, `develop`, the current branch) pinned first.
//! 2. Picking a branch runs [`engine::plan_transfer`] (a dry run) on a
//!    background task, which reports back through [`Event::TransferPlanned`].
//! 3. A plan with nothing to transfer, or one that failed, becomes an error
//!    notification. Otherwise a confirmation dialog shows what matches (and
//!    which steps will be left out), and confirming runs
//!    [`engine::apply_transfer`] in one transaction.
//!
//! The TUI always reads the branch's committed tip; working-tree transfers
//! stay CLI-only.

use codemark_core::engine::transfer::{
    self as engine, OnUnmatched, TransferMode, TransferOptions, TransferPlan, TransferTarget,
};
use codemark_core::git::branches::{self, BranchRef};
use codemark_core::storage::db::Database;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget};
use std::path::{Path, PathBuf};

use super::{BrowserLayout, ConfirmDialog, ContentTab, DialogAction, HealNotification};
use crate::event::Event;

/// Unmatched steps listed by name in the confirmation dialog before the rest
/// are summarized as "…and N more".
const MAX_LISTED_UNMATCHED: usize = 4;

/// Whether a transfer clones the collection or re-points it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferKind {
    /// Clone into a new collection on the target branch.
    Copy,
    /// Re-point the existing collection at the target branch.
    Move,
}

impl TransferKind {
    fn verb(self) -> &'static str {
        match self {
            Self::Copy => "Copy",
            Self::Move => "Move",
        }
    }

    fn verb_lower(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Move => "move",
        }
    }

    fn past(self) -> &'static str {
        match self {
            Self::Copy => "Copied",
            Self::Move => "Moved",
        }
    }
}

/// What a key press in the [`BranchPicker`] asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerOutcome {
    /// Keep the picker open.
    Continue,
    /// Close the picker without doing anything.
    Cancel,
    /// Transfer to this branch.
    Pick(BranchRef),
}

/// A modal, filterable branch list for choosing a transfer target.
#[derive(Debug, Clone)]
pub struct BranchPicker {
    pub kind: TransferKind,
    pub collection_id: String,
    pub collection_name: String,
    /// The database holding the collection, pinned when the picker opens so
    /// the transfer stays on it even if the repo scope changes meanwhile.
    pub db_path: PathBuf,
    /// All branches, pinned first (see [`branches::list_branches`]).
    branches: Vec<BranchRef>,
    filter: String,
    /// Index into [`Self::visible`].
    selected: usize,
}

impl BranchPicker {
    pub fn new(
        kind: TransferKind,
        collection_id: String,
        collection_name: String,
        db_path: PathBuf,
        branches: Vec<BranchRef>,
    ) -> Self {
        Self {
            kind,
            collection_id,
            collection_name,
            db_path,
            branches,
            filter: String::new(),
            selected: 0,
        }
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Branches matching the filter, best matches first.
    ///
    /// A case-insensitive substring match ranks above a subsequence match
    /// (`fx` finds `feat-x`); within a rank the pinned-first order is kept, so
    /// an empty filter shows the full list with `main` on top.
    pub fn visible(&self) -> Vec<&BranchRef> {
        if self.filter.is_empty() {
            return self.branches.iter().collect();
        }
        let needle = self.filter.to_lowercase();
        let mut ranked: Vec<(u8, &BranchRef)> = self
            .branches
            .iter()
            .filter_map(|b| {
                let hay = b.name.to_lowercase();
                if hay.contains(&needle) {
                    Some((0, b))
                } else if is_subsequence(&needle, &hay) {
                    Some((1, b))
                } else {
                    None
                }
            })
            .collect();
        ranked.sort_by_key(|(rank, _)| *rank);
        ranked.into_iter().map(|(_, b)| b).collect()
    }

    /// The highlighted branch, if any branch matches the filter.
    pub fn selected_branch(&self) -> Option<&BranchRef> {
        self.visible().get(self.selected).copied()
    }

    /// Append text to the filter (typed characters or a paste).
    pub fn push_str(&mut self, s: &str) {
        self.filter.extend(s.chars().filter(|c| !c.is_control()));
        self.selected = 0;
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> PickerOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let count = self.visible().len();
        match key.code {
            KeyCode::Esc => return PickerOutcome::Cancel,
            KeyCode::Enter => {
                return match self.selected_branch() {
                    Some(b) => PickerOutcome::Pick(b.clone()),
                    None => PickerOutcome::Continue,
                };
            }
            KeyCode::Up | KeyCode::BackTab => self.move_selection(-1, count),
            KeyCode::Down | KeyCode::Tab => self.move_selection(1, count),
            KeyCode::Char('p') if ctrl => self.move_selection(-1, count),
            KeyCode::Char('n') if ctrl => self.move_selection(1, count),
            KeyCode::Char('u') if ctrl => {
                self.filter.clear();
                self.selected = 0;
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.selected = 0;
            }
            KeyCode::Char(c) if !ctrl => self.push_str(&c.to_string()),
            _ => {}
        }
        PickerOutcome::Continue
    }

    fn move_selection(&mut self, delta: isize, count: usize) {
        if count == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(count as isize) as usize;
    }
}

fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut hay = hay.chars();
    needle.chars().all(|c| hay.any(|h| h == c))
}

/// The result of planning a transfer off the UI thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferPlanned {
    pub kind: TransferKind,
    pub collection_id: String,
    pub collection_name: String,
    /// The database the plan was computed against (see [`BranchPicker::db_path`]).
    pub db_path: PathBuf,
    pub branch: String,
    /// The plan and, for a copy, the new collection's name. `Err` carries a
    /// displayable reason.
    pub result: Result<(TransferPlan, Option<String>), String>,
}

/// Why a successful plan can't be applied, if it can't.
fn blocking_reason(plan: &TransferPlan) -> Option<String> {
    let total = plan.steps.len();
    if total == 0 {
        return Some("the collection has no steps".to_string());
    }
    if plan.matched().next().is_none() {
        let first = plan
            .unmatched()
            .find_map(|s| s.reason.clone())
            .map(|r| format!(" (e.g. {r})"))
            .unwrap_or_default();
        return Some(format!(
            "none of its {total} step{} match on that branch{first}",
            if total == 1 { "" } else { "s" }
        ));
    }
    None
}

/// The confirmation message for an appliable plan.
fn confirmation_message(
    kind: TransferKind,
    collection: &str,
    copy_name: Option<&str>,
    plan: &TransferPlan,
) -> String {
    let commit = plan.target_commit.as_deref().map(|c| &c[..c.len().min(8)]).unwrap_or("?");
    let mut msg = format!("{} \"{collection}\" to {} ({commit})", kind.verb(), plan.target_branch);
    if let Some(name) = copy_name {
        msg.push_str(&format!(" as \"{name}\""));
    }
    msg.push('?');
    let total = plan.steps.len();
    let matched = plan.matched().count();
    msg.push_str(&format!("\n{matched} of {total} steps match."));
    if matched < total {
        msg.push_str(&format!(
            "\n{} will be left out (their bookmarks are kept):",
            total - matched
        ));
        let unmatched: Vec<_> = plan.unmatched().collect();
        for s in unmatched.iter().take(MAX_LISTED_UNMATCHED) {
            msg.push_str(&format!("\n✗ {}. {}", s.position + 1, s.file_path));
        }
        if unmatched.len() > MAX_LISTED_UNMATCHED {
            msg.push_str(&format!("\n…and {} more", unmatched.len() - MAX_LISTED_UNMATCHED));
        }
    }
    msg
}

impl BrowserLayout {
    /// Open the branch picker for the selected collection (`c` / `m`).
    pub(super) fn open_branch_picker(&mut self, kind: TransferKind) -> Option<bool> {
        if !matches!(self.active_content_tab(), Some(ContentTab::Collections)) {
            return None;
        }
        // Rows carry the collection id; names aren't unique, so the name is only
        // a fallback for rows without one.
        let (id, name, repo_root) =
            self.left_pane.content_panel.active_panel().and_then(|panel| panel.selected()).map(
                |s| (s.user_data.clone(), s.text().to_string(), s.repo_root().map(str::to_string)),
            )?;
        if self.transfer_planning {
            self.notify(false, "A transfer is already being planned".to_string());
            return Some(true);
        }

        let db = self.workspace.db_for(repo_root.as_deref());
        let found = match &id {
            Some(id) => db.get_collection_by_id(id),
            None => db.get_collection_by_name(&name),
        };
        let collection = match found {
            Ok(Some(c)) => c,
            Ok(None) => {
                self.notify(false, format!("Collection \"{name}\" not found"));
                return Some(true);
            }
            Err(e) => {
                self.notify(false, format!("Can't read collection \"{name}\": {e}"));
                return Some(true);
            }
        };
        let name = collection.name.clone();
        // Pin the source database now: the repo scope can change while the
        // plan runs, and `db_for` would then fall back to another repo.
        let db_path = db.path().to_path_buf();
        let repo_path = engine::repo_root_for(db);
        let branches = match branches::list_branches(&repo_path) {
            Ok(b) if !b.is_empty() => b,
            Ok(_) => {
                self.notify(
                    false,
                    format!(
                        "Can't {} \"{name}\": the repository has no branches",
                        kind.verb_lower()
                    ),
                );
                return Some(true);
            }
            Err(e) => {
                tracing::warn!(target: "codemark::git", error = %e, "listing branches failed");
                self.notify(false, format!("Can't {} \"{name}\": {e}", kind.verb_lower()));
                return Some(true);
            }
        };
        tracing::debug!(
            target: "codemark::ui",
            collection = %name,
            ?kind,
            branches = branches.len(),
            "branch picker opened"
        );
        self.branch_picker = Some(BranchPicker::new(kind, collection.id, name, db_path, branches));
        Some(true)
    }

    /// Route a key to the open branch picker. Always returns `true` (modal).
    pub(super) fn handle_branch_picker_key(&mut self, key: &KeyEvent) -> bool {
        let Some(picker) = self.branch_picker.as_mut() else { return false };
        match picker.handle_key(key) {
            PickerOutcome::Continue => {}
            PickerOutcome::Cancel => {
                tracing::debug!(target: "codemark::ui", "branch picker cancelled");
                self.branch_picker = None;
            }
            PickerOutcome::Pick(branch) => {
                if let Some(picker) = self.branch_picker.take() {
                    self.start_transfer_plan(picker, branch);
                }
            }
        }
        true
    }

    /// Plan the transfer on a background task; the result arrives as
    /// [`Event::TransferPlanned`].
    fn start_transfer_plan(&mut self, picker: BranchPicker, branch: BranchRef) {
        tracing::debug!(
            target: "codemark::ui",
            collection = %picker.collection_name,
            branch = %branch.name,
            rev = %branch.rev,
            kind = ?picker.kind,
            "planning collection transfer"
        );
        let db_path = picker.db_path.clone();
        self.transfer_planning = true;
        self.add_spinner(&picker.collection_id, self.tab_position(ContentTab::Collections));
        let event_handler = self.event_handler.clone();

        tokio::task::spawn_blocking(move || {
            let result = (|| -> Result<(TransferPlan, Option<String>), String> {
                let db = Database::open(&db_path).map_err(|e| e.to_string())?;
                let collection = db
                    .get_collection_by_id(&picker.collection_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "the collection no longer exists".to_string())?;
                let repo_root = engine::repo_root_for(&db);
                let sha =
                    engine::resolve_revision(&repo_root, &branch.rev).map_err(|e| e.to_string())?;
                let options = TransferOptions {
                    target_branch: branch.name.clone(),
                    target: TransferTarget::Revision { rev: branch.rev.clone(), sha },
                    allow_fuzzy: false,
                };
                // On a spawn_blocking thread, so block directly.
                let plan = tokio::runtime::Handle::current()
                    .block_on(engine::plan_transfer(&db, &collection, &options))
                    .map_err(|e| e.to_string())?;
                let copy_name = match picker.kind {
                    TransferKind::Copy => Some(
                        engine::unique_copy_name(&db, &collection.name, &branch.name)
                            .map_err(|e| e.to_string())?,
                    ),
                    TransferKind::Move => None,
                };
                Ok((plan, copy_name))
            })();
            if let Err(e) = &result {
                tracing::warn!(target: "codemark::ui", error = %e, "planning transfer failed");
            }
            let _ = event_handler.send(Event::TransferPlanned(Box::new(TransferPlanned {
                kind: picker.kind,
                collection_id: picker.collection_id,
                collection_name: picker.collection_name,
                db_path: picker.db_path,
                branch: branch.name,
                result,
            })));
        });
    }

    /// Turn a finished plan into a notification (can't transfer) or a
    /// confirmation dialog (can).
    pub(super) fn on_transfer_planned(&mut self, planned: &TransferPlanned) {
        self.transfer_planning = false;
        self.schedule_clear_spinners();
        let verb = planned.kind.verb_lower();
        let cant = |reason: &str| {
            format!("Can't {verb} \"{}\" to {}: {reason}", planned.collection_name, planned.branch)
        };
        let (plan, copy_name) = match &planned.result {
            Ok(ok) => ok,
            Err(e) => {
                self.notify(false, cant(e));
                return;
            }
        };
        tracing::debug!(
            target: "codemark::ui",
            collection = %planned.collection_name,
            branch = %planned.branch,
            matched = plan.matched().count(),
            unmatched = plan.unmatched().count(),
            "transfer planned"
        );
        if let Some(reason) = blocking_reason(plan) {
            self.notify(false, cant(&reason));
            return;
        }
        let mode = match copy_name {
            Some(new_name) => TransferMode::Copy { new_name: new_name.clone() },
            None => TransferMode::Move,
        };
        let dialog = ConfirmDialog::new(
            format!("{} Collection", planned.kind.verb()),
            confirmation_message(
                planned.kind,
                &planned.collection_name,
                copy_name.as_deref(),
                plan,
            ),
            DialogAction::ApplyTransfer {
                plan: Box::new(plan.clone()),
                mode,
                db_path: planned.db_path.clone(),
            },
        );
        if self.dialog.is_some() {
            // Don't clobber a dialog the user opened while the plan ran (e.g. a
            // delete confirmation); show this one once that closes.
            tracing::debug!(target: "codemark::ui", "transfer confirmation queued");
            self.queued_dialog = Some(dialog);
        } else {
            self.request_confirmation(dialog);
        }
    }

    /// Apply a confirmed plan. Returns the success notification text.
    pub(super) fn apply_confirmed_transfer(
        &mut self,
        plan: &TransferPlan,
        mode: &TransferMode,
        db_path: &Path,
    ) -> Result<String, String> {
        // Only plans with at least one match reach the dialog; drop the rest.
        let on_unmatched = if plan.has_unmatched() { OnUnmatched::Drop } else { OnUnmatched::Fail };
        let verb = match mode {
            TransferMode::Copy { .. } => "copy",
            TransferMode::Move => "move",
        };
        let fail = |e: &dyn std::fmt::Display| {
            tracing::warn!(target: "codemark::ui", error = %e, "applying transfer failed");
            format!("Can't {verb} \"{}\" to {}: {e}", plan.collection, plan.target_branch)
        };
        // Write through the database the plan came from, even if its repo has
        // since left the scope (a fresh connection; the workspace's see the
        // change on their next read).
        let outcome = match self.workspace.dbs().find(|(_, db)| db.path() == db_path) {
            Some((_, db)) => engine::apply_transfer(db, plan, mode, on_unmatched, None),
            None => Database::open(db_path)
                .and_then(|db| engine::apply_transfer(&db, plan, mode, on_unmatched, None)),
        }
        .map_err(|e| fail(&e))?;
        tracing::debug!(
            target: "codemark::ui",
            mode = outcome.mode,
            collection = %outcome.collection,
            forked = outcome.forked.len(),
            dropped = outcome.dropped.len(),
            needs_embedding = outcome.needs_embedding.len(),
            "transfer applied"
        );
        let kind = match mode {
            TransferMode::Copy { .. } => TransferKind::Copy,
            TransferMode::Move => TransferKind::Move,
        };
        let steps = outcome.forked.len();
        let mut msg = match mode {
            TransferMode::Copy { new_name } => format!(
                "{} \"{}\" to {} as \"{new_name}\"",
                kind.past(),
                plan.collection,
                outcome.target_branch
            ),
            TransferMode::Move => {
                format!("{} \"{}\" to {}", kind.past(), plan.collection, outcome.target_branch)
            }
        };
        msg.push_str(&format!(": {steps} step{}", if steps == 1 { "" } else { "s" }));
        if !outcome.dropped.is_empty() {
            msg.push_str(&format!(", {} left out", outcome.dropped.len()));
        }
        Ok(msg)
    }

    fn notify(&mut self, success: bool, message: String) {
        self.pending_notification = Some(HealNotification { message, success });
    }

    /// Bindings shown while the branch picker is open.
    pub(super) fn branch_picker_bindings() -> [(&'static str, &'static str); 4] {
        [("type", "Filter"), ("↑/↓", "Select branch"), ("Enter", "Pick"), ("Esc", "Cancel")]
    }

    /// Render the branch picker (if open) centered over `area`.
    pub(super) fn render_branch_picker(&self, area: Rect, buf: &mut Buffer) {
        if let Some(picker) = &self.branch_picker {
            render_picker(picker, area, buf);
        }
    }
}

/// One rendered row of the picker list.
enum Row<'a> {
    Header(&'static str),
    Branch(usize, &'a BranchRef),
}

fn render_picker(picker: &BranchPicker, area: Rect, buf: &mut Buffer) {
    let palette = crate::theme::palette();
    let width = (area.width.saturating_mul(4) / 5).clamp(30.min(area.width), 72);
    let height = area.height.saturating_sub(2).clamp(5.min(area.height), 22);
    let popup = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    Widget::render(Clear, popup, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(format!(" {} \"{}\" to branch ", picker.kind.verb(), picker.collection_name))
        .title_style(Style::default().bold())
        .border_style(Style::default().fg(palette.accent));
    let inner = block.inner(popup);
    block.render(popup, buf);
    if inner.height < 3 {
        return;
    }

    // Filter input, then a hint line, then the list.
    let visible = picker.visible();
    let input = Line::from(vec![
        Span::styled("> ", Style::default().fg(palette.accent).bold()),
        Span::raw(picker.filter().to_string()),
        Span::styled("█", Style::default().fg(palette.accent)),
    ]);
    buf.set_line(inner.x, inner.y, &input, inner.width);
    let hint = format!(
        "{} of {} · ↑/↓ select · Enter pick · Esc cancel",
        visible.len(),
        picker.branches.len()
    );
    buf.set_line(
        inner.x,
        inner.y + 1,
        &Line::from(Span::styled(hint, Style::default().fg(palette.dim))),
        inner.width,
    );

    let list_area = Rect { y: inner.y + 2, height: inner.height - 2, ..inner };
    let mut rows = Vec::new();
    let mut in_pinned = None;
    for (i, b) in visible.iter().enumerate() {
        if in_pinned != Some(b.pinned) {
            rows.push(Row::Header(if b.pinned { "Common" } else { "Branches" }));
            in_pinned = Some(b.pinned);
        }
        rows.push(Row::Branch(i, b));
    }
    if rows.is_empty() {
        buf.set_line(
            list_area.x,
            list_area.y,
            &Line::from(Span::styled(
                "  No branch matches the filter",
                Style::default().fg(palette.dim).italic(),
            )),
            list_area.width,
        );
        return;
    }
    let selected_row =
        rows.iter().position(|r| matches!(r, Row::Branch(i, _) if *i == picker.selected));
    let capacity = list_area.height as usize;
    let offset = selected_row.map(|r| (r + 1).saturating_sub(capacity)).unwrap_or(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();

    for (line_no, row) in rows.iter().skip(offset).take(capacity).enumerate() {
        let y = list_area.y + line_no as u16;
        let line = match row {
            Row::Header(title) => {
                Line::from(Span::styled(*title, Style::default().fg(palette.dim).bold()))
            }
            Row::Branch(i, b) => {
                let is_selected = *i == picker.selected;
                let mut tags = Vec::new();
                if b.is_default {
                    tags.push("default");
                }
                if b.is_current {
                    tags.push("current");
                }
                if b.is_remote {
                    tags.push("remote");
                }
                let mut spans = vec![
                    Span::raw(if is_selected { "▸ " } else { "  " }),
                    Span::styled(b.name.clone(), Style::default().bold()),
                ];
                for t in tags {
                    spans.push(Span::styled(format!("  {t}"), Style::default().fg(palette.info)));
                }
                if b.last_commit > 0 {
                    spans.push(Span::styled(
                        format!("  {}", relative_age(now - b.last_commit)),
                        Style::default().fg(palette.dim),
                    ));
                }
                let line = Line::from(spans);
                if is_selected {
                    // Fill the whole row so the highlight reads as a bar.
                    let bar = Rect { y, height: 1, ..list_area };
                    buf.set_style(bar, Style::default().bg(palette.accent).fg(palette.inverse));
                    line.style(Style::default().bg(palette.accent).fg(palette.inverse))
                } else {
                    line
                }
            }
        };
        buf.set_line(list_area.x, y, &line, list_area.width);
    }
}

/// "3m ago", "5h ago", "2d ago", "4mo ago", "1y ago".
fn relative_age(secs: i64) -> String {
    let secs = secs.max(0);
    let (n, unit) = match secs {
        s if s < 3600 => (s / 60, "m"),
        s if s < 86_400 => (s / 3600, "h"),
        s if s < 86_400 * 30 => (s / 86_400, "d"),
        s if s < 86_400 * 365 => (s / (86_400 * 30), "mo"),
        s => (s / (86_400 * 365), "y"),
    };
    format!("{n}{unit} ago")
}

#[cfg(test)]
mod tests {
    use super::*;
    use codemark_core::engine::transfer::StepStatus;

    fn branch(name: &str, pinned: bool) -> BranchRef {
        BranchRef {
            name: name.to_string(),
            rev: name.to_string(),
            is_remote: false,
            is_current: false,
            is_default: name == "main",
            pinned,
            last_commit: 0,
        }
    }

    fn picker() -> BranchPicker {
        BranchPicker::new(
            TransferKind::Move,
            "id".into(),
            "tour".into(),
            PathBuf::from("/tmp/db"),
            vec![
                branch("main", true),
                branch("develop", true),
                branch("feat-x", false),
                branch("fix-main-crash", false),
                branch("fx-tools", false),
            ],
        )
    }

    fn names(p: &BranchPicker) -> Vec<&str> {
        p.visible().iter().map(|b| b.name.as_str()).collect()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    #[test]
    fn enter_with_empty_filter_picks_the_pinned_default_branch() {
        let mut p = picker();
        assert_eq!(p.handle_key(&key(KeyCode::Enter)), PickerOutcome::Pick(branch("main", true)));
    }

    #[test]
    fn typing_filters_substring_matches_before_subsequence_matches() {
        let mut p = picker();
        for c in "fx".chars() {
            p.handle_key(&key(KeyCode::Char(c)));
        }
        // "fx-tools" contains "fx"; the others only match as a subsequence.
        assert_eq!(names(&p), ["fx-tools", "feat-x", "fix-main-crash"]);
        p.handle_key(&key(KeyCode::Backspace));
        p.handle_key(&key(KeyCode::Backspace));
        for c in "main".chars() {
            p.handle_key(&key(KeyCode::Char(c)));
        }
        // Both contain "main"; pinned order is kept within the rank.
        assert_eq!(names(&p), ["main", "fix-main-crash"]);
        p.push_str("x");
        assert!(names(&p).is_empty());
        assert_eq!(p.handle_key(&key(KeyCode::Enter)), PickerOutcome::Continue);
    }

    #[test]
    fn arrows_wrap_and_typing_resets_selection() {
        let mut p = picker();
        p.handle_key(&key(KeyCode::Up));
        assert_eq!(p.selected_branch().unwrap().name, "fx-tools");
        p.handle_key(&key(KeyCode::Down));
        p.handle_key(&key(KeyCode::Down));
        assert_eq!(p.selected_branch().unwrap().name, "develop");
        p.handle_key(&key(KeyCode::Char('f')));
        assert_eq!(p.selected_branch().unwrap().name, "feat-x");
    }

    #[test]
    fn esc_cancels_and_q_is_typed_not_quit() {
        let mut p = picker();
        assert_eq!(p.handle_key(&key(KeyCode::Char('q'))), PickerOutcome::Continue);
        assert_eq!(p.filter(), "q");
        assert_eq!(p.handle_key(&key(KeyCode::Esc)), PickerOutcome::Cancel);
    }

    fn plan(statuses: &[StepStatus]) -> TransferPlan {
        TransferPlan {
            collection: "tour".into(),
            collection_id: "c".into(),
            source_branch: Some("feat".into()),
            target_branch: "main".into(),
            target_commit: Some("abcdef1234".into()),
            allow_fuzzy: false,
            prompt: None,
            steps: statuses.iter().enumerate().map(|(i, s)| step(i, *s)).collect(),
        }
    }

    fn step(position: usize, status: StepStatus) -> codemark_core::engine::transfer::StepPlan {
        codemark_core::engine::transfer::StepPlan::stub(
            position,
            format!("src/f{position}.rs"),
            status,
        )
    }

    #[test]
    fn plans_with_nothing_to_transfer_are_blocked() {
        assert!(blocking_reason(&plan(&[])).unwrap().contains("no steps"));
        let none = plan(&[StepStatus::Unmatched, StepStatus::Unmatched]);
        assert!(blocking_reason(&none).unwrap().contains("none of its 2 steps"));
        assert!(blocking_reason(&plan(&[StepStatus::Matched, StepStatus::Unmatched])).is_none());
    }

    #[test]
    fn confirmation_lists_steps_that_will_be_left_out() {
        let p = plan(&[StepStatus::Matched, StepStatus::Unmatched]);
        let msg = confirmation_message(TransferKind::Copy, "tour", Some("tour-main"), &p);
        assert!(msg.starts_with("Copy \"tour\" to main (abcdef12) as \"tour-main\"?"), "{msg}");
        assert!(msg.contains("1 of 2 steps match."));
        assert!(msg.contains("✗ 2. src/f1.rs"));
    }

    #[test]
    fn relative_age_picks_the_largest_unit() {
        assert_eq!(relative_age(30), "0m ago");
        assert_eq!(relative_age(7200), "2h ago");
        assert_eq!(relative_age(86_400 * 3), "3d ago");
        assert_eq!(relative_age(86_400 * 400), "1y ago");
    }

    fn test_layout() -> (BrowserLayout, tempfile::TempDir) {
        use crate::event::{EventHandler, EventHandlerConfig};
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(&dir.path().join("test.db")).expect("open db");
        let handler = EventHandler::new(EventHandlerConfig::default()).expect("event handler");
        (BrowserLayout::new(db, handler), dir)
    }

    fn planned(result: Result<(TransferPlan, Option<String>), String>) -> TransferPlanned {
        TransferPlanned {
            kind: TransferKind::Move,
            collection_id: "c".into(),
            collection_name: "tour".into(),
            db_path: PathBuf::from("/tmp/db"),
            branch: "main".into(),
            result,
        }
    }

    #[test]
    fn a_failed_plan_notifies_instead_of_confirming() {
        let (mut layout, _dir) = test_layout();
        layout.transfer_planning = true;
        layout.on_transfer_planned(&planned(Err("cannot resolve 'main'".into())));
        assert!(!layout.has_active_dialog());
        assert!(!layout.transfer_planning, "the in-flight guard is released");
        let n = layout.pending_notification.take().expect("notification");
        assert!(!n.success);
        assert_eq!(n.message, "Can't move \"tour\" to main: cannot resolve 'main'");
    }

    #[test]
    fn a_plan_with_no_matching_steps_notifies_instead_of_confirming() {
        let (mut layout, _dir) = test_layout();
        let none = plan(&[StepStatus::Unmatched, StepStatus::Unmatched]);
        layout.on_transfer_planned(&planned(Ok((none, None))));
        assert!(!layout.has_active_dialog());
        let n = layout.pending_notification.take().expect("notification");
        assert!(!n.success);
        assert!(n.message.contains("none of its 2 steps match"), "{}", n.message);
    }

    #[test]
    fn a_partially_matching_plan_asks_for_confirmation() {
        let (mut layout, _dir) = test_layout();
        let partial = plan(&[StepStatus::Matched, StepStatus::Unmatched]);
        layout.on_transfer_planned(&planned(Ok((partial, None))));
        let dialog = layout.dialog.as_ref().expect("confirmation dialog");
        assert_eq!(dialog.title, "Move Collection");
        assert!(dialog.message.contains("1 will be left out"));
        assert!(matches!(
            &dialog.action,
            DialogAction::ApplyTransfer { mode: TransferMode::Move, .. }
        ));
        assert!(layout.pending_notification.is_none());
    }

    #[test]
    fn the_picker_is_modal_and_esc_closes_it() {
        let (mut layout, _dir) = test_layout();
        layout.branch_picker = Some(picker());
        assert!(layout.has_active_dialog());
        assert!(layout.handle_event(&Event::Paste("dev".into())));
        assert_eq!(layout.branch_picker.as_ref().unwrap().filter(), "dev");
        assert!(layout.handle_event(&Event::Key(key(KeyCode::Esc))));
        assert!(!layout.has_active_dialog());
    }

    #[test]
    fn a_plan_finishing_under_another_dialog_waits_for_it_to_close() {
        let (mut layout, _dir) = test_layout();
        layout.request_confirmation(ConfirmDialog::new(
            "Delete Collection",
            "Delete?",
            DialogAction::DeleteCollection { id: "x".into(), repo_root: None },
        ));
        let partial = plan(&[StepStatus::Matched]);
        layout.on_transfer_planned(&planned(Ok((partial, None))));
        assert_eq!(layout.dialog.as_ref().unwrap().title, "Delete Collection");
        // Cancel the delete; the transfer confirmation takes its place.
        layout.handle_event(&Event::Key(key(KeyCode::Esc)));
        assert_eq!(layout.dialog.as_ref().unwrap().title, "Move Collection");
    }
}
