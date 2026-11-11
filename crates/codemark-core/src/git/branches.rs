//! Listing the branches a collection can be copied or moved to.
//!
//! Used by the TUI's branch picker. Branches come from the git object store
//! (git2, no subprocess). Common integration branches are pinned to the front
//! so the usual target (`main`) is always one keypress away.

use std::collections::HashSet;
use std::path::Path;

use git2::{BranchType, Repository};

use crate::error::{Error, Result};

/// Branch names treated as common integration targets, in pin order.
pub const COMMON_BRANCHES: &[&str] = &["main", "master", "develop", "dev", "trunk"];

/// A branch that a collection can be transferred to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRef {
    /// The branch name the forks record (`feat-x`, never `origin/feat-x`).
    pub name: String,
    /// The full ref to read: `refs/heads/<name>` for a local branch,
    /// `refs/remotes/<remote>/<name>` for a remote-tracking branch with no local
    /// counterpart. Fully qualified so a tag with the same name can't shadow it.
    pub rev: String,
    /// Only a remote-tracking ref exists for this branch.
    pub is_remote: bool,
    /// The branch checked out in the repository.
    pub is_current: bool,
    /// The repository's default branch (`origin/HEAD`, else `main`/`master`).
    pub is_default: bool,
    /// Pinned to the front of the list (default, common, or current branch).
    pub pinned: bool,
    /// Commit time of the branch tip (seconds since the epoch).
    pub last_commit: i64,
}

/// List the branches of the repository at `repo_root`.
///
/// Local branches come first; remote-tracking branches are included only when
/// no local branch has the same name, and `*/HEAD` aliases are skipped. The
/// result is ordered: default branch, other [`COMMON_BRANCHES`] that exist, the
/// current branch, then everything else by most recent commit.
pub fn list_branches(repo_root: &Path) -> Result<Vec<BranchRef>> {
    let repo = Repository::discover(repo_root)
        .map_err(|e| Error::Git(format!("cannot open repo: {e}")))?;
    let current =
        repo.head().ok().filter(|h| h.is_branch()).and_then(|h| h.shorthand().map(str::to_string));
    let default = default_branch(&repo);

    let mut branches: Vec<BranchRef> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let iter = repo.branches(None).map_err(|e| Error::Git(format!("cannot list branches: {e}")))?;
    let mut remotes = Vec::new();
    for entry in iter.flatten() {
        let (branch, kind) = entry;
        let Ok(Some(full)) = branch.name() else { continue };
        let full = full.to_string();
        let last_commit =
            branch.get().peel_to_commit().map(|c| c.time().seconds()).unwrap_or_default();
        match kind {
            BranchType::Local => {
                seen.insert(full.clone());
                branches.push(BranchRef {
                    rev: format!("refs/heads/{full}"),
                    name: full,
                    is_remote: false,
                    is_current: false,
                    is_default: false,
                    pinned: false,
                    last_commit,
                });
            }
            BranchType::Remote => remotes.push((full, last_commit)),
        }
    }
    // When several remotes carry the same branch, keep `origin`'s: it's the one
    // `origin/HEAD` (and so the default-branch marker) refers to.
    remotes.sort_by_key(|(full, _)| !full.starts_with("origin/"));
    for (full, last_commit) in remotes {
        // `origin/feat-x` → `feat-x`; skip the `origin/HEAD` alias.
        let Some((_, short)) = full.split_once('/') else { continue };
        if short == "HEAD" || !seen.insert(short.to_string()) {
            continue;
        }
        branches.push(BranchRef {
            name: short.to_string(),
            rev: format!("refs/remotes/{full}"),
            is_remote: true,
            is_current: false,
            is_default: false,
            pinned: false,
            last_commit,
        });
    }

    for b in &mut branches {
        b.is_current = current.as_deref() == Some(b.name.as_str());
        b.is_default = default.as_deref() == Some(b.name.as_str());
        b.pinned = b.is_default || b.is_current || COMMON_BRANCHES.contains(&b.name.as_str());
    }
    branches.sort_by_key(|b| (pin_rank(b), std::cmp::Reverse(b.last_commit)));
    tracing::debug!(
        target: "codemark::git",
        repo = %repo_root.display(),
        count = branches.len(),
        default = ?default,
        current = ?current,
        "listed branches"
    );
    Ok(branches)
}

/// Sort key: default branch, then common branches in [`COMMON_BRANCHES`]
/// order, then the current branch, then everything else.
fn pin_rank(b: &BranchRef) -> usize {
    if b.is_default {
        return 0;
    }
    if let Some(i) = COMMON_BRANCHES.iter().position(|c| *c == b.name) {
        return 1 + i;
    }
    if b.is_current {
        return 1 + COMMON_BRANCHES.len();
    }
    2 + COMMON_BRANCHES.len()
}

/// The default branch: what `origin/HEAD` points at, else `main`, else `master`.
fn default_branch(repo: &Repository) -> Option<String> {
    if let Ok(r) = repo.find_reference("refs/remotes/origin/HEAD")
        && let Some(target) = r.symbolic_target()
        && let Some(name) = target.strip_prefix("refs/remotes/origin/")
    {
        return Some(name.to_string());
    }
    ["main", "master"]
        .into_iter()
        .find(|n| {
            repo.find_branch(n, BranchType::Local).is_ok()
                || repo.find_branch(&format!("origin/{n}"), BranchType::Remote).is_ok()
        })
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::Signature;

    fn commit(repo: &Repository, branch: &str, time: i64) {
        let sig = Signature::new("t", "t@e", &git2::Time::new(time, 0)).unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        let refname = format!("refs/heads/{branch}");
        repo.commit(Some(&refname), &sig, &sig, "c", &tree, &parents).unwrap();
    }

    fn names(branches: &[BranchRef]) -> Vec<&str> {
        branches.iter().map(|b| b.name.as_str()).collect()
    }

    #[test]
    fn pins_default_common_and_current_then_orders_by_recency() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        commit(&repo, "main", 100);
        commit(&repo, "older", 200);
        commit(&repo, "newer", 300);
        commit(&repo, "develop", 50);
        commit(&repo, "feature", 150);
        repo.set_head("refs/heads/feature").unwrap();

        let branches = list_branches(dir.path()).unwrap();
        assert_eq!(names(&branches), ["main", "develop", "feature", "newer", "older"]);
        let main = &branches[0];
        assert!(main.is_default && main.pinned && !main.is_current);
        assert!(branches[2].is_current && branches[2].pinned);
        assert!(!branches[3].pinned);
    }

    #[test]
    fn remote_only_branches_are_listed_by_short_name() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        commit(&repo, "main", 100);
        let head = repo.head().unwrap().target().unwrap();
        // A remote-tracking copy of main (deduped) and a remote-only branch.
        repo.reference("refs/remotes/origin/main", head, true, "t").unwrap();
        repo.reference("refs/remotes/origin/feat-x", head, true, "t").unwrap();
        repo.reference_symbolic("refs/remotes/origin/HEAD", "refs/remotes/origin/main", true, "t")
            .unwrap();

        let branches = list_branches(dir.path()).unwrap();
        assert_eq!(names(&branches), ["main", "feat-x"]);
        let feat = &branches[1];
        assert!(feat.is_remote);
        assert_eq!(feat.rev, "refs/remotes/origin/feat-x");
        assert_eq!(branches[0].rev, "refs/heads/main");
        assert!(!branches[0].is_remote);
    }

    #[test]
    fn default_branch_follows_origin_head() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        commit(&repo, "main", 100);
        commit(&repo, "release", 50);
        let head = repo.head().unwrap().target().unwrap();
        repo.reference("refs/remotes/origin/release", head, true, "t").unwrap();
        repo.reference_symbolic(
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/release",
            true,
            "t",
        )
        .unwrap();

        let branches = list_branches(dir.path()).unwrap();
        assert_eq!(branches[0].name, "release");
        assert!(branches[0].is_default);
        assert!(!branches[1].is_default);
    }

    #[test]
    fn origin_wins_when_remotes_share_a_branch_and_tags_cannot_shadow_it() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        commit(&repo, "main", 100);
        let first = repo.head().unwrap().target().unwrap();
        commit(&repo, "main", 200);
        let second = repo.head().unwrap().target().unwrap();
        // `aaa` sorts before `origin` but points somewhere else.
        repo.reference("refs/remotes/aaa/release", first, true, "t").unwrap();
        repo.reference("refs/remotes/origin/release", second, true, "t").unwrap();
        // A tag named like the branch must not win when the rev is resolved.
        repo.reference("refs/tags/main", first, true, "t").unwrap();

        let branches = list_branches(dir.path()).unwrap();
        let release = branches.iter().find(|b| b.name == "release").unwrap();
        assert_eq!(release.rev, "refs/remotes/origin/release");
        let main = branches.iter().find(|b| b.name == "main").unwrap();
        let resolved = repo.revparse_single(&main.rev).unwrap().peel_to_commit().unwrap();
        assert_eq!(resolved.id(), second);
    }
}
