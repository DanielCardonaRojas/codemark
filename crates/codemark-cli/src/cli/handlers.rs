//! CLI command handlers and shared utilities.
//!
//! This module contains the dispatch function and delegates to specialized submodules:
//! - [`bookmark`]: add, show, remove, annotate, resolve
//! - [`collection`]: collection management
//! - [`search`]: FTS and semantic search, reindex
//! - [`maintenance`]: heal, status, diff, gc, export, import

use std::collections::HashSet;
use std::io::Write;

use crate::cli::output::{OutputMode, write_json_success, write_success};
use crate::cli::*;
use codemark_core::config::Config;
use codemark_core::embeddings::config::EmbeddingModel;
use codemark_core::engine::bookmark::{
    Bookmark, BookmarkFilter, BookmarkHealth, Collection, CollectionHealth, Resolution, ResolutionMethod, Visibility,
};
use codemark_core::engine::{health, resolution};
use codemark_core::error::{Error, Result};
use codemark_core::git::context as git_context;
use codemark_core::parser::languages::{Language, ParseCache};
use codemark_core::storage::{SemanticRepo, db::Database};

// Handler submodules
pub mod bookmark;
pub mod collection;
pub mod maintenance;
pub mod publish;
pub mod pull;
pub mod search;
pub mod tour;

/// Dispatch a parsed CLI command to its handler.
pub async fn dispatch(cli: &Cli) -> Result<()> {
    // JSON is the default output format for all commands
    let mode = OutputMode::resolve_with_default(false, cli.format.as_deref(), true);
    match &cli.command {
        Command::Init => handle_init(cli, &mode).await,
        Command::Add(args) => bookmark::handle_add(cli, &mode, args).await,
        Command::AddFromSnippet(args) => bookmark::handle_add_from_snippet(cli, &mode, args).await,
        Command::AddFromQuery(args) => bookmark::handle_add_from_query(cli, &mode, args).await,
        Command::Resolve(args) => bookmark::handle_resolve(cli, &mode, args).await,
        Command::Show(args) => bookmark::handle_show(cli, &mode, args).await,
        Command::Remove(args) => bookmark::handle_remove(cli, &mode, args).await,
        Command::Heal(args) => maintenance::handle_heal(cli, &mode, args).await,
        Command::Status => maintenance::handle_status(cli, &mode).await,
        Command::List(args) => handle_list(cli, &mode, args).await,
        Command::Preview(args) => handle_preview(cli, args).await,
        Command::Search(args) => search::handle_search(cli, &mode, args).await,
        Command::Reindex(args) => search::handle_reindex(cli, &mode, args).await,
        Command::Collection(args) => dispatch_collection(cli, &mode, args).await,
        Command::Diff(args) => maintenance::handle_diff(cli, &mode, args).await,
        Command::Gc(args) => maintenance::handle_gc(cli, &mode, args).await,
        Command::Export(args) => maintenance::handle_export(cli, args).await,
        Command::Import(args) => maintenance::handle_import(cli, &mode, args).await,
        Command::Completions(args) => handle_completions(args),
        Command::Annotate(args) => bookmark::handle_annotate(cli, &mode, args).await,
        Command::Open(args) => handle_open(cli, args).await,
        Command::Publish(args) => publish::handle_publish(cli, &mode, args).await,
        Command::Pull(args) => pull::handle_pull(cli, &mode, args).await,
        Command::Tour(args) => dispatch_tour(cli, &mode, args).await,
    }
}

async fn dispatch_collection(cli: &Cli, mode: &OutputMode, args: &CollectionArgs) -> Result<()> {
    match &args.command {
        CollectionCommand::Create(a) => collection::handle_collection_create(cli, mode, a).await,
        CollectionCommand::Delete(a) => collection::handle_collection_delete(cli, mode, a).await,
        CollectionCommand::Add(a) => collection::handle_collection_add(cli, mode, a).await,
        CollectionCommand::Remove(a) => collection::handle_collection_remove(cli, mode, a).await,
        CollectionCommand::List(a) => collection::handle_collection_list(cli, mode, a).await,
        CollectionCommand::Show(a) => collection::handle_collection_show(cli, mode, a).await,
        CollectionCommand::Resolve(a) => collection::handle_collection_resolve(cli, mode, a).await,
        CollectionCommand::Reorder(a) => collection::handle_collection_reorder(cli, mode, a).await,
        CollectionCommand::Tag(a) => collection::handle_collection_tag(cli, mode, a).await,
        CollectionCommand::Link(a) => collection::handle_collection_link(cli, mode, a).await,
    }
}

async fn dispatch_tour(cli: &Cli, mode: &OutputMode, args: &TourArgs) -> Result<()> {
    match &args.command {
        TourCommand::List(a) => tour::handle_tour_list(cli, mode, a).await,
    }
}

// --- Shared helpers ---

/// Open the primary database for writing.
///
/// If an explicit path is provided, it will be created if it doesn't exist.
/// For auto-detected paths, it requires the .codemark directory to exist.
pub fn open_db_for_write(cli: &Cli) -> Result<Database> {
    if let Some(path) = cli.db.first() {
        return Database::create(path);
    }
    // Auto-detect from git root
    let cwd = std::env::current_dir()?;
    if let Some(ctx) = git_context::detect_context(&cwd) {
        let db_path = ctx.repo_root.join(".codemark").join("codemark.db");
        if !db_path.parent().map(|p| p.exists()).unwrap_or(false) {
            return Err(Error::NotInitialized);
        }
        return Database::create(&db_path);
    }
    // Fallback: current directory
    let db_path = cwd.join(".codemark").join("codemark.db");
    if !db_path.parent().map(|p| p.exists()).unwrap_or(false) {
        return Err(Error::NotInitialized);
    }
    Database::create(&db_path)
}

/// Returns the project root (parent of .codemark) for a given database.
pub fn get_project_root(db: &Database) -> std::path::PathBuf {
    let db_path = db.path();
    if let Some(parent) = db_path.parent() {
        if parent.ends_with(".codemark") {
            return parent.parent().unwrap_or(parent).to_path_buf();
        }
        return parent.to_path_buf();
    }
    std::env::current_dir().unwrap_or_default()
}

/// Open the primary database (for single-db reads).
///
/// Returns Error::NotInitialized only if an explicit path was provided but it doesn't exist.
/// For auto-detected paths, if the DB doesn't exist, it returns an in-memory DB (effectively empty).
pub fn open_db(cli: &Cli) -> Result<Database> {
    if let Some(path) = cli.db.first() {
        if path.exists() {
            return Database::open(path);
        }
        return Database::open_in_memory();
    }
    // Auto-detect from git root
    let cwd = std::env::current_dir()?;
    if let Some(ctx) = git_context::detect_context(&cwd) {
        let db_path = ctx.repo_root.join(".codemark").join("codemark.db");
        if db_path.exists() {
            return Database::open(&db_path);
        }
    } else {
        // Fallback: current directory
        let db_path = cwd.join(".codemark").join("codemark.db");
        if db_path.exists() {
            return Database::open(&db_path);
        }
    }
    Database::open_in_memory()
}

/// Generate embedding for a bookmark if semantic search is enabled.
/// Returns Ok(()) even if semantic search is disabled or fails.
pub async fn generate_embedding_for_bookmark(
    cli: &Cli,
    config: &Config,
    bookmark: &Bookmark,
) -> Result<()> {
    if !config.semantic.is_enabled() {
        return Ok(());
    }

    // Get models directory from config (defaults to global cache)
    let models_dir = config.semantic.get_models_dir();

    // Parse model from config
    let model = config
        .semantic
        .model
        .as_deref()
        .and_then(|m| m.parse::<EmbeddingModel>().ok())
        .unwrap_or(EmbeddingModel::AllMiniLmL6V2);

    // Get distance metric and threshold from config
    let distance_metric = config.semantic.get_distance_metric();
    let threshold = config.semantic.threshold;

    let semantic_repo = SemanticRepo::with_config(models_dir, model, distance_metric, threshold);

    // Open database for writing embedding
    let mut db = open_db_for_write(cli)?;

    // Generate and store the embedding
    let conn = db.conn_mut();
    // Ignore errors - embedding generation failure shouldn't block bookmark creation
    let _ = semantic_repo.store_embeddings(conn, std::slice::from_ref(bookmark)).await;

    Ok(())
}

/// Load the config from the .codemark directory (same location as the primary DB).
/// Uses layered loading: global config merged with local (per-repo) override.
pub fn load_config(cli: &Cli) -> Config {
    if let Some(path) = cli.db.first()
        && let Some(parent) = path.parent()
    {
        return Config::load_layered(parent);
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    if let Some(ctx) = git_context::detect_context(&cwd) {
        return Config::load_layered(&ctx.repo_root.join(".codemark"));
    }
    Config::load_layered(&cwd.join(".codemark"))
}

/// Resolve the current user identity for bookmark and repo metadata creation.
///
/// Returns (db_owner_email, db_owner_name). Uses config override, git config, or system fallback.
pub fn resolve_identity(config: &Config) -> (String, Option<String>) {
    // If force identity is set, use it as both email and name
    if let Some(ref forced) = config.identity.force {
        return (forced.clone(), Some(forced.clone()));
    }

    let cwd = std::env::current_dir().unwrap_or_default();

    // Try git config first
    if let Some(identity) = git_context::detect_identity(&cwd) {
        let name = config.identity.name.clone().or(identity.user_name.clone());
        // Synthesize a unique local email if not configured
        let email = config.identity.email.clone().or(identity.user_email).unwrap_or_else(|| {
            identity
                .user_name
                .as_ref()
                .map(|n| format!("{n}@local"))
                .unwrap_or_else(|| "user@local".to_string())
        });
        return (email, name);
    }

    // Fallback to system username
    let fallback = git_context::detect_fallback_identity();
    let name = config.identity.name.clone().or(fallback.user_name.clone());
    // Synthesize a unique local email if not configured
    let email = config.identity.email.clone().or(fallback.user_email).unwrap_or_else(|| {
        fallback
            .user_name
            .as_ref()
            .map(|n| format!("{n}@local"))
            .unwrap_or_else(|| "user@local".to_string())
    });
    (email, name)
}

/// Resolve or create repository metadata in the database.
///
/// Detects git repo information (origin URL, owner, name) and upserts to the repos table.
/// Returns the repo ID if successful, None if not in a git repo.
pub fn resolve_or_create_repo_metadata(
    db: &Database,
    _config: &Config,
    db_owner_email: &str,
    db_owner_name: Option<&str>,
) -> Result<Option<String>> {
    let cwd = std::env::current_dir()?;

    // Get git context
    let git_ctx = match git_context::detect_context(&cwd) {
        Some(ctx) => ctx,
        None => return Ok(None), // Not in a git repo
    };

    // Detect repo metadata
    let repo_metadata = git_context::detect_repo_metadata(&cwd);
    let Some(repo_metadata) = repo_metadata else {
        return Ok(None);
    };

    let new_name = db_owner_name.map(|s| s.to_string());

    // Try to find existing repo by origin URL first
    if let Some(ref origin_url) = repo_metadata.origin_url {
        if let Ok(Some(existing)) = db.get_repo_by_origin(origin_url)
            && (existing.db_owner_email != db_owner_email || existing.db_owner_name != new_name)
        {
            let mut updated = existing.clone();
            updated.db_owner_email = db_owner_email.to_string();
            updated.db_owner_name = new_name;
            db.upsert_repo(&updated)?;
            return Ok(Some(existing.id));
        }

        if let Ok(Some(existing)) = db.get_repo_by_origin(origin_url) {
            return Ok(Some(existing.id));
        }
    }

    // For local repos (no origin_url), try to find by repo_root
    // This prevents duplicate rows when running codemark init multiple times
    let repo_root_str = git_ctx.repo_root.to_string_lossy().to_string();
    if let Ok(Some(existing)) = db.get_repo_by_root(&repo_root_str)
        && (existing.db_owner_email != db_owner_email || existing.db_owner_name != new_name)
    {
        let mut updated = existing.clone();
        updated.db_owner_email = db_owner_email.to_string();
        updated.db_owner_name = new_name;
        db.upsert_repo(&updated)?;
        return Ok(Some(existing.id));
    }

    if let Ok(Some(existing)) = db.get_repo_by_root(&repo_root_str) {
        return Ok(Some(existing.id));
    }

    // Create new repo entry
    use codemark_core::engine::bookmark::Repo;
    use uuid::Uuid;

    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let repo = Repo {
        id: Uuid::new_v4().to_string(),
        repo_owner: repo_metadata.repo_owner.unwrap_or_else(|| "unknown".to_string()),
        repo_name: repo_metadata.repo_name.unwrap_or_else(|| {
            git_ctx
                .repo_root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string())
        }),
        origin_url: repo_metadata.origin_url,
        repo_root: repo_root_str,
        db_owner_email: db_owner_email.to_string(),
        db_owner_name: new_name,
        detected_at: now,
    };

    let repo_id = db.upsert_repo(&repo)?;
    Ok(Some(repo_id))
}

/// Open all specified databases (for read commands that support cross-repo queries).
///
/// Returns (source_label, database) pairs. Falls back to single auto-detected db.
/// Returns Error::NotInitialized if any of the specified databases do not exist.
///
/// Database loading strategy:
/// 1. If CLI `--db` is specified, only those paths are used (override mode)
/// 2. Otherwise, uses auto-detected primary DB + configured additional databases
pub fn open_all_dbs(cli: &Cli) -> Result<Vec<(String, Database)>> {
    // If CLI specified explicit --db paths, use only those (override mode)
    if !cli.db.is_empty() {
        let mut dbs = Vec::new();
        for path in &cli.db {
            if path.exists() {
                let label = source_label_from_path(path);
                dbs.push((label, Database::open(path)?));
            }
        }
        return Ok(dbs);
    }

    // No CLI --db specified: use auto-detected primary + configured additional
    let mut dbs = Vec::new();

    // Always include primary DB (auto-detected from git root)
    let primary_db = open_db(cli)?;
    let primary_label = source_label_from_cli(cli);
    dbs.push((primary_label, primary_db));

    // Load additional DBs from local config
    let cwd = std::env::current_dir()?;
    if let Some(ctx) = git_context::detect_context(&cwd) {
        let codemark_dir = ctx.repo_root.join(".codemark");
        let config = Config::load_layered(&codemark_dir);
        let additional_paths = config.databases.resolve_additional_paths(&ctx.repo_root);

        for path in additional_paths {
            if path.exists() {
                let label = source_label_from_path(&path);
                // Only add if not already present (avoid duplicates)
                if !dbs.iter().any(|(l, _)| l == &label) {
                    // Silently skip databases that fail to open (e.g., locked, corrupted)
                    // This prevents flakiness when tests run in parallel
                    if let Ok(db) = Database::open(&path) {
                        dbs.push((label, db));
                    }
                }
            }
        }
    }

    Ok(dbs)
}

/// Filter databases by user email (from repos table).
///
/// Returns only databases where the db_owner_email in the repos table matches the given email.
/// If the email is None, returns all databases.
/// If the repos table is empty (Ok(None)), keeps the database (allows querying unmigrated DBs).
pub fn filter_dbs_by_user_email(
    dbs: Vec<(String, Database)>,
    user_email: Option<&str>,
) -> Vec<(String, Database)> {
    let Some(email) = user_email else {
        return dbs;
    };

    dbs.into_iter()
        .filter(|(label, db)| {
            match db.get_db_owner() {
                Ok(Some(owner)) => owner == email,
                Ok(None) => true, // Empty repos table - keep the DB
                Err(e) => {
                    eprintln!(
                        "codemark: warning: failed to query repos table in database '{}': {e}",
                        label
                    );
                    false // Skip databases with errors
                }
            }
        })
        .collect()
}

/// Filter databases by repository owner (from repos table).
///
/// Returns only databases where any repo has repo_owner matching the given pattern.
/// Empty repos tables are kept (consistent with filter_dbs_by_user_email).
pub fn filter_dbs_by_repo_owner(
    dbs: Vec<(String, Database)>,
    repo_owner: Option<&str>,
) -> Vec<(String, Database)> {
    let Some(pattern) = repo_owner else {
        return dbs;
    };

    dbs.into_iter()
        .filter(|(label, db)| {
            match db.list_repos() {
                Ok(repos) => {
                    // Keep DB if any repo has a matching owner, or if the table is empty
                    // (consistent with filter_dbs_by_user_email's Ok(None) handling).
                    repos.is_empty() || repos.iter().any(|r| r.repo_owner.contains(pattern))
                }
                Err(e) => {
                    eprintln!(
                        "codemark: warning: failed to query repos table in database '{}': {e}",
                        label
                    );
                    false
                }
            }
        })
        .collect()
}

/// Open all specified databases for commands that support additional --db flags.
///
/// This extends `open_all_dbs` to include command-specific --db flags (from ListArgs/SearchArgs).
pub fn open_all_dbs_with_extra(
    cli: &Cli,
    extra_db_paths: &[String],
) -> Result<Vec<(String, Database)>> {
    let mut dbs = open_all_dbs(cli)?;

    // Add extra databases from command-specific --db flags
    for path_str in extra_db_paths {
        let path = std::path::PathBuf::from(path_str);
        if path.exists() {
            let label = source_label_from_path(&path);
            // Only add if not already present (avoid duplicates)
            if !dbs.iter().any(|(l, _)| l == &label)
                && let Ok(db) = Database::open(&path)
            {
                dbs.push((label, db));
            }
        }
    }

    Ok(dbs)
}

/// Derive a source label from a db path.
///
/// Example: /foo/repo-name/.codemark/codemark.db -> "repo-name"
pub fn source_label_from_path(path: &std::path::Path) -> String {
    // Canonicalize to resolve relative paths like .codemark/codemark.db
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    resolved
        .parent() // .codemark/
        .and_then(|p| p.parent()) // repo dir
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn source_label_from_cli(cli: &Cli) -> String {
    if let Some(path) = cli.db.first() {
        return source_label_from_path(path);
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    if let Some(ctx) = git_context::detect_context(&cwd) {
        ctx.repo_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "local".to_string())
    } else {
        "local".to_string()
    }
}

/// Parse a range string. Supports:
/// - "42" — single line
/// - "42:10" — line 42, column 10
/// - "42-67" — line range (inclusive)
/// - "b42-67" or "42-67b" — byte range (explicit)
pub fn parse_range(range: &str, source: &str) -> Result<(usize, usize)> {
    let r = range.trim();

    // Byte range: b prefix or b suffix
    if r.starts_with('b') || r.starts_with('B') {
        return parse_byte_range(&r[1..]);
    }
    if r.ends_with('b') || r.ends_with('B') {
        return parse_byte_range(&r[..r.len() - 1]);
    }

    // Unambiguous Range: START-END (e.g., 10-20 or 10:5-10:20)
    if let Some((start_str, end_str)) = r.split_once('-') {
        let start = parse_point(source, start_str)?;
        let end = parse_point(source, end_str)?;
        if start > end {
            return Err(Error::Input(format!(
                "invalid range: start byte {} is greater than end byte {}",
                start, end
            )));
        }
        return Ok((start, end));
    }

    // Point or Single Line: LINE:COL or LINE
    if let Some((line_str, col_str)) = r.split_once(':') {
        let line: usize = line_str
            .parse()
            .map_err(|_| Error::Input(format!("invalid line number: {}", line_str)))?;
        let col: usize = col_str
            .parse()
            .map_err(|_| Error::Input(format!("invalid column number: {}", col_str)))?;
        let byte = line_col_to_byte(source, line, col)?;
        Ok((byte, byte))
    } else {
        // Single line: treat as a range covering the whole line
        let line: usize =
            r.parse().map_err(|_| Error::Input(format!("invalid line number: {}", r)))?;
        line_range_to_bytes(source, line, line)
    }
}

/// Parse a byte range string (e.g., "100-200" or "100:200").
fn parse_byte_range(s: &str) -> Result<(usize, usize)> {
    let (start_str, end_str) = s
        .split_once('-')
        .or_else(|| s.split_once(':'))
        .ok_or_else(|| Error::Input("byte range must be start-end or start:end".into()))?;

    let start: usize =
        start_str.parse().map_err(|_| Error::Input("invalid byte range start".into()))?;
    let end: usize = end_str.parse().map_err(|_| Error::Input("invalid byte range end".into()))?;
    Ok((start, end))
}

/// Convert a 1-indexed line and column to a 0-indexed byte offset.
fn line_col_to_byte(source: &str, line: usize, col: usize) -> Result<usize> {
    if line == 0 || col == 0 {
        return Err(Error::Input("line and column numbers are 1-indexed".into()));
    }

    let mut byte_offset = 0;
    for (i, line_text) in source.split_inclusive('\n').enumerate() {
        let line_num = i + 1;
        if line_num == line {
            // Trim trailing newline/carriage return for bounds check
            let clean_line = line_text.trim_end_matches(['\n', '\r']);
            if col > clean_line.len() + 1 {
                return Err(Error::Input(format!(
                    "column {} is out of bounds for line {}",
                    col, line
                )));
            }
            return Ok(byte_offset + col - 1);
        }
        byte_offset += line_text.len();
    }

    Err(Error::Input(format!("line {} is out of bounds", line)))
}

/// Parse a point string (e.g., "42:10" or "42") and return its byte offset.
fn parse_point(source: &str, s: &str) -> Result<usize> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() == 2 {
        let line: usize = parts[0].parse().map_err(|_| Error::Input("invalid line".into()))?;
        let col: usize = parts[1].parse().map_err(|_| Error::Input("invalid col".into()))?;
        line_col_to_byte(source, line, col)
    } else {
        let line: usize = s.parse().map_err(|_| Error::Input("invalid line".into()))?;
        let (start, _) = line_range_to_bytes(source, line, line)?;
        Ok(start)
    }
}

/// Convert byte offset to line number (1-indexed).
pub fn byte_to_line(source: &str, byte_offset: usize) -> usize {
    let mut current_byte = 0;
    for (i, line_text) in source.split_inclusive('\n').enumerate() {
        current_byte += line_text.len();
        if current_byte > byte_offset {
            return i + 1;
        }
    }
    // If we're exactly at the end of the file
    source.split_inclusive('\n').count()
}

/// Convert 1-indexed inclusive line range to byte range.
pub fn line_range_to_bytes(
    source: &str,
    start_line: usize,
    end_line: usize,
) -> Result<(usize, usize)> {
    if start_line == 0 || end_line == 0 {
        return Err(Error::Input("line numbers are 1-indexed".into()));
    }
    if start_line > end_line {
        return Err(Error::Input(format!("start line {start_line} > end line {end_line}")));
    }

    let mut start_byte = None;
    let mut end_byte = None;
    let mut current_byte = 0;

    for (i, line_text) in source.split_inclusive('\n').enumerate() {
        let line_num = i + 1;
        if line_num == start_line {
            start_byte = Some(current_byte);
        }
        current_byte += line_text.len();
        if line_num == end_line {
            end_byte = Some(current_byte);
            break;
        }
    }

    match (start_byte, end_byte) {
        (Some(s), Some(e)) => Ok((s, e.min(source.len()))),
        _ => {
            let line_count = source.split_inclusive('\n').count();
            Err(Error::Input(format!(
                "line range {start_line}-{end_line} is out of bounds (file has {} lines)",
                line_count
            )))
        }
    }
}

/// Parse a git diff hunk header to extract the new-side line range.
/// Format: @@ -old_start[,old_count] +new_start[,new_count] @@ [context]
pub fn parse_hunk(hunk: &str) -> Result<(usize, usize)> {
    let re = regex::Regex::new(r"\+(\d+)(?:,(\d+))?").unwrap();
    let caps =
        re.captures(hunk).ok_or_else(|| Error::Input(format!("invalid hunk format: {hunk}")))?;
    let start: usize = caps[1].parse().map_err(|_| Error::Input("invalid hunk start".into()))?;
    let count: usize =
        caps.get(2).map(|m: regex::Match| m.as_str().parse().unwrap_or(1)).unwrap_or(1);
    let end = start + count.saturating_sub(1);
    Ok((start, end.max(start)))
}

pub fn parse_status_filter(status: Option<&str>) -> Option<Vec<BookmarkHealth>> {
    status.map(|s| s.split(',').filter_map(|part| part.trim().parse().ok()).collect())
}

/// Emit a one-time deprecation warning for --status.
pub fn check_deprecated_status(health: &Option<String>, status: &Option<String>) {
    if health.is_none() && status.is_some() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static WARNED: AtomicBool = AtomicBool::new(false);
        if !WARNED.swap(true, Ordering::SeqCst) {
            eprintln!("codemark: warning: --status is deprecated, use --health instead");
        }
    }
}

/// Extract ID from input (handles line-format input).
pub fn extract_id(id_or_line: &str) -> &str {
    // If it looks like a line-format string (contains tabs), extract the first field
    id_or_line.split('\t').next().unwrap_or(id_or_line)
}

/// Get the center line number from a bookmark's latest resolution.
/// Returns the center line number (1-indexed) or None if no resolution exists.
/// Note: bookmark_id should be the full ID, not a short prefix.
pub fn get_bookmark_line(db: &Database, bookmark_id: &str, _file_path: &str) -> Option<usize> {
    // Get the latest resolution (limit 1)
    let resolutions = db.list_resolutions(bookmark_id, 1).ok()?;
    let res = resolutions.first()?;

    // Parse the line_range "start-end" (1-indexed, inclusive)
    let line_range = res.line_range.as_ref()?;
    let parts: Vec<&str> = line_range.split('-').collect();
    if parts.len() != 2 {
        return None;
    }

    let start: usize = parts[0].parse().ok()?;
    let end: usize = parts[1].parse().ok()?;

    // Return the center line for better preview positioning
    Some((start + end) / 2)
}

/// Find a bookmark by ID or prefix in a single database.
pub fn find_bookmark(db: &Database, id: &str) -> Result<Bookmark> {
    let id = extract_id(id);
    // Try exact match first, then prefix
    if let Some(bm) = db.get_bookmark(id)? {
        return Ok(bm);
    }
    db.get_bookmark_by_prefix(id)?.ok_or_else(|| Error::Input(format!("bookmark not found: {id}")))
}

/// Search for a bookmark across multiple databases. Returns the bookmark and a reference to the DB.
pub fn find_bookmark_across<'a>(
    dbs: &'a [(String, Database)],
    id: &str,
) -> Result<(Bookmark, &'a Database)> {
    let id = extract_id(id);
    for (_label, db) in dbs {
        if let Some(bm) = db.get_bookmark(id)? {
            return Ok((bm, db));
        }
        if id.len() >= 4
            && let Ok(Some(bm)) = db.get_bookmark_by_prefix(id)
        {
            return Ok((bm, db));
        }
    }
    Err(Error::Input(format!("bookmark not found: {id}")))
}

/// Get current timestamp in ISO format.
pub fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Parse duration string (e.g., "30d", "2w", "6m") to days.
pub fn parse_duration_days(duration: &str) -> Result<i64> {
    let s = duration.trim();
    if let Some(d) = s.strip_suffix('d') {
        return d.parse().map_err(|_| Error::Input("invalid duration".into()));
    }
    if let Some(w) = s.strip_suffix('w') {
        let weeks: i64 = w.parse().map_err(|_| Error::Input("invalid duration".into()))?;
        return Ok(weeks * 7);
    }
    if let Some(m) = s.strip_suffix('m') {
        let months: i64 = m.parse().map_err(|_| Error::Input("invalid duration".into()))?;
        return Ok(months * 30);
    }
    Err(Error::Input("duration must end with d, w, or m (e.g., 30d, 2w, 6m)".into()))
}

/// Add a bookmark to a collection, auto-creating the collection if it doesn't exist.
/// Returns the collection name if the bookmark was added, None otherwise.
pub fn add_bookmark_to_collection(
    db: &Database,
    bookmark_id: &str,
    collection_name: &str,
) -> Result<Option<String>> {
    // Auto-create collection if it doesn't exist (same logic as handle_collection_add)
    let collection = match db.get_collection_by_name(collection_name)? {
        Some(c) => c,
        None => {
            let cwd = std::env::current_dir().unwrap_or_default();
            let git_ctx = git_context::detect_context(&cwd);
            let created_branch = git_ctx.and_then(|ctx| ctx.branch_name);

            let c = Collection {
                id: uuid::Uuid::new_v4().to_string(),
                name: collection_name.to_string(),
                description: None,
                visibility: Visibility::Private,
                created_at: now_iso(),
                created_by: None,
                created_branch,
                published_at: None,
                published_commit_sha: None,
                repo_url: None,
                status: None,
                health: None,
                health_computed_at: None,
                updated_at: None,
                imported_from_url: None,
            };
            db.insert_collection(&c)?;
            c
        }
    };
    db.add_to_collection(&collection.id, &[bookmark_id.to_string()])?;
    Ok(Some(collection_name.to_string()))
}

/// A structured result for a single bookmark resolution.
#[derive(serde::Serialize)]
pub struct ResolutionRecord {
    pub id: String,
    pub file: String,
    pub line: usize,
    pub method: String,
    pub status: String,
}

/// Batch resolve bookmarks and return structured results.
pub async fn resolve_batch(
    db: &Database,
    bookmarks: &[Bookmark],
    config: &Config,
    dry_run: bool,
) -> Result<Vec<ResolutionRecord>> {
    use codemark_core::parser::languages::ParseCache;

    let mut results = Vec::new();
    let provider = codemark_core::vfs::LocalFileProvider;
    let mut affected_collections: HashSet<String> = std::collections::HashSet::new();

    for bm in bookmarks {
        let Ok(lang) = bm.language.parse::<Language>() else {
            continue;
        };
        let mut cache = ParseCache::new(lang)?;
        let ts_lang = lang.tree_sitter_language();
        let result = resolution::resolve(bm, &mut cache, &ts_lang, db.path(), &provider).await?;
        let new_status = health::transition(bm.health, result.method, result.hash_matches);

        // Resolve relative path to absolute for output
        let absolute_path = git_context::resolve_bookmark_file_path(&result.file_path, db.path())
            .unwrap_or_else(|_| std::path::PathBuf::from(&result.file_path));

        results.push(ResolutionRecord {
            id: crate::cli::output::short_id(&bm.id).to_string(),
            file: absolute_path.to_string_lossy().to_string(),
            line: result.start_line + 1,
            method: result.method.to_string(),
            status: new_status.to_string(),
        });

        // In dry-run mode, skip database updates
        if dry_run {
            continue;
        }

        let stale_since = if new_status == BookmarkHealth::Stale {
            bm.stale_since.clone().or_else(|| Some(now_iso()))
        } else {
            None
        };

        db.update_bookmark_health(
            &bm.id,
            new_status,
            Some(result.method),
            Some(&now_iso()),
            stale_since.as_deref(),
        )?;

        if let Some(ref new_query) = result.new_query {
            db.update_bookmark_query(&bm.id, new_query, &result.file_path, &result.content_hash)?;
        }

        // Record resolution history (deduped)
        let res = Resolution {
            id: uuid::Uuid::new_v4().to_string(),
            bookmark_id: bm.id.clone(),
            resolved_at: now_iso(),
            commit_hash: git_context::detect_context(&std::env::current_dir()?)
                .and_then(|ctx| ctx.head_commit),
            method: result.method,
            match_count: Some(1),
            file_path: Some(result.file_path.clone()),
            byte_range: Some(format!("{}-{}", result.byte_range.0, result.byte_range.1)),
            line_range: Some(format!("{}-{}", result.start_line + 1, result.end_line + 1)),
            content_hash: Some(result.content_hash.clone()),
            headline: None,
            preview_lines: None,
        };
        let _ = db.insert_resolution_if_changed(&res, config.storage.max_resolutions());
    }

    Ok(results)
}

/// Output a batch of resolution results.
pub fn write_batch_output(mode: &OutputMode, results: &[ResolutionRecord]) -> Result<()> {
    match mode {
        OutputMode::Json => write_json_success(&results)?,
        OutputMode::Table => {
            let mut table = comfy_table::Table::new();
            table.set_header(vec!["ID", "File", "Line", "Method", "Status"]);
            for r in results {
                table.add_row(vec![
                    r.id.clone(),
                    r.file.clone(),
                    r.line.to_string(),
                    r.method.clone(),
                    r.status.clone(),
                ]);
            }
            println!("{table}");
        }
        _ => {
            let mut stdout = std::io::stdout().lock();
            for r in results {
                writeln!(stdout, "{}\t{}:{}\t{}\t{}", r.id, r.file, r.line, r.method, r.status,)?;
            }
        }
    }
    Ok(())
}

/// Write resolution output for a single bookmark.
pub fn write_resolution_output(
    mode: &OutputMode,
    bm: &Bookmark,
    result: &resolution::ResolutionResult,
    db_path: &std::path::Path,
) -> Result<()> {
    use crate::cli::output::short_id;

    // Get first annotation's note for display
    let note = bm.annotations.first().and_then(|a| a.notes.as_deref());

    // Resolve relative path to absolute for output
    let absolute_path = git_context::resolve_bookmark_file_path(&result.file_path, db_path)?;
    let absolute_path_str = absolute_path.to_string_lossy();

    match mode {
        OutputMode::Json => {
            write_json_success(&serde_json::json!({
                "id": bm.id,
                "file": absolute_path_str,
                "line": result.start_line + 1,
                "column": result.start_col,
                "byte_range": format!("{}-{}", result.byte_range.0, result.byte_range.1),
                "line_range": format!("{}-{}", result.start_line + 1, result.end_line + 1),
                "line_range_colon": format!("{}:{}", result.start_line + 1, result.end_line + 1),
                "method": result.method.to_string(),
                "status": health::transition(bm.health, result.method, result.hash_matches).to_string(),
                "preview": result.matched_text.lines().next().unwrap_or(""),
                "note": note,
                "tags": bm.tags,
            }))?;
        }
        OutputMode::Line => {
            let mut stdout = std::io::stdout().lock();
            writeln!(
                stdout,
                "{}\t{}:{}\t{}\t{}\t{}",
                short_id(&bm.id),
                absolute_path_str,
                result.start_line + 1,
                result.method,
                bm.tags.join(","),
                note.unwrap_or("")
            )?;
        }
        _ => {
            println!(
                "{}  {}:{}  [{}]",
                short_id(&bm.id),
                absolute_path_str,
                result.start_line + 1,
                result.method
            );
            if let Some(preview) = result.matched_text.lines().next() {
                println!("  {preview}");
            }
        }
    }
    Ok(())
}

// --- Individual command handlers ---

/// Initialize a new codemark repository in the current directory or git root.
///
/// Creates the .codemark directory and initializes the database.
/// If already initialized, prints a message and does nothing.
pub async fn handle_init(cli: &Cli, mode: &OutputMode) -> Result<()> {
    if let Some(path) = cli.db.first() {
        if path.exists() {
            write_success(mode, &format!("Codemark already initialized at {}", path.display()))?;
            return Ok(());
        }
        Database::create(path)?;

        // Populate repos table with identity and repo metadata
        let db = Database::open(path)?;
        let config = load_config(cli);
        let (db_owner_email, db_owner_name) = resolve_identity(&config);
        resolve_or_create_repo_metadata(&db, &config, &db_owner_email, db_owner_name.as_deref())?;

        write_success(mode, &format!("Initialized codemark database at {}", path.display()))?;
        return Ok(());
    }

    let cwd = std::env::current_dir()?;
    let git_ctx = git_context::detect_context(&cwd);
    let db_path = if let Some(ref ctx) = git_ctx {
        ctx.repo_root.join(".codemark").join("codemark.db")
    } else {
        cwd.join(".codemark").join("codemark.db")
    };

    if db_path.exists() {
        write_success(mode, &format!("Codemark already initialized at {}", db_path.display()))?;
        return Ok(());
    }

    Database::create(&db_path)?;

    // Populate repos table with identity and repo metadata
    let db = Database::open(&db_path)?;
    let config = load_config(cli);
    let (db_owner_email, db_owner_name) = resolve_identity(&config);
    resolve_or_create_repo_metadata(&db, &config, &db_owner_email, db_owner_name.as_deref())?;

    write_success(
        mode,
        &format!("Initialized codemark repository in {}", db_path.parent().unwrap().display()),
    )?;

    if git_ctx.is_some() {
        eprintln!("Note: It is recommended to add .codemark/ to your .gitignore");
    }

    Ok(())
}

/// Generate shell completions for the specified shell.
pub fn handle_completions(args: &CompletionsArgs) -> Result<()> {
    use clap::CommandFactory;
    use clap_complete::generate;

    let mut cmd = Cli::command();
    generate(args.shell, &mut cmd, "codemark", &mut std::io::stdout());
    Ok(())
}

/// List bookmarks with optional filters.
pub async fn handle_list(cli: &Cli, mode: &OutputMode, args: &ListArgs) -> Result<()> {
    use crate::cli::output::template_needs_line;

    check_deprecated_status(&args.health, &args.status);

    let dbs = open_all_dbs_with_extra(cli, &args.add_db)?;
    let dbs = filter_dbs_by_user_email(dbs, args.user_email.as_deref());
    let dbs = filter_dbs_by_repo_owner(dbs, args.repo_owner.as_deref());

    let health_input = args.health.as_deref().or(args.status.as_deref());
    let filter = BookmarkFilter {
        tag: args.tag.clone(),
        health: parse_status_filter(health_input)
            .or(Some(vec![BookmarkHealth::Active, BookmarkHealth::Drifted])),
        file_path: args.file.as_ref().map(|p| p.to_string_lossy().to_string()),
        language: args.lang.clone(),
        created_by: args.author.clone(),
        collection: args.collection.clone(),
        collection_id: args.collection_id.clone(),
        limit: args.limit,
    };

    // Check if we need line numbers
    let needs_line =
        mode.needs_line() || args.line_format.as_deref().is_some_and(template_needs_line);

    if dbs.len() == 1 {
        let bookmarks = dbs[0].1.list_bookmarks(&filter)?;
        let db = &dbs[0].1;

        if needs_line {
            // Capture both full IDs and file paths
            let bookmark_data: std::collections::HashMap<String, (String, String)> = bookmarks
                .iter()
                .map(|bm| {
                    (
                        crate::cli::output::short_id(&bm.id).to_string(),
                        (bm.id.clone(), bm.file_path.clone()),
                    )
                })
                .collect();
            crate::cli::output::write_bookmarks_with_line(
                mode,
                &bookmarks,
                args.line_format.as_deref(),
                |short_id| {
                    let (full_id, file_path) = bookmark_data.get(short_id)?;
                    get_bookmark_line(db, full_id, file_path)
                },
            )?;
        } else {
            crate::cli::output::write_bookmarks(mode, &bookmarks, args.line_format.as_deref())?;
        }
    } else {
        // Multi-database case with line number support
        let mut all = Vec::new();
        // Keep track of which database each bookmark belongs to
        let mut db_map: std::collections::HashMap<String, &Database> =
            std::collections::HashMap::new();
        for (label, db) in &dbs {
            db_map.insert(label.clone(), db);
            let bookmarks = db.list_bookmarks(&filter)?;
            for bm in bookmarks {
                all.push((label.clone(), bm));
            }
        }
        let annotated: Vec<crate::cli::output::AnnotatedBookmark> = all
            .iter()
            .map(|(label, bm)| crate::cli::output::AnnotatedBookmark {
                source: label,
                bookmark: bm,
            })
            .collect();

        if needs_line {
            let bookmark_data: std::collections::HashMap<String, (String, String, String)> = all
                .iter()
                .map(|(label, bm)| {
                    (
                        crate::cli::output::short_id(&bm.id).to_string(),
                        (label.clone(), bm.id.clone(), bm.file_path.clone()),
                    )
                })
                .collect();

            let get_line_fn = |short_id: &str| -> Option<usize> {
                let (label, full_id, file_path) = bookmark_data.get(short_id)?;
                let db = db_map.get(label)?;
                get_bookmark_line(db, full_id, file_path)
            };

            crate::cli::output::write_annotated_bookmarks(
                mode,
                &annotated,
                args.line_format.as_deref(),
                Some(&get_line_fn),
            )?;
        } else {
            crate::cli::output::write_annotated_bookmarks(
                mode,
                &annotated,
                args.line_format.as_deref(),
                None as Option<&fn(&str) -> Option<usize>>,
            )?;
        }
    }
    Ok(())
}

/// Show the current location of a bookmark (file, line, byte range).
pub async fn handle_preview(cli: &Cli, args: &PreviewArgs) -> Result<()> {
    use crate::cli::output::ByteLocation;

    let dbs = open_all_dbs(cli)?;
    let id = extract_id(&args.id);
    let (bm, db) = find_bookmark_across(&dbs, id)?;

    // Determine which resolution to use
    let resolution = if let Some(ref res_id) = args.resolution_id {
        // Use specific resolution by ID
        db.get_resolution(res_id)?
            .ok_or_else(|| Error::Input(format!("resolution not found: {res_id}")))?
    } else if args.at_creation {
        // Find resolution at creation time
        let all = db.list_resolutions(&bm.id, 100)?;
        all.into_iter()
            .rev()
            .find(|r| r.commit_hash.as_deref() == bm.commit_hash.as_deref())
            .ok_or_else(|| Error::Resolution("no resolution found at creation time".into()))?
    } else if args.at_commit.is_some() {
        // Find resolution at specific commit
        let commit = args.at_commit.as_deref().unwrap();
        let all = db.list_resolutions(&bm.id, 100)?;
        all.into_iter()
            .rev()
            .find(|r| r.commit_hash.as_deref().map(|c| c.starts_with(commit)).unwrap_or(false))
            .ok_or_else(|| Error::Resolution(format!("no resolution found at commit {commit}")))?
    } else {
        // Default: use resolution from nearest ancestor commit
        // This allows preview to work correctly when checking out old commits
        let cwd = std::env::current_dir()?;
        let all_resolutions = db.list_resolutions(&bm.id, 100)?;

        // Extract commit hashes from all resolutions
        let commit_hashes: Vec<String> =
            all_resolutions.iter().filter_map(|r| r.commit_hash.clone()).collect();

        // Find the nearest ancestor commit
        match git_context::find_nearest_ancestor(&cwd, &commit_hashes)? {
            Some(nearest_commit) => {
                // Find the resolution with this commit hash
                all_resolutions
                    .into_iter()
                    .find(|r| r.commit_hash.as_deref() == Some(&nearest_commit))
                    .ok_or_else(|| {
                        Error::Resolution(format!(
                            "resolution for commit {nearest_commit} not found"
                        ))
                    })?
            }
            None => {
                // No ancestor found, fall back to most recent resolution
                let resolutions = db.list_resolutions(&bm.id, 1)?;
                match resolutions.first() {
                    Some(r) => r.clone(),
                    None => {
                        return Err(Error::Input(format!(
                            "bookmark {} has no resolution history; run `codemark resolve {}` first",
                            crate::cli::output::short_id(&bm.id),
                            crate::cli::output::short_id(&bm.id)
                        )));
                    }
                }
            }
        }
    };

    // Resolve the file path to absolute for output (but keep it relative in the database)
    let relative_path = resolution.file_path.as_ref().unwrap_or(&bm.file_path);
    let absolute_path = git_context::resolve_bookmark_file_path(relative_path, db.path())?;

    // Handle raw output mode
    if args.raw {
        let byte_range_str = resolution
            .byte_range
            .as_ref()
            .ok_or_else(|| Error::Input("no byte range available for this bookmark".to_string()))?;

        let byte_location = ByteLocation::from_str(byte_range_str)
            .ok_or_else(|| Error::Input(format!("invalid byte range format: {byte_range_str}")))?;

        let file_bytes = std::fs::read(&absolute_path).map_err(|e| {
            Error::Input(format!("failed to read file {}: {}", absolute_path.display(), e))
        })?;

        if byte_location.start_byte >= file_bytes.len()
            || byte_location.end_byte > file_bytes.len()
            || byte_location.start_byte > byte_location.end_byte
        {
            return Err(Error::Input(format!(
                "byte range {}:{} out of bounds for file (size: {})",
                byte_location.start_byte,
                byte_location.end_byte,
                file_bytes.len()
            )));
        }

        let content = &file_bytes[byte_location.start_byte..byte_location.end_byte];
        std::io::stdout().write_all(content)?;
        std::io::stdout().flush()?;
        return Ok(());
    }

    // Output JSON with resolution data (using standard envelope)
    let line_range_colon = resolution.line_range.as_ref().map(|r| r.replace('-', ":"));
    let data = serde_json::json!({
        "bookmark_id": bm.id,
        "file_path": absolute_path.to_string_lossy(),
        "line_range": resolution.line_range,
        "line_range_colon": line_range_colon,
        "byte_range": resolution.byte_range,
        "status": bm.health,
        "resolution_method": resolution.method,
        "resolved_at": resolution.resolved_at,
        "commit_hash": resolution.commit_hash,
        "content_hash": resolution.content_hash,
        "drifted": bm.health == BookmarkHealth::Drifted || bm.health == BookmarkHealth::Stale,
    });

    write_json_success(&data)?;
    Ok(())
}

/// Open a bookmarked file in the configured editor.
pub async fn handle_open(cli: &Cli, args: &OpenArgs) -> Result<()> {
    use crate::cli::output::short_id;
    use codemark_core::parser::languages::ParseCache;

    let db = open_db(cli)?;
    let config = load_config(cli);

    // Find the bookmark by ID or prefix
    let bookmark = match db.get_bookmark(&args.id) {
        Ok(Some(bm)) => bm,
        Ok(None) => {
            // Try prefix match
            match db.get_bookmark_by_prefix(&args.id) {
                Ok(Some(bm)) => bm,
                Ok(None) => return Err(Error::Input(format!("bookmark '{}' not found", args.id))),
                Err(e) => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };

    // Resolve the bookmark to get current location
    let lang = bookmark
        .language
        .parse::<Language>()
        .map_err(|_| Error::Input(format!("unsupported language: {}", bookmark.language)))?;
    let ts_lang = lang.tree_sitter_language();
    let mut cache = ParseCache::new(lang)?;
    let db_path = db.path();
    let provider = codemark_core::vfs::LocalFileProvider;
    let result = resolution::resolve(&bookmark, &mut cache, &ts_lang, db_path, &provider).await?;

    if result.method == ResolutionMethod::Failed {
        return Err(Error::Resolution(format!(
            "failed to resolve bookmark '{}': the code could not be found in the file",
            short_id(&bookmark.id)
        )));
    }

    // Get the absolute file path
    let absolute_path = git_context::resolve_bookmark_file_path(&result.file_path, db_path)?;

    // Extract file extension for command lookup
    let extension =
        std::path::Path::new(&result.file_path).extension().and_then(|e| e.to_str()).unwrap_or("");

    // Get the command template
    let command_template = if let Some(cmd) =
        config.open.get_command_for_extension(extension).or(config.open.default.as_ref())
    {
        // User configured a full command template
        cmd.clone()
    } else {
        // Use default: $EDITOR {FILE}
        // Note: We don't add line numbers since syntax varies by editor
        // Users should configure extension-specific commands for line number support
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
        format!("{} {{FILE}}", editor)
    };

    // Substitute placeholders
    let line_start = result.start_line + 1; // Convert to 1-indexed
    let line_end = result.end_line + 1; // Convert to 1-indexed
    let substituted = command_template
        .replace("{FILE}", &absolute_path.to_string_lossy())
        .replace("{LINE_START}", &line_start.to_string())
        .replace("{LINE_END}", &line_end.to_string())
        .replace("{ID}", &bookmark.id);

    // Tokenize the command safely
    let tokens = shlex::split(&substituted)
        .ok_or_else(|| Error::Input(format!("invalid command template: {}", command_template)))?;

    if tokens.is_empty() {
        return Err(Error::Input("empty command after tokenization".into()));
    }

    let program = &tokens[0];
    let args = &tokens[1..];

    // Determine if we should wait for the editor to complete
    let program_name =
        std::path::Path::new(program).file_name().and_then(|n| n.to_str()).unwrap_or("");
    let should_wait = config.open.should_wait_for_editor(program_name);

    if should_wait {
        // Wait for terminal editors to complete
        let status = tokio::process::Command::new(program)
            .args(args)
            .status()
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Error::Input(format!(
                        "editor not found: '{}'. Install the editor or configure a different command in codemark.toml",
                        program
                    ))
                } else {
                    Error::Operation(format!("failed to start editor: {}", e))
                }
            })?;

        if !status.success() {
            eprintln!("codemark: editor exited with status {}", status);
        }
    } else {
        // Spawn GUI editors in the background
        tokio::process::Command::new(program)
            .args(args)
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Error::Input(format!(
                        "editor not found: '{}'. Install the editor or configure a different command in codemark.toml",
                        program
                    ))
                } else {
                    Error::Operation(format!("failed to start editor: {}", e))
                }
            })?;
    }

    Ok(())
}
