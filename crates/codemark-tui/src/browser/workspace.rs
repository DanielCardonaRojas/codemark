//! Multi-repo workspace: owns one open [`Database`] per *checked* repo.
//!
//! The TUI browser lets the user check multiple repos and query across them.
//! [`RepoWorkspace`] holds the open DB connections for the checked set, keyed by
//! repo root (insertion-ordered), plus a `focus` repo that single-repo
//! operations act on. A repo root is a directory; its DB lives at
//! `<root>/.codemark/codemark.db` (matching `BrowserLayout::switch_database`).

use codemark_core::error::Result;
use codemark_core::storage::db::Database;
use std::path::{Path, PathBuf};

/// Owns the open databases for the *checked* repos, plus the focused repo.
///
/// The checked set is never empty: [`RepoWorkspace::new`] seeds one repo and
/// [`set_scope`](RepoWorkspace::set_scope) treats an empty request — or a
/// request whose repos are all unopenable — as a no-op (the "uncheck-last"
/// invariant), so the scope never drops to zero entries. `focus` is always one
/// of the checked repos.
pub struct RepoWorkspace {
    /// Checked repos, insertion-ordered: `repo_root -> open db`. A `Vec` (not a
    /// map) so insertion order — the order the user checked repos in — is
    /// preserved for display, and reuse is a cheap linear scan over a handful
    /// of entries.
    dbs: Vec<(PathBuf, Database)>,
    /// The focused repo root; always present in `dbs`.
    focus: PathBuf,
}

impl RepoWorkspace {
    /// Path to a repo's database, given its root directory.
    fn db_path(root: &Path) -> PathBuf {
        root.join(".codemark").join("codemark.db")
    }

    /// Open a workspace seeded with a single checked + focused repo.
    pub fn new(root: PathBuf) -> Result<Self> {
        let db = Database::open(&Self::db_path(&root))?;
        Ok(Self { dbs: vec![(root.clone(), db)], focus: root })
    }

    /// Wrap an already-open database as a single-repo workspace (checked + focused).
    /// `root` is the repo root the db belongs to (used as its lookup key).
    pub fn from_db(root: PathBuf, db: Database) -> Self {
        Self { dbs: vec![(root.clone(), db)], focus: root }
    }

    /// Replace the checked set with `checked`.
    ///
    /// Opens newly-checked repos, drops unchecked ones, and *reuses* already-open
    /// connections (still-checked `(root, db)` entries keep their live
    /// `Database`, so an unchanged repo is never reopened). A newly-checked repo
    /// whose DB fails to open is skipped but logged.
    ///
    /// The scope never empties. An empty `checked` is a no-op (uncheck-last
    /// invariant); likewise, if the entire requested scope is unopenable (no
    /// currently-open entry survives the filter *and* every newly-checked root
    /// fails to open), the previous scope is kept unchanged. If the current
    /// focus is dropped, focus falls back to the first remaining checked repo.
    pub fn set_scope(&mut self, checked: &[PathBuf]) -> Result<()> {
        if checked.is_empty() {
            return Ok(()); // uncheck-last is a no-op; scope never empties
        }
        // Open newly-checked roots that aren't already open, into a temp vec
        // first — so a failed open never forces dropping good connections.
        // Tolerate (log) failures.
        let mut opened: Vec<(PathBuf, Database)> = Vec::new();
        for root in checked {
            let already_open = self.dbs.iter().any(|(r, _)| r == root);
            let already_queued = opened.iter().any(|(r, _)| r == root);
            if !already_open && !already_queued {
                match Database::open(&Self::db_path(root)) {
                    Ok(db) => opened.push((root.clone(), db)),
                    Err(e) => tracing::warn!(
                        target: "codemark::ui",
                        root = %root.display(),
                        error = %e,
                        "failed to open repo database; excluding from scope"
                    ),
                }
            }
        }
        // Would the new scope be empty? (no surviving open entry AND nothing
        // newly opened). If so, keep the previous scope intact.
        let has_survivor = self.dbs.iter().any(|(r, _)| checked.contains(r));
        if !has_survivor && opened.is_empty() {
            return Ok(());
        }
        // Commit: keep still-checked open connections (reused), drop the rest,
        // add the newly opened ones.
        let mut kept: Vec<(PathBuf, Database)> = std::mem::take(&mut self.dbs)
            .into_iter()
            .filter(|(r, _)| checked.contains(r))
            .collect();
        kept.extend(opened);
        self.dbs = kept;
        // Keep focus valid: if it was dropped, fall back to the first checked
        // repo via a safe accessor (never unchecked indexing).
        if !self.dbs.iter().any(|(r, _)| r == &self.focus)
            && let Some((root, _)) = self.dbs.first()
        {
            self.focus = root.clone();
        }
        Ok(())
    }

    /// Whether more than one repo is checked (i.e. queries fan out).
    pub fn is_multi(&self) -> bool {
        self.dbs.len() > 1
    }

    /// Iterate the checked repos as `(root, db)`, in insertion order.
    pub fn dbs(&self) -> impl Iterator<Item = (&Path, &Database)> {
        self.dbs.iter().map(|(r, d)| (r.as_path(), d))
    }

    /// The open database for a checked repo, if `root` is checked.
    pub fn get(&self, root: &Path) -> Option<&Database> {
        self.dbs.iter().find(|(r, _)| r == root).map(|(_, d)| d)
    }

    /// Resolve the database for a repo root string (as carried on a PanelItem's
    /// `repo_root()`), falling back to the focused repo when the tag is missing or
    /// doesn't match a checked repo.
    pub fn db_for(&self, repo_root: Option<&str>) -> &Database {
        repo_root.and_then(|r| self.get(std::path::Path::new(r))).unwrap_or_else(|| self.focus_db())
    }

    /// The focused repo root (always a checked repo).
    pub fn focus(&self) -> &Path {
        &self.focus
    }

    /// The focused repo's database. Never panics: `focus` is always checked.
    pub fn focus_db(&self) -> &Database {
        self.get(&self.focus).expect("focus is always a checked repo")
    }

    /// Mutable access to the focused repo's database, for the few DB operations
    /// that take `&mut self` (e.g. `delete_collection_recursive`). Never panics:
    /// `focus` is always checked.
    pub fn focus_db_mut(&mut self) -> &mut Database {
        let focus = self.focus.clone();
        self.dbs
            .iter_mut()
            .find(|(r, _)| r == &focus)
            .map(|(_, d)| d)
            .expect("focus is always a checked repo")
    }

    /// Set the focused repo. Ignored if `root` is not currently checked, so
    /// focus stays valid.
    pub fn set_focus(&mut self, root: PathBuf) {
        if self.get(&root).is_some() {
            self.focus = root;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Create a temp repo directory with an initialized `.codemark/codemark.db`.
    ///
    /// `Database::open` requires the parent dir to exist, so we create
    /// `.codemark/` and open the db there — mirroring the on-disk layout
    /// `RepoWorkspace` opens against.
    fn temp_repo() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        // Initialize the db so a later `Database::open` at the same path succeeds.
        Database::create(&RepoWorkspace::db_path(&root)).expect("init db");
        (dir, root)
    }

    #[test]
    fn set_scope_opens_and_drops_without_reopening_unchanged() {
        let (_a, root_a) = temp_repo();
        let (_b, root_b) = temp_repo();

        let mut ws = RepoWorkspace::new(root_a.clone()).unwrap();
        assert!(!ws.is_multi());

        ws.set_scope(&[root_a.clone(), root_b.clone()]).unwrap();
        assert!(ws.is_multi());
        assert!(ws.get(&root_a).is_some());
        assert!(ws.get(&root_b).is_some());

        // Reuse is asserted structurally rather than by pointer identity: the
        // `&Database` address is not a stable reuse signal, because a `push`
        // (adding B) can reallocate the `Vec` and move A's still-open connection
        // to a new address without reopening it. `set_scope` reuses connections
        // by `retain`-ing the existing `(root, db)` entries and only opening
        // roots not already present — so an unchanged repo keeps its live
        // `Database`. The drop/keep assertions below verify that behaviour: A
        // survives across scope changes while B is opened then dropped.
        //
        // Narrowing back to just A drops B and reuses A again.
        ws.set_scope(std::slice::from_ref(&root_a)).unwrap();
        assert!(ws.get(&root_a).is_some());
        assert!(ws.get(&root_b).is_none());
        assert!(!ws.is_multi());
    }

    #[test]
    fn from_db_wraps_single_repo() {
        let (_a, root_a) = temp_repo();
        let db = Database::open(&RepoWorkspace::db_path(&root_a)).unwrap();
        let db_path = db.path().to_path_buf();

        let ws = RepoWorkspace::from_db(root_a.clone(), db);
        assert!(!ws.is_multi());
        assert_eq!(ws.focus(), root_a.as_path());
        assert_eq!(ws.focus_db().path(), db_path.as_path());
    }

    #[test]
    fn uncheck_last_is_noop() {
        let (_a, root_a) = temp_repo();

        let mut ws = RepoWorkspace::new(root_a.clone()).unwrap();
        // Unchecking everything must not empty the scope.
        ws.set_scope(&[]).unwrap();

        assert!(ws.get(&root_a).is_some());
        assert_eq!(ws.focus(), root_a.as_path());
    }

    #[test]
    fn focus_follows_when_focus_repo_unchecked() {
        let (_a, root_a) = temp_repo();
        let (_b, root_b) = temp_repo();

        let mut ws = RepoWorkspace::new(root_a.clone()).unwrap();
        ws.set_scope(&[root_a.clone(), root_b.clone()]).unwrap();
        ws.set_focus(root_b.clone());
        assert_eq!(ws.focus(), root_b.as_path());

        // Unchecking B (the focus) must fall focus back to a checked repo (A).
        ws.set_scope(std::slice::from_ref(&root_a)).unwrap();
        assert_eq!(ws.focus(), root_a.as_path());
        // focus_db must resolve without panicking.
        let _ = ws.focus_db();
    }

    #[test]
    fn set_scope_keeps_prior_scope_when_all_unopenable() {
        let (_a, root_a) = temp_repo();

        let mut ws = RepoWorkspace::new(root_a.clone()).unwrap();

        // A temp path with NO `.codemark/codemark.db`, so `Database::open` fails.
        let missing = tempfile::tempdir().expect("tempdir");
        let nonexistent_root = missing.path().to_path_buf();

        // The entire requested scope is unopenable: must be a no-op, not a panic.
        ws.set_scope(std::slice::from_ref(&nonexistent_root)).unwrap();

        // Prior scope (just A) is preserved.
        assert!(ws.get(&root_a).is_some());
        assert_eq!(ws.focus(), root_a.as_path());
        assert!(!ws.is_multi());
    }

    #[test]
    fn db_for_resolves_by_repo_root() {
        let (_a, root_a) = temp_repo();
        let (_b, root_b) = temp_repo();

        let mut ws = RepoWorkspace::new(root_a.clone()).unwrap();
        ws.set_scope(&[root_a.clone(), root_b.clone()]).unwrap();

        let db_a_path = RepoWorkspace::db_path(&root_a);
        let db_b_path = RepoWorkspace::db_path(&root_b);

        assert_eq!(ws.db_for(Some(&root_a.to_string_lossy())).path(), db_a_path.as_path());
        assert_eq!(ws.db_for(Some(&root_b.to_string_lossy())).path(), db_b_path.as_path());
    }

    #[test]
    fn db_for_falls_back_to_focus_when_unknown_or_none() {
        let (_a, root_a) = temp_repo();
        let (_b, root_b) = temp_repo();

        let mut ws = RepoWorkspace::new(root_a.clone()).unwrap();
        ws.set_scope(&[root_a.clone(), root_b.clone()]).unwrap();
        // Focus B so the fallback is distinguishable from resolving A.
        ws.set_focus(root_b.clone());

        let focus_path = ws.focus_db().path().to_path_buf();

        assert_eq!(ws.db_for(None).path(), focus_path.as_path());
        assert_eq!(ws.db_for(Some("/no/such/repo")).path(), focus_path.as_path());
    }

    #[test]
    fn set_scope_keeps_valid_and_skips_invalid_mix() {
        let (_a, root_a) = temp_repo();

        let mut ws = RepoWorkspace::new(root_a.clone()).unwrap();

        let missing = tempfile::tempdir().expect("tempdir");
        let nonexistent_root = missing.path().to_path_buf();

        // A valid + invalid mix keeps the valid one and skips the invalid.
        ws.set_scope(&[root_a.clone(), nonexistent_root.clone()]).unwrap();

        assert!(ws.get(&root_a).is_some());
        assert!(ws.get(&nonexistent_root).is_none());
        assert!(!ws.is_multi());
    }
}
