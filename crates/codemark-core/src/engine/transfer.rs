//! Copying and moving collections, optionally onto another git branch.
//!
//! Knowledge follows the tour (ADR 0001, GitHub discussion #300). A copy or
//! move therefore **forks** every step: it creates a new bookmark row (with
//! `forked_from` pointing at the original) carrying the original's tags, notes
//! and comments, plus a single fresh resolution at the target commit. The new
//! tour's knowledge and history never leak into the original's.
//!
//! The work is split in two:
//! - [`plan_transfer`] is a dry run. It resolves every step against the target
//!   (a committed revision read from the git object store, or the working
//!   tree), follows file renames, and reports what matched.
//! - [`apply_transfer`] writes a plan in one transaction.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::engine::bookmark::{
    Annotation, Bookmark, BookmarkComment, BookmarkHealth, Collection, CollectionLink,
    CollectionTag, ResolutionMethod, Tag,
};
use crate::engine::resolution;
use crate::error::{Error, Result};
use crate::git::context as git_context;
use crate::parser::languages::ParseCache;
use crate::storage::db::Database;
use crate::vfs::{FileProvider, GitRevFileProvider, WorktreeFileProvider};

/// Where a transfer resolves its steps.
#[derive(Debug, Clone)]
pub enum TransferTarget {
    /// The working tree of `worktree_root` (used when no `--to` branch is
    /// given). That's the worktree the command runs in, which may differ from
    /// the main checkout the database lives in. `head` is the commit the forks
    /// are recorded against.
    WorkingTree { head: Option<String>, worktree_root: PathBuf },
    /// A committed revision, read without a checkout. `sha` is the resolved
    /// commit for `rev`.
    Revision { rev: String, sha: String },
}

impl TransferTarget {
    /// The commit forked steps are recorded against.
    pub fn commit(&self) -> Option<&str> {
        match self {
            TransferTarget::WorkingTree { head, .. } => head.as_deref(),
            TransferTarget::Revision { sha, .. } => Some(sha),
        }
    }
}

/// Options for planning a transfer.
#[derive(Debug, Clone)]
pub struct TransferOptions {
    /// The branch the forks will belong to.
    pub target_branch: String,
    pub target: TransferTarget,
    /// Accept relaxed, minimized and hash-fallback matches. Off by default: a
    /// loose query can land on unrelated code once the original is gone, and a
    /// false Active is worse than an honest unmatched step.
    pub allow_fuzzy: bool,
}

/// What to do with steps that don't match on the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OnUnmatched {
    /// Write nothing and report the unmatched steps.
    Fail,
    /// Leave unmatched steps out. A move never deletes their original rows.
    Drop,
}

impl std::str::FromStr for OnUnmatched {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "fail" => Ok(OnUnmatched::Fail),
            "drop" => Ok(OnUnmatched::Drop),
            other => {
                Err(Error::Input(format!("--on-unmatched must be fail or drop, got '{other}'")))
            }
        }
    }
}

/// How a planned transfer is applied.
#[derive(Debug, Clone)]
pub enum TransferMode {
    /// Clone into a new collection called `new_name`.
    Copy { new_name: String },
    /// Re-point the existing collection at the forks, in place.
    Move,
}

impl TransferMode {
    fn label(&self) -> &'static str {
        match self {
            TransferMode::Copy { .. } => "copy",
            TransferMode::Move => "move",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Matched,
    Unmatched,
}

/// The location a matched step resolved to on the target.
#[derive(Debug, Clone)]
struct ResolvedLocation {
    content_hash: String,
    byte_range: String,
    snapshot: String,
    breadcrumbs: Option<String>,
    /// The matched code isn't committed (working-tree transfers only).
    is_dirty: bool,
}

/// One step of a [`TransferPlan`].
#[derive(Debug, Clone, Serialize)]
pub struct StepPlan {
    pub position: usize,
    pub bookmark_id: String,
    pub query: String,
    /// The step's path in the source collection.
    pub file_path: String,
    /// The path on the target; differs from `file_path` when the file was renamed.
    pub target_file_path: String,
    pub renamed: bool,
    pub notes: Vec<String>,
    pub status: StepStatus,
    /// The tier that matched on the target (`failed` when nothing did).
    pub method: ResolutionMethod,
    /// Matched only through a relaxed, minimized or hash-fallback query.
    pub fuzzy: bool,
    /// The matched code differs from what the original bookmark recorded.
    pub hash_changed: bool,
    pub line_range: Option<String>,
    /// Why the step is unmatched.
    pub reason: Option<String>,
    #[serde(skip)]
    resolved: Option<ResolvedLocation>,
}

/// The result of [`plan_transfer`]: what each step resolves to on the target.
#[derive(Debug, Clone, Serialize)]
pub struct TransferPlan {
    pub collection: String,
    pub collection_id: String,
    pub source_branch: Option<String>,
    pub target_branch: String,
    pub target_commit: Option<String>,
    pub allow_fuzzy: bool,
    /// The collection's stored prompt, so an agent can rebuild unmatched steps.
    pub prompt: Option<String>,
    pub steps: Vec<StepPlan>,
}

impl TransferPlan {
    pub fn matched(&self) -> impl Iterator<Item = &StepPlan> {
        self.steps.iter().filter(|s| s.status == StepStatus::Matched)
    }

    pub fn unmatched(&self) -> impl Iterator<Item = &StepPlan> {
        self.steps.iter().filter(|s| s.status == StepStatus::Unmatched)
    }

    pub fn has_unmatched(&self) -> bool {
        self.unmatched().next().is_some()
    }
}

/// A step that was forked by [`apply_transfer`].
#[derive(Debug, Clone, Serialize)]
pub struct ForkedStep {
    pub original_id: String,
    pub fork_id: String,
}

/// The result of [`apply_transfer`].
#[derive(Debug, Clone, Serialize)]
pub struct TransferOutcome {
    pub mode: &'static str,
    /// The collection that now holds the forks (the new one for a copy).
    pub collection: String,
    pub collection_id: String,
    pub target_branch: String,
    pub forked: Vec<ForkedStep>,
    /// Bookmark ids of unmatched steps left out of the collection. Their rows
    /// are never deleted.
    pub dropped: Vec<String>,
    /// Move only: original rows deleted because nothing else referenced them.
    pub deleted_originals: Vec<String>,
    /// Move only: original rows kept because another collection still uses them.
    pub kept_originals: Vec<String>,
}

/// The repository root a database belongs to (`<root>/.codemark/codemark.db`),
/// always absolute: a relative `--db` (e.g. `.codemark/codemark.db`) yields a
/// relative or empty root, which is resolved against the current directory.
pub fn repo_root_for(db: &Database) -> PathBuf {
    let root = absolute_db_root(db);
    std::fs::canonicalize(&root).unwrap_or(root)
}

/// The database's project root made absolute but *not* canonicalized: this is
/// the spelling `resolve_bookmark_file_path` produces for files that don't
/// exist on disk (e.g. through a symlinked `--db` path).
fn absolute_db_root(db: &Database) -> PathBuf {
    let root = crate::storage::workspace::Workspace::project_root(db);
    if root.is_absolute() {
        root
    } else {
        std::env::current_dir().map(|cwd| cwd.join(&root)).unwrap_or(root)
    }
}

/// Bookmark ids of a collection's steps, in step order.
fn ordered_member_ids(db: &Database, collection_id: &str) -> Result<Vec<String>> {
    let mut stmt = db.conn().prepare(
        "SELECT bookmark_id FROM collection_bookmarks WHERE collection_id = ?1
         ORDER BY position, added_at",
    )?;
    let ids = stmt.query_map([collection_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

/// The commit a bookmark was last seen at, used as the "from" side of rename
/// detection.
fn source_commit(db: &Database, bookmark: &Bookmark) -> Option<String> {
    bookmark
        .current_resolution_id
        .as_deref()
        .and_then(|id| db.get_resolution(id).ok().flatten())
        .and_then(|r| r.commit_hash)
        .or_else(|| bookmark.commit_hash.clone())
}

/// Resolve every step of `collection` against the target. Writes nothing.
pub async fn plan_transfer(
    db: &Database,
    collection: &Collection,
    options: &TransferOptions,
) -> Result<TransferPlan> {
    let repo_root = repo_root_for(db);
    let alias = absolute_db_root(db);
    let provider: Box<dyn FileProvider> = match &options.target {
        TransferTarget::WorkingTree { worktree_root, .. } => {
            Box::new(WorktreeFileProvider::new(&repo_root, worktree_root).with_alias(&alias))
        }
        TransferTarget::Revision { sha, .. } => {
            Box::new(GitRevFileProvider::new(&repo_root, sha).with_alias(&alias))
        }
    };
    // HEAD stands in for the working tree when looking for renames.
    let rename_target = options.target.commit().map(str::to_string);
    tracing::debug!(
        target: "codemark::git",
        collection = %collection.name,
        target_branch = %options.target_branch,
        target_commit = ?rename_target,
        "planning collection transfer"
    );

    let mut caches: HashMap<String, ParseCache> = HashMap::new();
    let mut steps = Vec::new();

    for (position, id) in ordered_member_ids(db, &collection.id)?.into_iter().enumerate() {
        let Some(bookmark) = db.get_bookmark(&id)? else { continue };
        let notes = bookmark.annotations.iter().filter_map(|a| a.notes.clone()).collect::<Vec<_>>();
        let mut step = StepPlan {
            position,
            bookmark_id: bookmark.id.clone(),
            query: bookmark.query.clone(),
            file_path: bookmark.file_path.clone(),
            target_file_path: bookmark.file_path.clone(),
            renamed: false,
            notes,
            status: StepStatus::Unmatched,
            method: ResolutionMethod::Failed,
            fuzzy: false,
            hash_changed: false,
            line_range: None,
            reason: None,
            resolved: None,
        };

        // Follow a rename when the file is gone at the target.
        let abs = git_context::resolve_bookmark_file_path(&bookmark.file_path, db.path())?;
        if !provider.exists(&abs, None).await
            && let (Some(from), Some(to)) = (source_commit(db, &bookmark), rename_target.as_deref())
            && let Some(new_path) =
                git_context::find_renamed_path(&repo_root, &from, to, &bookmark.file_path)
        {
            step.target_file_path = new_path;
            step.renamed = true;
        }

        let lang = match bookmark.grammar() {
            Ok(lang) => lang,
            Err(e) => {
                step.reason = Some(format!("grammar unavailable: {e}"));
                steps.push(step);
                continue;
            }
        };
        // Key by name *and* grammar identity: a dynamic grammar can share a name
        // with the built-in it overrides, and each needs its own parser.
        let cache_key = format!("{}/{}", lang, lang.identity().as_str());
        let cache = match caches.entry(cache_key) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => e.insert(ParseCache::new(lang)?),
        };
        let ts_lang = cache.language().clone();

        let mut probe = bookmark.clone();
        probe.file_path = step.target_file_path.clone();
        match resolution::resolve(&probe, cache, &ts_lang, db.path(), provider.as_ref()).await {
            Err(e) => {
                step.reason = Some(format!("file not readable on target: {e}"));
            }
            Ok(result) => {
                step.method = result.method;
                step.hash_changed = !result.hash_matches;
                let fuzzy = result.method != ResolutionMethod::Exact;
                step.fuzzy = fuzzy && result.method != ResolutionMethod::Failed;
                if result.method == ResolutionMethod::Failed {
                    step.reason = Some("query did not match on target".to_string());
                } else if fuzzy && !options.allow_fuzzy {
                    step.reason = Some(format!(
                        "only a {} match; pass --allow-fuzzy to accept it",
                        result.method
                    ));
                    step.line_range =
                        Some(format!("{}-{}", result.start_line + 1, result.end_line + 1));
                } else {
                    step.status = StepStatus::Matched;
                    step.line_range =
                        Some(format!("{}-{}", result.start_line + 1, result.end_line + 1));
                    step.resolved = Some(ResolvedLocation {
                        content_hash: result.content_hash.clone(),
                        byte_range: format!("{}-{}", result.byte_range.0, result.byte_range.1),
                        snapshot: result.matched_text.clone(),
                        breadcrumbs: if result.breadcrumbs.is_empty() {
                            None
                        } else {
                            serde_json::to_string(&result.breadcrumbs).ok()
                        },
                        // Committed revisions are clean by definition; working-tree
                        // code may not be in HEAD. Fail closed on git errors.
                        is_dirty: match &options.target {
                            TransferTarget::Revision { .. } => false,
                            TransferTarget::WorkingTree { worktree_root, .. } => {
                                !git_context::is_file_clean(worktree_root, &step.target_file_path)
                                    .unwrap_or(false)
                            }
                        },
                    });
                }
            }
        }
        tracing::debug!(
            target: "codemark::git",
            bookmark = %step.bookmark_id,
            status = ?step.status,
            method = %step.method,
            renamed = step.renamed,
            "planned transfer step"
        );
        steps.push(step);
    }

    Ok(TransferPlan {
        collection: collection.name.clone(),
        collection_id: collection.id.clone(),
        source_branch: collection.created_branch.clone(),
        target_branch: options.target_branch.clone(),
        target_commit: options.target.commit().map(str::to_string),
        allow_fuzzy: options.allow_fuzzy,
        prompt: collection.prompt.clone(),
        steps,
    })
}

/// Fork one matched step: a new row owned by the destination tour, carrying the
/// original's tags, notes and comments and exactly one resolution.
fn fork_step(
    db: &Database,
    original: &Bookmark,
    step: &StepPlan,
    plan: &TransferPlan,
    now: &str,
) -> Result<String> {
    let loc = step.resolved.as_ref().ok_or_else(|| {
        Error::Operation(format!("step {} has no resolved location", step.bookmark_id))
    })?;
    let health = if step.fuzzy { BookmarkHealth::Drifted } else { BookmarkHealth::Active };

    let mut fork = original.clone();
    fork.id = uuid::Uuid::new_v4().to_string();
    fork.file_path = step.target_file_path.clone();
    fork.content_hash = Some(loc.content_hash.clone());
    fork.commit_hash = plan.target_commit.clone();
    fork.health = health;
    fork.resolution_method = Some(step.method);
    fork.created_at = now.to_string();
    fork.current_resolution_id = None;
    fork.forked_from = Some(original.id.clone());
    fork.branch = Some(plan.target_branch.clone());

    let fork_id = db.insert_bookmark(&fork)?;
    if fork_id != fork.id {
        return Err(Error::Operation(format!(
            "fork of {} collided with existing bookmark {fork_id}",
            original.id
        )));
    }

    // `insert_bookmark` records the single initial resolution; fill in the
    // location detail so the fork displays without a re-heal.
    db.conn().execute(
        "UPDATE resolutions SET byte_range = ?1, line_range = ?2, snapshot = ?3, breadcrumbs = ?4,
         is_dirty = ?5 WHERE id = (SELECT current_resolution_id FROM bookmarks WHERE id = ?6)",
        rusqlite::params![
            loc.byte_range,
            step.line_range,
            loc.snapshot,
            loc.breadcrumbs,
            loc.is_dirty,
            fork_id
        ],
    )?;

    let tags: Vec<Tag> = original
        .tags
        .iter()
        .map(|t| Tag {
            bookmark_id: fork_id.clone(),
            tag: t.clone(),
            added_at: now.to_string(),
            added_by: original.created_by.clone(),
        })
        .collect();
    // Row-at-a-time: the batch helpers open their own transaction.
    for tag in &tags {
        db.insert_tag(tag)?;
    }

    let annotations: Vec<Annotation> = original
        .annotations
        .iter()
        .map(|a| Annotation {
            id: uuid::Uuid::new_v4().to_string(),
            bookmark_id: fork_id.clone(),
            ..a.clone()
        })
        .collect();
    for annotation in &annotations {
        db.insert_annotation(annotation)?;
    }

    // Comments thread through parent_id, so remap ids and insert parents first.
    let id_map: HashMap<String, String> = original
        .comments
        .iter()
        .map(|c| (c.id.clone(), uuid::Uuid::new_v4().to_string()))
        .collect();
    let mut comments: Vec<BookmarkComment> = original
        .comments
        .iter()
        .map(|c| BookmarkComment {
            id: id_map[&c.id].clone(),
            bookmark_id: fork_id.clone(),
            parent_id: c.parent_id.as_ref().and_then(|p| id_map.get(p).cloned()),
            ..c.clone()
        })
        .collect();
    comments.sort_by_key(|c| c.parent_id.is_some());
    for comment in &comments {
        db.insert_comment(comment)?;
    }

    Ok(fork_id)
}

/// Apply a plan. Runs in one transaction: either every step is forked and the
/// collection re-pointed, or nothing changes.
pub fn apply_transfer(
    db: &Database,
    plan: &TransferPlan,
    mode: &TransferMode,
    on_unmatched: OnUnmatched,
    created_by: Option<&str>,
) -> Result<TransferOutcome> {
    if on_unmatched == OnUnmatched::Fail && plan.has_unmatched() {
        let n = plan.unmatched().count();
        return Err(Error::Input(format!(
            "{n} step(s) don't match on '{}'; nothing was written (use --on-unmatched drop to leave them out)",
            plan.target_branch
        )));
    }
    let source = db
        .get_collection_by_id(&plan.collection_id)?
        .ok_or_else(|| Error::Input(format!("collection '{}' not found", plan.collection)))?;
    if let TransferMode::Copy { new_name } = mode
        && db.get_collection_by_name(new_name)?.is_some()
    {
        return Err(Error::Input(format!("a collection named '{new_name}' already exists")));
    }

    let conn = db.conn();
    let owns_txn = conn.is_autocommit();
    if owns_txn {
        conn.execute_batch("BEGIN")?;
    }
    let result = apply_in_txn(db, plan, mode, &source, created_by);
    if owns_txn {
        match &result {
            Ok(_) => conn.execute_batch("COMMIT")?,
            Err(_) => {
                let _ = conn.execute_batch("ROLLBACK");
            }
        }
    }
    result
}

fn apply_in_txn(
    db: &Database,
    plan: &TransferPlan,
    mode: &TransferMode,
    source: &Collection,
    created_by: Option<&str>,
) -> Result<TransferOutcome> {
    let now = chrono::Utc::now().to_rfc3339();

    let target = match mode {
        TransferMode::Copy { new_name } => {
            let copy = Collection {
                id: uuid::Uuid::new_v4().to_string(),
                name: new_name.clone(),
                created_at: now.clone(),
                created_by: created_by.map(str::to_string).or_else(|| source.created_by.clone()),
                created_branch: Some(plan.target_branch.clone()),
                // A copy is a new, unpublished collection. `repo_url` stays: it
                // identifies the repo (set from origin at creation), not a publish.
                published_at: None,
                published_commit_sha: None,
                status: None,
                health: None,
                health_computed_at: None,
                updated_at: None,
                imported_from_url: None,
                ..source.clone()
            };
            db.insert_collection(&copy)?;
            let tags: Vec<CollectionTag> = db
                .list_tags_for_collection(&source.id)?
                .into_iter()
                .map(|t| CollectionTag { collection_id: copy.id.clone(), ..t })
                .collect();
            for tag in &tags {
                db.insert_collection_tag(tag)?;
            }
            for link in db.list_links_for_collection(&source.id)? {
                db.insert_collection_link(&CollectionLink {
                    id: uuid::Uuid::new_v4().to_string(),
                    collection_id: copy.id.clone(),
                    ..link
                })?;
            }
            copy
        }
        TransferMode::Move => source.clone(),
    };

    let mut forked = Vec::new();
    for step in plan.matched() {
        let original = db
            .get_bookmark(&step.bookmark_id)?
            .ok_or_else(|| Error::Operation(format!("bookmark {} vanished", step.bookmark_id)))?;
        let fork_id = fork_step(db, &original, step, plan, &now)?;
        forked.push(ForkedStep { original_id: original.id, fork_id });
    }
    let dropped: Vec<String> = plan.unmatched().map(|s| s.bookmark_id.clone()).collect();

    let conn = db.conn();
    let mut deleted_originals = Vec::new();
    let mut kept_originals = Vec::new();
    if matches!(mode, TransferMode::Move) {
        // Replace the steps; unmatched originals simply leave the collection.
        conn.execute("DELETE FROM collection_bookmarks WHERE collection_id = ?1", [&target.id])?;
        conn.execute(
            "UPDATE collections SET created_branch = ?1 WHERE id = ?2",
            rusqlite::params![plan.target_branch, target.id],
        )?;
    }
    for (position, f) in forked.iter().enumerate() {
        conn.execute(
            "INSERT INTO collection_bookmarks (collection_id, bookmark_id, added_at, position)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![target.id, f.fork_id, now, position as i64],
        )?;
    }
    if matches!(mode, TransferMode::Move) {
        // Only matched originals are cleaned up, and only when no other tour
        // still uses them, so a delete never cascades into another collection.
        for f in &forked {
            let removed = conn.execute(
                "DELETE FROM bookmarks WHERE id = ?1
                 AND NOT EXISTS (SELECT 1 FROM collection_bookmarks WHERE bookmark_id = ?1)",
                [&f.original_id],
            )?;
            if removed > 0 {
                // vec0 tables have no FK cascade; drop the embedding so dead
                // rows don't crowd semantic search. The table may be absent
                // (embeddings never initialised), which is fine to ignore.
                let _ = conn.execute(
                    "DELETE FROM bookmark_embeddings WHERE bookmark_id = ?1",
                    [&f.original_id],
                );
                deleted_originals.push(f.original_id.clone());
            } else {
                kept_originals.push(f.original_id.clone());
            }
        }
    }
    db.recompute_collection_health(&target.id)?;

    tracing::debug!(
        target: "codemark::db",
        mode = mode.label(),
        collection = %target.name,
        forked = forked.len(),
        dropped = dropped.len(),
        deleted = deleted_originals.len(),
        "applied collection transfer"
    );
    Ok(TransferOutcome {
        mode: mode.label(),
        collection: target.name,
        collection_id: target.id,
        target_branch: plan.target_branch.clone(),
        forked,
        dropped,
        deleted_originals,
        kept_originals,
    })
}

/// Resolve `rev` (a branch name or any revision) to a commit SHA.
pub fn resolve_revision(repo_root: &Path, rev: &str) -> Result<String> {
    let repo = git2::Repository::discover(repo_root)
        .map_err(|e| Error::Git(format!("cannot open repo: {e}")))?;
    let commit = repo
        .revparse_single(rev)
        .and_then(|o| o.peel_to_commit())
        .map_err(|e| Error::Input(format!("cannot resolve '{rev}' to a commit: {e}")))?;
    Ok(commit.id().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::bookmark::Visibility;
    use crate::parser::languages::Language;
    use crate::vfs::LocalFileProvider;
    use git2::{IndexAddOption, Oid, Repository, Signature};
    use std::fs;

    const LIB: &str =
        "int alpha(int x) {\n  return x + 1;\n}\n\nint beta(int y) {\n  return y * 2;\n}\n";
    const LIB_NO_BETA: &str = "int alpha(int x) {\n  return x + 1;\n}\n";

    /// A temp repo with `.codemark/codemark.db`, ignored by git.
    struct Fixture {
        root: PathBuf,
        repo: Repository,
        db: Database,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("codemark_transfer_{name}_{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(root.join(".codemark")).unwrap();
            let root = fs::canonicalize(&root).unwrap();
            fs::write(root.join(".gitignore"), ".codemark/\n").unwrap();
            let repo = Repository::init(&root).unwrap();
            let db = Database::create(&root.join(".codemark").join("codemark.db")).unwrap();
            Fixture { root, repo, db }
        }

        /// Make the working tree exactly `files` and commit it onto `branch`.
        fn commit(&self, branch: &str, files: &[(&str, &str)], parent: Option<Oid>) -> Oid {
            for entry in fs::read_dir(&self.root).unwrap() {
                let path = entry.unwrap().path();
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                if name == ".git" || name == ".codemark" || name == ".gitignore" {
                    continue;
                }
                if path.is_dir() {
                    fs::remove_dir_all(&path).unwrap();
                } else {
                    fs::remove_file(&path).unwrap();
                }
            }
            for (rel, content) in files {
                let path = self.root.join(rel);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, content).unwrap();
            }
            let mut index = self.repo.index().unwrap();
            index.clear().unwrap();
            index.add_all(["*"], IndexAddOption::DEFAULT, None).unwrap();
            index.write().unwrap();
            let tree = self.repo.find_tree(index.write_tree().unwrap()).unwrap();
            let sig = Signature::now("Test", "test@example.com").unwrap();
            let parents: Vec<git2::Commit> =
                parent.map(|p| self.repo.find_commit(p).unwrap()).into_iter().collect();
            let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
            self.repo
                .commit(Some(&format!("refs/heads/{branch}")), &sig, &sig, "c", &tree, &parent_refs)
                .unwrap()
        }

        /// Bookmark the function `name` in `lib.dart` (as on disk now).
        async fn bookmark(&self, name: &str, commit: Oid, note: &str) -> String {
            let path = self.root.join("lib.dart");
            let source = fs::read_to_string(&path).unwrap();
            // Anchor on the name: in Dart a function's signature and body are
            // sibling nodes, so a range spanning both would select the file.
            let start = source.find(&format!("int {name}(")).unwrap() + 4;
            let end = start + name.len();
            let lang = Language::Dart;
            let mut cache = ParseCache::new(lang.clone()).unwrap();
            let ts = cache.language().clone();
            let (tree, _) = cache.get_or_parse(&path, &LocalFileProvider).await.unwrap();
            let generated = crate::query::generator::generate_query(
                tree,
                source.as_bytes(),
                (start, end),
                &ts,
                lang.profile(),
            )
            .unwrap();
            // Hash what resolution will match: the whole @target node.
            let matches =
                crate::query::matcher::run_query(&generated.query, tree, source.as_bytes(), &ts)
                    .unwrap();
            let text = matches[0].node_text.clone();
            let bm = Bookmark {
                id: uuid::Uuid::new_v4().to_string(),
                query: generated.query,
                language: "dart".to_string(),
                grammar_source: Some(lang.identity()),
                forked_from: None,
                branch: None,
                file_path: "lib.dart".to_string(),
                content_hash: Some(crate::engine::hash::content_hash(&text)),
                commit_hash: Some(commit.to_string()),
                health: BookmarkHealth::Active,
                resolution_method: Some(ResolutionMethod::Exact),
                last_resolved_at: None,
                stale_since: None,
                created_at: "2026-11-09T00:00:00Z".to_string(),
                created_by: Some("claude".to_string()),
                current_resolution_id: None,
                repo_id: None,
                tags: Vec::new(),
                annotations: Vec::new(),
                comments: Vec::new(),
            };
            let id = self.db.insert_bookmark(&bm).unwrap();
            self.db
                .insert_annotation(&Annotation {
                    id: uuid::Uuid::new_v4().to_string(),
                    bookmark_id: id.clone(),
                    added_at: "2026-11-09T00:00:00Z".to_string(),
                    added_by: Some("claude".to_string()),
                    notes: Some(note.to_string()),
                    context: None,
                    source: None,
                })
                .unwrap();
            id
        }

        fn collection(&self, name: &str, ids: &[String]) -> Collection {
            let col = Collection {
                id: uuid::Uuid::new_v4().to_string(),
                name: name.to_string(),
                description: Some("tour".to_string()),
                visibility: Visibility::Private,
                created_at: "2026-11-09T00:00:00Z".to_string(),
                created_by: Some("claude".to_string()),
                created_branch: Some("feature".to_string()),
                published_at: None,
                published_commit_sha: None,
                repo_url: None,
                repo_id: None,
                status: None,
                health: None,
                health_computed_at: None,
                updated_at: None,
                imported_from_url: None,
                prompt: Some("Explain the arithmetic helpers".to_string()),
            };
            self.db.insert_collection(&col).unwrap();
            self.db.add_to_collection(&col.id, ids).unwrap();
            col
        }

        fn options(&self, branch: &str, allow_fuzzy: bool) -> TransferOptions {
            let sha = resolve_revision(&self.root, branch).unwrap();
            TransferOptions {
                target_branch: branch.to_string(),
                target: TransferTarget::Revision { rev: branch.to_string(), sha },
                allow_fuzzy,
            }
        }

        fn count(&self, sql: &str, id: &str) -> i64 {
            self.db.conn().query_row(sql, [id], |r| r.get(0)).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    async fn setup(name: &str) -> (Fixture, Collection, Oid, [String; 2]) {
        let fx = Fixture::new(name);
        let feature = fx.commit("feature", &[("lib.dart", LIB)], None);
        let alpha = fx.bookmark("alpha", feature, "alpha adds one").await;
        let beta = fx.bookmark("beta", feature, "beta doubles").await;
        let col = fx.collection("arith", &[alpha.clone(), beta.clone()]);
        (fx, col, feature, [alpha, beta])
    }

    #[tokio::test]
    async fn copy_to_identical_branch_forks_every_step() {
        let (fx, col, feature, [alpha, beta]) = setup("identical").await;
        fx.commit("main", &[("lib.dart", LIB)], Some(feature));

        let plan = plan_transfer(&fx.db, &col, &fx.options("main", false)).await.unwrap();
        assert!(!plan.has_unmatched(), "{:?}", plan.steps);
        assert!(
            plan.steps.iter().all(|s| s.method == ResolutionMethod::Exact && !s.hash_changed),
            "{:#?}",
            plan.steps
        );
        assert_eq!(plan.prompt.as_deref(), Some("Explain the arithmetic helpers"));

        let out = apply_transfer(
            &fx.db,
            &plan,
            &TransferMode::Copy { new_name: "arith-main".into() },
            OnUnmatched::Fail,
            None,
        )
        .unwrap();
        assert_eq!(out.forked.len(), 2);
        let copy = fx.db.get_collection_by_name("arith-main").unwrap().unwrap();
        assert_eq!(copy.created_branch.as_deref(), Some("main"));
        assert_eq!(copy.prompt, col.prompt);
        assert_eq!(ordered_member_ids(&fx.db, &copy.id).unwrap().len(), 2);

        for (f, original) in out.forked.iter().zip([&alpha, &beta]) {
            let fork = fx.db.get_bookmark(&f.fork_id).unwrap().unwrap();
            assert_eq!(fork.forked_from.as_deref(), Some(original.as_str()));
            assert_eq!(fork.branch.as_deref(), Some("main"));
            assert_eq!(fork.health, BookmarkHealth::Active);
            assert_eq!(fork.annotations.len(), 1, "notes are copied onto the fork");
            assert_eq!(
                fx.count("SELECT COUNT(*) FROM resolutions WHERE bookmark_id = ?1", &f.fork_id),
                1,
                "a fork starts with exactly one resolution"
            );
        }
        // The original collection is untouched.
        assert_eq!(ordered_member_ids(&fx.db, &col.id).unwrap(), vec![alpha, beta]);
    }

    /// Without `--to`, a transfer reads the *invoking* worktree's files (the
    /// database belongs to the main checkout), and records uncommitted code as
    /// dirty rather than as committed at HEAD.
    #[tokio::test]
    async fn working_tree_transfer_reads_the_linked_worktree_and_flags_dirty_code() {
        let (fx, col, _feature, _) = setup("worktree").await;
        fx.repo.set_head("refs/heads/feature").unwrap();
        let wt_path = fx
            .root
            .with_file_name(format!("{}-wt", fx.root.file_name().unwrap().to_string_lossy()));
        let wt = fx.repo.worktree("wt", &wt_path, None).unwrap();
        let wt_root = fs::canonicalize(wt.path()).unwrap();
        // Uncommitted edit in the linked worktree only: beta is gone, alpha changed.
        fs::write(
            wt_root.join("lib.dart"),
            LIB_NO_BETA.replace("int alpha(int x)", "int alpha(int x, int k)"),
        )
        .unwrap();

        let options = TransferOptions {
            target_branch: "wt".to_string(),
            target: TransferTarget::WorkingTree { head: None, worktree_root: wt_root.clone() },
            allow_fuzzy: false,
        };
        let plan = plan_transfer(&fx.db, &col, &options).await.unwrap();
        assert_eq!(plan.steps[0].status, StepStatus::Matched);
        assert!(plan.steps[0].hash_changed, "read the worktree's edited alpha");
        assert_eq!(plan.steps[1].status, StepStatus::Unmatched, "beta is gone in the worktree");

        let out =
            apply_transfer(&fx.db, &plan, &TransferMode::Move, OnUnmatched::Drop, None).unwrap();
        let dirty: bool = fx.db.conn().query_row(
            "SELECT r.is_dirty FROM bookmarks b JOIN resolutions r ON r.id = b.current_resolution_id
             WHERE b.id = ?1",
            [&out.forked[0].fork_id],
            |r| r.get(0),
        ).unwrap();
        assert!(dirty, "uncommitted code must not look committed");
        let _ = fs::remove_dir_all(&wt_path);
    }

    #[tokio::test]
    async fn renamed_file_is_followed() {
        let (fx, col, feature, _) = setup("renamed").await;
        fx.commit("main", &[("src/math.dart", LIB)], Some(feature));

        let plan = plan_transfer(&fx.db, &col, &fx.options("main", false)).await.unwrap();
        assert!(!plan.has_unmatched(), "{:?}", plan.steps);
        assert!(plan.steps.iter().all(|s| s.renamed && s.target_file_path == "src/math.dart"));
    }

    #[tokio::test]
    async fn unmatched_step_fails_or_drops_without_deleting_its_original() {
        let (fx, col, feature, [alpha, beta]) = setup("unmatched").await;
        fx.commit("main", &[("lib.dart", LIB_NO_BETA)], Some(feature));

        let plan = plan_transfer(&fx.db, &col, &fx.options("main", false)).await.unwrap();
        let unmatched: Vec<_> = plan.unmatched().map(|s| s.bookmark_id.clone()).collect();
        assert_eq!(unmatched, vec![beta.clone()]);

        let err = apply_transfer(&fx.db, &plan, &TransferMode::Move, OnUnmatched::Fail, None);
        assert!(err.is_err());
        assert_eq!(ordered_member_ids(&fx.db, &col.id).unwrap(), vec![alpha.clone(), beta.clone()]);

        let out =
            apply_transfer(&fx.db, &plan, &TransferMode::Move, OnUnmatched::Drop, None).unwrap();
        assert_eq!(out.dropped, vec![beta.clone()]);
        assert_eq!(out.deleted_originals, vec![alpha.clone()]);
        let members = ordered_member_ids(&fx.db, &col.id).unwrap();
        assert_eq!(members, vec![out.forked[0].fork_id.clone()]);
        assert!(fx.db.get_bookmark(&beta).unwrap().is_some(), "unmatched original is kept");
        assert!(fx.db.get_bookmark(&alpha).unwrap().is_none(), "matched orphan is cleaned up");
        let moved = fx.db.get_collection_by_id(&col.id).unwrap().unwrap();
        assert_eq!(moved.created_branch.as_deref(), Some("main"));
    }

    #[tokio::test]
    async fn fuzzy_matches_need_allow_fuzzy() {
        let (fx, col, feature, _) = setup("fuzzy").await;
        // `beta` is renamed: its exact query can't match, and any looser match
        // lands on other code, which is why fuzzy matches are opt-in.
        fx.commit("main", &[("lib.dart", &LIB.replace("beta", "gamma"))], Some(feature));

        let strict = plan_transfer(&fx.db, &col, &fx.options("main", false)).await.unwrap();
        let beta = &strict.steps[1];
        assert_ne!(beta.method, ResolutionMethod::Exact);
        assert_eq!(beta.status, StepStatus::Unmatched);
        assert!(beta.reason.is_some());

        let loose = plan_transfer(&fx.db, &col, &fx.options("main", true)).await.unwrap();
        if loose.steps[1].method != ResolutionMethod::Failed {
            assert!(loose.steps[1].fuzzy);
            assert_eq!(loose.steps[1].status, StepStatus::Matched);
            let out = apply_transfer(&fx.db, &loose, &TransferMode::Move, OnUnmatched::Fail, None)
                .unwrap();
            let fork = fx.db.get_bookmark(&out.forked[1].fork_id).unwrap().unwrap();
            assert_eq!(fork.health, BookmarkHealth::Drifted, "fuzzy forks are not marked Active");
        }
    }

    #[tokio::test]
    async fn repeated_copies_are_independent() {
        let (fx, col, feature, [alpha, _]) = setup("independent").await;
        fx.commit("main", &[("lib.dart", LIB)], Some(feature));
        let plan = plan_transfer(&fx.db, &col, &fx.options("main", false)).await.unwrap();

        let a = apply_transfer(
            &fx.db,
            &plan,
            &TransferMode::Copy { new_name: "a".into() },
            OnUnmatched::Fail,
            None,
        )
        .unwrap();
        let b = apply_transfer(
            &fx.db,
            &plan,
            &TransferMode::Copy { new_name: "b".into() },
            OnUnmatched::Fail,
            None,
        )
        .unwrap();
        assert_ne!(a.forked[0].fork_id, b.forked[0].fork_id);

        // A note on one clone stays on that clone.
        fx.db
            .insert_annotation(&Annotation {
                id: uuid::Uuid::new_v4().to_string(),
                bookmark_id: a.forked[0].fork_id.clone(),
                added_at: "2026-11-09T00:00:00Z".to_string(),
                added_by: None,
                notes: Some("only in a".to_string()),
                context: None,
                source: None,
            })
            .unwrap();
        let count_notes = |id: &str| {
            fx.count("SELECT COUNT(*) FROM bookmark_annotations WHERE bookmark_id = ?1", id)
        };
        assert_eq!(count_notes(&a.forked[0].fork_id), 2);
        assert_eq!(count_notes(&b.forked[0].fork_id), 1);
        assert_eq!(count_notes(&alpha), 1);

        // A name that already exists is refused.
        assert!(
            apply_transfer(
                &fx.db,
                &plan,
                &TransferMode::Copy { new_name: "a".into() },
                OnUnmatched::Fail,
                None
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn move_keeps_originals_another_collection_uses_and_can_repeat() {
        let (fx, col, feature, [alpha, beta]) = setup("shared").await;
        fx.commit("main", &[("lib.dart", LIB)], Some(feature));
        let other = fx.collection("other", std::slice::from_ref(&alpha));

        let plan = plan_transfer(&fx.db, &col, &fx.options("main", false)).await.unwrap();
        let out =
            apply_transfer(&fx.db, &plan, &TransferMode::Move, OnUnmatched::Fail, None).unwrap();
        assert_eq!(out.kept_originals, vec![alpha.clone()]);
        assert_eq!(out.deleted_originals, vec![beta]);
        assert_eq!(
            ordered_member_ids(&fx.db, &other.id).unwrap(),
            vec![alpha],
            "other tour keeps its step"
        );

        // Another tour picks up one of the forks, then the tour is moved again.
        fx.db.add_to_collection(&other.id, std::slice::from_ref(&out.forked[0].fork_id)).unwrap();
        let again = plan_transfer(&fx.db, &col, &fx.options("main", false)).await.unwrap();
        let out2 =
            apply_transfer(&fx.db, &again, &TransferMode::Move, OnUnmatched::Fail, None).unwrap();
        assert_eq!(out2.forked.len(), 2);
        assert!(out2.kept_originals.contains(&out.forked[0].fork_id));
        assert!(ordered_member_ids(&fx.db, &other.id).unwrap().contains(&out.forked[0].fork_id));
    }
}
