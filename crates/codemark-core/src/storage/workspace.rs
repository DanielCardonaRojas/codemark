//! Workspace database discovery and resolution.

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
    /// Auto-detect the database from the current directory (or git root).
    /// If it doesn't exist, returns an in-memory database.
    pub fn open_primary() -> Result<Database> {
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
    pub fn open_primary_for_write() -> Result<Database> {
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
                if let Some(parent) = repo_dir.parent() {
                    if let Some(parent_name) = parent.file_name() {
                        return format!("{}/{}", parent_name.to_string_lossy(), name);
                    }
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
                // For registry repos, the label is the owner/name reference
                let label = repo_ref.clone();
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
        let primary_db = Self::open_primary()?;
        let primary_path = primary_db.path().to_path_buf();

        // 1. Determine if we are in override mode
        // Override mode is only active if explicit_paths is provided and it doesn't just contain the primary DB
        let is_override = !opts.explicit_paths.is_empty()
            && (opts.explicit_paths.len() > 1 || opts.explicit_paths[0] != primary_path);

        if is_override {
            for path in &opts.explicit_paths {
                if path.exists() {
                    let label = Self::source_label_from_path(path);
                    dbs.push((label, Database::open(path)?));
                }
            }
            return Ok(dbs);
        }

        // 2. Additive mode: start with auto-detected primary
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
                    if !dbs.iter().any(|(_, db)| db.path() == path)
                        && let Ok(db) = Database::open(&path)
                    {
                        dbs.push((label, db));
                    }
                }
            }
        }

        // 3. Add any repositories specified by references via the registry
        if !opts.repo_refs.is_empty() {
            let extra_repos = Self::open_repos_from_registry(&opts.repo_refs)?;
            for (label, db) in extra_repos {
                // Avoid duplicates by path check
                if !dbs.iter().any(|(_, d)| d.path() == db.path()) {
                    dbs.push((label, db));
                }
            }
        }

        Ok(dbs)
    }
}
