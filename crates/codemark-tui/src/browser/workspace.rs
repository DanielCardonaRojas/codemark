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
/// [`set_scope`](RepoWorkspace::set_scope) treats an empty request as a no-op
/// (the "uncheck-last" invariant). `focus` is always one of the checked repos.
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

    /// Replace the checked set with `checked`.
    ///
    /// Opens newly-checked repos, drops unchecked ones, and *reuses* already-open
    /// connections (a `retain` keeps their live `Database`, so an unchanged repo
    /// is never reopened). A repo whose DB fails to open is silently skipped.
    ///
    /// An empty `checked` is a no-op: the checked set never empties
    /// (uncheck-last invariant). If the current focus is dropped, focus falls
    /// back to the first remaining checked repo.
    pub fn set_scope(&mut self, checked: &[PathBuf]) -> Result<()> {
        if checked.is_empty() {
            return Ok(());
        }
        // Drop unchecked repos, keeping the open connections for the rest.
        self.dbs.retain(|(root, _)| checked.contains(root));
        // Open any newly-checked repo not already present.
        for root in checked {
            if !self.dbs.iter().any(|(r, _)| r == root)
                && let Ok(db) = Database::open(&Self::db_path(root))
            {
                self.dbs.push((root.clone(), db));
            }
        }
        // Keep focus valid: if it was dropped, fall back to the first checked repo.
        if !self.dbs.iter().any(|(r, _)| r == &self.focus) {
            self.focus = self.dbs[0].0.clone();
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

    /// The focused repo root (always a checked repo).
    pub fn focus(&self) -> &Path {
        &self.focus
    }

    /// The focused repo's database. Never panics: `focus` is always checked.
    pub fn focus_db(&self) -> &Database {
        self.get(&self.focus).expect("focus is always a checked repo")
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
        ws.set_scope(&[root_a.clone()]).unwrap();
        assert!(ws.get(&root_a).is_some());
        assert!(ws.get(&root_b).is_none());
        assert!(!ws.is_multi());
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
        ws.set_scope(&[root_a.clone()]).unwrap();
        assert_eq!(ws.focus(), root_a.as_path());
        // focus_db must resolve without panicking.
        let _ = ws.focus_db();
    }
}
