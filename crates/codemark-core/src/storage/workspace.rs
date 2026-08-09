//! Workspace database discovery and resolution.

use crate::engine::bookmark::Bookmark;
use crate::error::{Error, Result};
use crate::storage::db::Database;
use std::path::{Path, PathBuf};

/// Options for opening multiple databases.
#[derive(Debug, Default, Clone)]
pub struct OpenDbOptions {
    /// Explicit database paths to use (overrides auto-detection)
    pub explicit_paths: Vec<PathBuf>,
    /// Repository references to resolve via registry
    pub repo_refs: Vec<String>,
}

/// Helper for discovering and opening workspace databases.
pub struct Workspace;

impl Workspace {
    /// Get a nice label for the primary database.
    pub fn primary_label() -> String {
        let cwd = std::env::current_dir().unwrap_or_default();
        if let Some(ctx) = crate::git::context::detect_context(&cwd) {
            ctx.repo_root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "local".to_string())
        } else {
            "local".to_string()
        }
    }

    /// Auto-detect the database from the current directory (or git root).
    /// If it doesn't exist, returns an in-memory database.
    pub fn open_primary(explicit_path: Option<&Path>) -> Result<Database> {
        if let Some(path) = explicit_path {
            if path.exists() {
                return Database::open(path);
            }
            return Database::open_in_memory();
        }

        let cwd = std::env::current_dir().unwrap_or_default();
        if let Some(ctx) = crate::git::context::detect_context(&cwd) {
            let db_path = ctx.repo_root.join(".codemark").join("codemark.db");
            if db_path.exists() {
                return Database::open(&db_path);
            }
        } else {
            let db_path = cwd.join(".codemark").join("codemark.db");
            if db_path.exists() {
                return Database::open(&db_path);
            }
        }
        Database::open_in_memory()
    }

    /// Auto-detect the database for writing (creates it if parent .codemark exists).
    pub fn open_primary_for_write(explicit_path: Option<&Path>) -> Result<Database> {
        if let Some(path) = explicit_path {
            return Database::create(path);
        }

        let cwd = std::env::current_dir().unwrap_or_default();
        if let Some(ctx) = crate::git::context::detect_context(&cwd) {
            let db_path = ctx.repo_root.join(".codemark").join("codemark.db");
            if !db_path.parent().map(|p| p.exists()).unwrap_or(false) {
                return Err(Error::NotInitialized);
            }
            return Database::create(&db_path);
        }
        let db_path = cwd.join(".codemark").join("codemark.db");
        if !db_path.parent().map(|p| p.exists()).unwrap_or(false) {
            return Err(Error::NotInitialized);
        }
        Database::create(&db_path)
    }

    /// Determine the project root for a given database.
    pub fn project_root(db: &Database) -> PathBuf {
        let db_path = db.path();
        if let Some(parent) = db_path.parent() {
            if parent.ends_with(".codemark") {
                return parent.parent().unwrap_or(parent).to_path_buf();
            }
            return parent.to_path_buf();
        }
        std::env::current_dir().unwrap_or_default()
    }

    /// Determine the source label from a db path.
    pub fn source_label_from_path(path: &Path) -> String {
        let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

        // Try to get a nice label: "parent-dir/repo-dir" if possible
        if let Some(repo_dir) = resolved.parent().and_then(|p| p.parent()) {
            let repo_name = repo_dir.file_name().map(|n| n.to_string_lossy().to_string());
            if let Some(name) = repo_name {
                // Check if we can get the parent of the repo for more context (helps in tests)
                if let Some(parent) = repo_dir.parent()
                    && let Some(parent_name) = parent.file_name()
                {
                    return format!("{}/{}", parent_name.to_string_lossy(), name);
                }
                return name;
            }
        }

        path.to_string_lossy().to_string()
    }

    /// Open databases for repositories specified by owner/name references via the global registry.
    pub fn open_repos_from_registry(repo_refs: &[String]) -> Result<Vec<(String, Database)>> {
        let conn = crate::storage::registry::open_registry()?;
        let resolved = crate::storage::registry::resolve_repos(&conn, repo_refs)?;

        if resolved.is_empty() {
            return Err(Error::Input(
                "no valid repositories found; run 'codemark repo list' to see known repositories"
                    .into(),
            ));
        }

        let mut dbs = Vec::new();
        for (repo_ref, repo_root) in resolved {
            let db_path = repo_root.join(".codemark").join("codemark.db");
            if db_path.exists() {
                // For registry repos, the label is "owner/name/repo-dir" for better context
                let label = format!(
                    "{}/{}",
                    repo_ref,
                    repo_root.file_name().and_then(|n| n.to_str()).unwrap_or("unknown")
                );
                if let Ok(db) = Database::open(&db_path) {
                    dbs.push((label, db));
                }
            } else {
                eprintln!("codemark: warning: no codemark database found at {}", db_path.display());
            }
        }

        if dbs.is_empty() {
            return Err(Error::Input(
                "no codemark databases found for the specified repositories".into(),
            ));
        }

        Ok(dbs)
    }

    /// Open all specified databases based on options.
    pub fn open_all(opts: &OpenDbOptions) -> Result<Vec<(String, Database)>> {
        let mut dbs = Vec::new();
        let primary_db = Self::open_primary(None)?;
        let primary_path = primary_db.path().to_path_buf();
        let canonical_primary =
            std::fs::canonicalize(&primary_path).unwrap_or_else(|_| primary_path.clone());

        // 1. Determine if we are in override mode for explicit paths
        // Override mode is only active if explicit_paths is provided and it doesn't just contain the primary DB
        let is_override = !opts.explicit_paths.is_empty() && {
            if opts.explicit_paths.len() > 1 {
                true
            } else {
                let explicit_path = std::fs::canonicalize(&opts.explicit_paths[0])
                    .unwrap_or_else(|_| opts.explicit_paths[0].clone());
                explicit_path != canonical_primary
            }
        };

        if is_override {
            for path in &opts.explicit_paths {
                if path.exists() {
                    let label = Self::source_label_from_path(path);
                    dbs.push((label, Database::open(path)?));
                }
            }
            // Fall through to add repo_refs if any
        } else {
            // Additive mode: start with auto-detected primary
            // The primary DB gets the "local" label by default (legacy behavior)
            dbs.push(("local".to_string(), primary_db));

            // Load additional DBs from local config
            let cwd = std::env::current_dir().unwrap_or_default();
            if let Some(ctx) = crate::git::context::detect_context(&cwd) {
                let codemark_dir = ctx.repo_root.join(".codemark");
                let config = crate::config::Config::load_layered(&codemark_dir);
                let additional_paths = config.databases.resolve_additional_paths(&ctx.repo_root);

                for path in additional_paths {
                    if path.exists() {
                        let label = Self::source_label_from_path(&path);
                        // Avoid duplicates by path check since labels might collide
                        let canonical_path =
                            std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                        if !dbs.iter().any(|(_, db)| {
                            std::fs::canonicalize(db.path())
                                .unwrap_or_else(|_| db.path().to_path_buf())
                                == canonical_path
                        }) && let Ok(db) = Database::open(&path)
                        {
                            dbs.push((label, db));
                        }
                    }
                }
            }
        }

        // 3. Add any repositories specified by references via the registry
        if !opts.repo_refs.is_empty() {
            let extra_repos = Self::open_repos_from_registry(&opts.repo_refs)?;
            for (label, db) in extra_repos {
                // Avoid duplicates by path check
                let canonical_db_path =
                    std::fs::canonicalize(db.path()).unwrap_or_else(|_| db.path().to_path_buf());
                if !dbs.iter().any(|(_, d)| {
                    std::fs::canonicalize(d.path()).unwrap_or_else(|_| d.path().to_path_buf())
                        == canonical_db_path
                }) {
                    dbs.push((label, db));
                }
            }
        }

        Ok(dbs)
    }

    /// Search for a bookmark by id or short (>=4 char) prefix across multiple databases.
    /// Returns the bookmark and a reference to the database it was found in.
    pub fn find_bookmark_across<'a>(
        dbs: &'a [(String, Database)],
        id: &str,
    ) -> Result<(Bookmark, &'a Database)> {
        tracing::debug!(target: "codemark::storage", id, "cross-database bookmark lookup started");
        // The TUI tab completion can append the file path and line number after a
        // tab character. The CLI must strip it to recover the bare id.
        let id = id.split('\t').next().unwrap_or(id);
        let id = id.strip_prefix('#').unwrap_or(id);
        for (label, db) in dbs {
            if let Some(bm) = db.get_bookmark(id)? {
                tracing::debug!(target: "codemark::storage", id, "found full id match in '{}'", label);
                return Ok((bm, db));
            }
            if id.len() >= 4
                && let Ok(Some(bm)) = db.get_bookmark_by_prefix(id)
            {
                tracing::debug!(target: "codemark::storage", id, full_id = bm.id, "found prefix match in '{}'", label);
                return Ok((bm, db));
            }
        }
        tracing::debug!(target: "codemark::storage", id, "bookmark not found across any database");
        Err(Error::Input(format!("bookmark not found: {id}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::bookmark::{BookmarkHealth, ResolutionMethod};

    fn test_bookmark(id: &str) -> Bookmark {
        Bookmark {
            id: id.to_string(),
            query: format!("(function_declaration) @{} /* {} */", "target", id),
            language: "swift".to_string(),
            file_path: format!("src/main_{}.swift", id),
            content_hash: Some("sha256:abcd1234abcd1234".to_string()),
            commit_hash: Some("abc123".to_string()),
            health: BookmarkHealth::Active,
            resolution_method: Some(ResolutionMethod::Exact),
            last_resolved_at: None,
            stale_since: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            created_by: None,
            current_resolution_id: None,
            repo_id: None,
            tags: Vec::new(),
            annotations: Vec::new(),
            comments: vec![],
        }
    }

    #[test]
    fn find_bookmark_across_finds_by_prefix_in_second_db() {
        let db1 = Database::open_in_memory().unwrap();
        let db2 = Database::open_in_memory().unwrap();
        // Insert a distinct bookmark into each DB so we can prove *which* DB was
        // selected by asserting on the returned bookmark id.
        db1.insert_bookmark(&test_bookmark("aaaa-1111-2222-3333")).unwrap();
        db2.insert_bookmark(&test_bookmark("beef-1111-2222-3333")).unwrap();

        let dbs = vec![("first".to_string(), db1), ("second".to_string(), db2)];

        // Prefix in the second DB resolves to db2's bookmark.
        let (bm, _db) = Workspace::find_bookmark_across(&dbs, "beef").unwrap();
        assert_eq!(bm.id, "beef-1111-2222-3333");

        // Prefix in the first DB resolves to db1's bookmark, confirming the
        // search actually selects the correct database rather than a fixed one.
        let (bm, _db) = Workspace::find_bookmark_across(&dbs, "aaaa").unwrap();
        assert_eq!(bm.id, "aaaa-1111-2222-3333");
    }

    #[test]
    fn find_bookmark_across_strips_leading_hash() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("beef-1111-2222-3333")).unwrap();

        let dbs = vec![("only".to_string(), db)];

        let (bm, _db) = Workspace::find_bookmark_across(&dbs, "#beef-1111-2222-3333").unwrap();
        assert_eq!(bm.id, "beef-1111-2222-3333");
    }

    #[test]
    fn find_bookmark_across_strips_tab_line_format_suffix() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("beef-1111-2222-3333")).unwrap();

        let dbs = vec![("only".to_string(), db)];

        let (bm, _db) =
            Workspace::find_bookmark_across(&dbs, "beef-1111-2222-3333\tsrc/foo.rs:12").unwrap();
        assert_eq!(bm.id, "beef-1111-2222-3333");
    }
}
