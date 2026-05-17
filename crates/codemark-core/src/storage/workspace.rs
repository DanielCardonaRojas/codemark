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
        resolved
            .parent() // .codemark/
            .and_then(|p| p.parent()) // repo dir
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string())
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
        // If explicit paths provided, use only those
        if !opts.explicit_paths.is_empty() {
            let mut dbs = Vec::new();
            for path in &opts.explicit_paths {
                if path.exists() {
                    let label = Self::source_label_from_path(path);
                    dbs.push((label, Database::open(path)?));
                }
            }
            return Ok(dbs);
        }

        // If repo refs provided, resolve via registry
        if !opts.repo_refs.is_empty() {
            return Self::open_repos_from_registry(&opts.repo_refs);
        }

        // Use auto-detected primary + configured additional
        let mut dbs = Vec::new();

        // Always include primary DB
        let cwd = std::env::current_dir().unwrap_or_default();
        let primary_db = Self::open_primary()?;
        let primary_label = if let Some(ctx) = crate::git::context::detect_context(&cwd) {
            ctx.repo_root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "local".to_string())
        } else {
            "local".to_string()
        };
        dbs.push((primary_label, primary_db));

        // Load additional DBs from config
        if let Some(ctx) = crate::git::context::detect_context(&cwd) {
            let codemark_dir = ctx.repo_root.join(".codemark");
            let config = crate::config::Config::load_layered(&codemark_dir);
            let additional_paths = config.databases.resolve_additional_paths(&ctx.repo_root);

            for path in additional_paths {
                if path.exists() {
                    let label = Self::source_label_from_path(&path);
                    if !dbs.iter().any(|(l, _)| l == &label)
                        && let Ok(db) = Database::open(&path)
                    {
                        dbs.push((label, db));
                    }
                }
            }
        }

        Ok(dbs)
    }
}
