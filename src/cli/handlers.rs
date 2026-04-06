use std::io::{self, Read, Write};
use std::path::PathBuf;

use clap::CommandFactory;
use clap_complete::generate;

use crate::cli::output::{
    self, short_id, write_bookmarks, write_json_success, write_not_implemented, write_success, OutputMode,
};
use crate::cli::*;
use crate::engine::bookmark::{
    Bookmark, BookmarkFilter, BookmarkStatus, Collection, Resolution, ResolutionMethod,
    tags_to_json,
};
use crate::engine::{hash, health, resolution};
use crate::error::{Error, Result};
use crate::git::context as git_context;
use crate::parser::languages::{Language, ParseCache};
use crate::query::generator as qgen;
use crate::config::Config;
use crate::storage::db::Database;

/// Dispatch a parsed CLI command to its handler.
pub fn dispatch(cli: &Cli) -> Result<()> {
    match &cli.command {
        Command::Show(args) => {
            let mode = OutputMode::resolve_with_default(cli.json, cli.format.as_deref(), true);
            handle_show(cli, &mode, args)
        }
        Command::Collection(args) => {
            let mode = match &args.command {
                CollectionCommand::List(_) | CollectionCommand::Show(_) => {
                    OutputMode::resolve_with_default(cli.json, cli.format.as_deref(), true)
                }
                _ => OutputMode::resolve(cli.json, cli.format.as_deref()),
            };
            dispatch_collection(cli, &mode, args)
        }
        _ => {
            let mode = OutputMode::resolve(cli.json, cli.format.as_deref());
            match &cli.command {
                Command::Add(args) => handle_add(cli, &mode, args),
                Command::AddFromSnippet(args) => handle_add_from_snippet(cli, &mode, args),
                Command::Resolve(args) => handle_resolve(cli, &mode, args),
                Command::Show(args) => handle_show(cli, &mode, args), // unreachable, handled above
                Command::Remove(args) => handle_remove(cli, &mode, args),
                Command::Validate(args) => handle_validate(cli, &mode, args),
                Command::Status => handle_status(cli, &mode),
                Command::List(args) => handle_list(cli, &mode, args),
                Command::Preview(args) => handle_preview(cli, args),
                Command::Search(args) => handle_search(cli, &mode, args),
                Command::Collection(args) => dispatch_collection(cli, &mode, args), // unreachable, handled above
                Command::Diff(args) => handle_diff(cli, &mode, args),
                Command::Gc(args) => handle_gc(cli, &mode, args),
                Command::Export(args) => handle_export(cli, args),
                Command::Import(args) => handle_import(cli, &mode, args),
                Command::Completions(args) => handle_completions(args),
            }
        }
    }
}

fn dispatch_collection(cli: &Cli, mode: &OutputMode, args: &CollectionArgs) -> Result<()> {
    match &args.command {
        CollectionCommand::Create(a) => handle_collection_create(cli, mode, a),
        CollectionCommand::Delete(a) => handle_collection_delete(cli, mode, a),
        CollectionCommand::Add(a) => handle_collection_add(cli, mode, a),
        CollectionCommand::Remove(a) => handle_collection_remove(cli, mode, a),
        CollectionCommand::List(a) => handle_collection_list(cli, mode, a),
        CollectionCommand::Show(a) => handle_collection_show(cli, mode, a),
        CollectionCommand::Resolve(a) => handle_collection_resolve(cli, mode, a),
        CollectionCommand::Reorder(a) => handle_collection_reorder(cli, mode, a),
    }
}

fn stub(mode: &OutputMode, name: &str) -> Result<()> {
    write_not_implemented(mode, name)?;
    Err(Error::NotImplemented(format!("{name}: not implemented")))
}

// --- Helpers ---

/// Open the primary database (for write commands, or single-db reads).
fn open_db(cli: &Cli) -> Result<Database> {
    if let Some(path) = cli.db.first() {
        return Database::open(path);
    }
    // Auto-detect from git root
    let cwd = std::env::current_dir()?;
    if let Some(ctx) = git_context::detect_context(&cwd) {
        let db_path = ctx.repo_root.join(".codemark").join("codemark.db");
        return Database::open(&db_path);
    }
    // Fallback: current directory
    let db_path = cwd.join(".codemark").join("codemark.db");
    Database::open(&db_path)
}

/// Load the config from the .codemark directory (same location as the primary DB).
fn load_config(cli: &Cli) -> Config {
    if let Some(path) = cli.db.first() {
        if let Some(parent) = path.parent() {
            return Config::load(parent);
        }
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    if let Some(ctx) = git_context::detect_context(&cwd) {
        return Config::load(&ctx.repo_root.join(".codemark"));
    }
    Config::load(&cwd.join(".codemark"))
}

/// Open all specified databases (for read commands that support cross-repo queries).
/// Returns (source_label, database) pairs. Falls back to single auto-detected db.
fn open_all_dbs(cli: &Cli) -> Result<Vec<(String, Database)>> {
    if cli.db.len() <= 1 {
        let db = open_db(cli)?;
        let label = source_label_from_cli(cli);
        return Ok(vec![(label, db)]);
    }
    let mut dbs = Vec::new();
    for path in &cli.db {
        let label = source_label_from_path(path);
        dbs.push((label, Database::open(path)?));
    }
    Ok(dbs)
}

/// Returns true if multiple databases are being queried.
fn is_multi_db(cli: &Cli) -> bool {
    cli.db.len() > 1
}

/// Derive a source label from a db path: /foo/repo-name/.codemark/codemark.db -> "repo-name"
fn source_label_from_path(path: &std::path::Path) -> String {
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
/// - "42:67" — line range (inclusive)
/// - "b42:67" or "42:67b" — byte range (explicit)
fn parse_range(range: &str, source: &str) -> Result<(usize, usize)> {
    let r = range.trim();

    // Byte range: b prefix or b suffix
    if r.starts_with('b') || r.starts_with('B') {
        return parse_byte_range(&r[1..]);
    }
    if r.ends_with('b') || r.ends_with('B') {
        return parse_byte_range(&r[..r.len() - 1]);
    }

    // Line range
    let parts: Vec<&str> = r.split(':').collect();
    match parts.len() {
        1 => {
            let line: usize = parts[0]
                .parse()
                .map_err(|_| Error::Input(format!("invalid line number: {}", parts[0])))?;
            line_range_to_bytes(source, line, line)
        }
        2 => {
            let start: usize = parts[0]
                .parse()
                .map_err(|_| Error::Input(format!("invalid start line: {}", parts[0])))?;
            let end: usize = parts[1]
                .parse()
                .map_err(|_| Error::Input(format!("invalid end line: {}", parts[1])))?;
            line_range_to_bytes(source, start, end)
        }
        _ => Err(Error::Input("range must be LINE, LINE:LINE, or bBYTE:BYTE".into())),
    }
}

fn parse_byte_range(s: &str) -> Result<(usize, usize)> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return Err(Error::Input("byte range must be start:end".into()));
    }
    let start: usize = parts[0]
        .parse()
        .map_err(|_| Error::Input("invalid byte range start".into()))?;
    let end: usize = parts[1]
        .parse()
        .map_err(|_| Error::Input("invalid byte range end".into()))?;
    Ok((start, end))
}

/// Convert 1-indexed inclusive line range to byte range.
fn line_range_to_bytes(source: &str, start_line: usize, end_line: usize) -> Result<(usize, usize)> {
    if start_line == 0 || end_line == 0 {
        return Err(Error::Input("line numbers are 1-indexed".into()));
    }
    if start_line > end_line {
        return Err(Error::Input(format!("start line {start_line} > end line {end_line}")));
    }

    let mut byte_offset = 0;
    let mut start_byte = None;
    let mut end_byte = None;

    for (i, line) in source.lines().enumerate() {
        let line_num = i + 1;
        if line_num == start_line {
            start_byte = Some(byte_offset);
        }
        byte_offset += line.len() + 1; // +1 for newline
        if line_num == end_line {
            end_byte = Some(byte_offset.min(source.len()));
            break;
        }
    }

    match (start_byte, end_byte) {
        (Some(s), Some(e)) => Ok((s, e)),
        _ => Err(Error::Input(format!(
            "line range {start_line}:{end_line} is out of bounds (file has {} lines)",
            source.lines().count()
        ))),
    }
}

/// Parse a git diff hunk header to extract the new-side line range.
/// Format: @@ -old_start[,old_count] +new_start[,new_count] @@ [context]
fn parse_hunk(hunk: &str) -> Result<(usize, usize)> {
    let re = regex::Regex::new(r"\+(\d+)(?:,(\d+))?").unwrap();
    let caps = re
        .captures(hunk)
        .ok_or_else(|| Error::Input(format!("invalid hunk format: {hunk}")))?;
    let start: usize = caps[1]
        .parse()
        .map_err(|_| Error::Input("invalid hunk start".into()))?;
    let count: usize = caps
        .get(2)
        .map(|m| m.as_str().parse().unwrap_or(1))
        .unwrap_or(1);
    let end = start + count.saturating_sub(1);
    Ok((start, end.max(start)))
}

/// Resolve the language from --lang flag or file extension.
fn resolve_language(lang_flag: Option<&str>, file: &std::path::Path) -> Result<Language> {
    if let Some(lang) = lang_flag {
        return lang.parse();
    }
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .ok_or_else(|| Error::Input(format!(
            "cannot infer language from '{}'; use --lang to specify",
            file.display()
        )))?;
    Language::from_extension(ext).ok_or_else(|| {
        Error::Input(format!(
            "cannot infer language from extension '.{ext}'; use --lang to specify"
        ))
    })
}

fn parse_status_filter(status: Option<&str>) -> Option<Vec<BookmarkStatus>> {
    status.map(|s| {
        s.split(',')
            .filter_map(|part| part.trim().parse().ok())
            .collect()
    })
}

fn resolve_file_path(file: &std::path::Path) -> Result<(PathBuf, String)> {
    let abs = if file.is_absolute() {
        file.to_path_buf()
    } else {
        std::env::current_dir()?.join(file)
    };
    if !abs.exists() {
        return Err(Error::Input(format!("file not found: {}", file.display())));
    }
    let cwd = std::env::current_dir()?;
    let rel = if let Some(ctx) = git_context::detect_context(&cwd) {
        git_context::relative_to_root(&ctx.repo_root, &abs)?
    } else {
        file.to_string_lossy().to_string()
    };
    Ok((abs, rel))
}

fn extract_id(id_or_line: &str) -> &str {
    // If it looks like a line-format string (contains tabs), extract the first field
    id_or_line.split('\t').next().unwrap_or(id_or_line)
}

/// Get the start line number from a bookmark's latest resolution.
/// Returns the line number (1-indexed) or None if no resolution exists.
/// Note: bookmark_id should be the full ID, not a short prefix.
fn get_bookmark_line(db: &Database, bookmark_id: &str, _file_path: &str) -> Option<usize> {
    // Get the latest resolution (limit 1)
    let resolutions = db.list_resolutions(bookmark_id, 1).ok()?;
    let res = resolutions.first()?;

    // Parse the line_range "start:end" (1-indexed, inclusive)
    let line_range = res.line_range.as_ref()?;
    let parts: Vec<&str> = line_range.split(':').collect();
    if parts.len() != 2 {
        return None;
    }

    parts[0].parse().ok()
}

fn find_bookmark(db: &Database, id: &str) -> Result<Bookmark> {
    let id = extract_id(id);
    // Try exact match first, then prefix
    if let Some(bm) = db.get_bookmark(id)? {
        return Ok(bm);
    }
    db.get_bookmark_by_prefix(id)?
        .ok_or_else(|| Error::Input(format!("bookmark not found: {id}")))
}

/// Search for a bookmark across multiple databases. Returns the bookmark and a reference to the DB.
fn find_bookmark_across<'a>(
    dbs: &'a [(String, Database)],
    id: &str,
) -> Result<(Bookmark, &'a Database)> {
    let id = extract_id(id);
    for (_label, db) in dbs {
        if let Some(bm) = db.get_bookmark(id)? {
            return Ok((bm, db));
        }
        if id.len() >= 4 {
            if let Ok(Some(bm)) = db.get_bookmark_by_prefix(id) {
                return Ok((bm, db));
            }
        }
    }
    Err(Error::Input(format!("bookmark not found: {id}")))
}

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

// --- Command handlers ---

fn handle_add(cli: &Cli, mode: &OutputMode, args: &AddArgs) -> Result<()> {
    let lang = resolve_language(args.lang.as_deref(), &args.file)?;
    let (abs_path, rel_path) = resolve_file_path(&args.file)?;

    let mut parser = crate::parser::languages::Parser::new(lang)?;
    let (tree, source) = parser.parse_file(&abs_path)?;
    let ts_lang = lang.tree_sitter_language();

    // Resolve byte range from --range or --hunk
    let byte_range = if let Some(ref hunk) = args.hunk {
        let (start_line, end_line) = parse_hunk(hunk)?;
        line_range_to_bytes(&source, start_line, end_line)?
    } else if let Some(ref range) = args.range {
        parse_range(range, &source)?
    } else {
        return Err(Error::Input("either --range or --hunk is required".into()));
    };

    let generated = qgen::generate_query(&tree, source.as_bytes(), byte_range, &ts_lang)?;
    let content_hash =
        hash::content_hash(&source[generated.byte_range.0..generated.byte_range.1]);

    // Count matches for uniqueness info
    let match_count = crate::query::matcher::run_query(
        &generated.query, &tree, source.as_bytes(), &ts_lang,
    )
    .map(|m| m.len())
    .unwrap_or(0);

    // Compute the line range of the target for display
    let target_start_line = source[..generated.byte_range.0].lines().count() + 1;
    let target_end_line = source[..generated.byte_range.1].lines().count();

    if args.dry_run {
        return write_dry_run(mode, &generated, &content_hash, &rel_path, target_start_line, target_end_line, match_count);
    }

    let db = open_db(cli)?;
    let cwd = std::env::current_dir()?;
    let commit_hash = git_context::detect_context(&cwd).and_then(|ctx| ctx.head_commit);

    let bookmark = Bookmark {
        id: uuid::Uuid::new_v4().to_string(),
        query: generated.query.clone(),
        language: lang.to_string(),
        file_path: rel_path,
        content_hash: Some(content_hash.clone()),
        commit_hash,
        status: BookmarkStatus::Active,
        resolution_method: Some(ResolutionMethod::Exact),
        last_resolved_at: Some(now_iso()),
        stale_since: None,
        created_at: now_iso(),
        created_by: Some(args.created_by.clone()),
        tags: args.tag.clone(),
        notes: args.note.clone(),
        context: args.context.clone(),
    };

    db.insert_bookmark(&bookmark)?;

    // Record initial resolution as baseline
    let config = load_config(cli);
    let initial_res = Resolution {
        id: uuid::Uuid::new_v4().to_string(),
        bookmark_id: bookmark.id.clone(),
        resolved_at: now_iso(),
        commit_hash: bookmark.commit_hash.clone(),
        method: ResolutionMethod::Exact,
        match_count: Some(match_count as i32),
        file_path: Some(bookmark.file_path.clone()),
        byte_range: Some(format!("{}:{}", generated.byte_range.0, generated.byte_range.1)),
        line_range: Some(format!("{}:{}", target_start_line, target_end_line)),
        content_hash: Some(content_hash.clone()),
    };
    db.insert_resolution_if_changed(&initial_res, config.storage.max_resolutions_per_bookmark)?;

    match mode {
        OutputMode::Json => write_json_success(&serde_json::json!({
            "id": bookmark.id,
            "query": generated.query,
            "node_type": generated.target_node_type,
            "name": generated.target_name,
            "lines": format!("{target_start_line}-{target_end_line}"),
            "content_hash": content_hash,
            "unique": match_count == 1,
            "created_by": bookmark.created_by,
        }))?,
        _ => {
            println!("Bookmark created: {}", output::short_id(&bookmark.id));
            println!("  Node type: {}", generated.target_node_type);
            if let Some(ref name) = generated.target_name {
                println!("  Target: {name}");
            }
            println!("  Lines: {target_start_line}-{target_end_line}");
        }
    }
    Ok(())
}

fn handle_add_from_snippet(cli: &Cli, mode: &OutputMode, args: &AddFromSnippetArgs) -> Result<()> {
    let lang = resolve_language(args.lang.as_deref(), &args.file)?;
    let (abs_path, rel_path) = resolve_file_path(&args.file)?;

    // Read snippet from stdin
    let mut snippet = String::new();
    io::stdin().read_to_string(&mut snippet)?;
    let snippet = snippet.trim();
    if snippet.is_empty() {
        return Err(Error::Input("no snippet provided on stdin".into()));
    }

    let mut parser = crate::parser::languages::Parser::new(lang)?;
    let (tree, source) = parser.parse_file(&abs_path)?;
    let ts_lang = lang.tree_sitter_language();

    let offset = source
        .find(snippet)
        .ok_or_else(|| Error::Input("snippet not found in file".into()))?;
    let byte_range = (offset, offset + snippet.len());

    let generated = qgen::generate_query(&tree, source.as_bytes(), byte_range, &ts_lang)?;
    let content_hash =
        hash::content_hash(&source[generated.byte_range.0..generated.byte_range.1]);

    let match_count = crate::query::matcher::run_query(
        &generated.query, &tree, source.as_bytes(), &ts_lang,
    )
    .map(|m| m.len())
    .unwrap_or(0);

    let target_start_line = source[..generated.byte_range.0].lines().count() + 1;
    let target_end_line = source[..generated.byte_range.1].lines().count();

    if args.dry_run {
        return write_dry_run(mode, &generated, &content_hash, &rel_path, target_start_line, target_end_line, match_count);
    }

    let db = open_db(cli)?;
    let cwd = std::env::current_dir()?;
    let commit_hash = git_context::detect_context(&cwd).and_then(|ctx| ctx.head_commit);

    let bookmark = Bookmark {
        id: uuid::Uuid::new_v4().to_string(),
        query: generated.query.clone(),
        language: lang.to_string(),
        file_path: rel_path,
        content_hash: Some(content_hash.clone()),
        commit_hash,
        status: BookmarkStatus::Active,
        resolution_method: Some(ResolutionMethod::Exact),
        last_resolved_at: Some(now_iso()),
        stale_since: None,
        created_at: now_iso(),
        created_by: Some(args.created_by.clone()),
        tags: args.tag.clone(),
        notes: args.note.clone(),
        context: args.context.clone(),
    };

    db.insert_bookmark(&bookmark)?;

    // Record initial resolution as baseline
    let config = load_config(cli);
    let initial_res = Resolution {
        id: uuid::Uuid::new_v4().to_string(),
        bookmark_id: bookmark.id.clone(),
        resolved_at: now_iso(),
        commit_hash: bookmark.commit_hash.clone(),
        method: ResolutionMethod::Exact,
        match_count: Some(match_count as i32),
        file_path: Some(bookmark.file_path.clone()),
        byte_range: Some(format!("{}:{}", generated.byte_range.0, generated.byte_range.1)),
        line_range: Some(format!("{}:{}", target_start_line, target_end_line)),
        content_hash: Some(content_hash.clone()),
    };
    db.insert_resolution_if_changed(&initial_res, config.storage.max_resolutions_per_bookmark)?;

    match mode {
        OutputMode::Json => write_json_success(&serde_json::json!({
            "id": bookmark.id,
            "query": generated.query,
            "node_type": generated.target_node_type,
            "name": generated.target_name,
            "content_hash": content_hash,
            "created_by": bookmark.created_by,
        }))?,
        _ => {
            println!("Bookmark created: {}", output::short_id(&bookmark.id));
            if let Some(ref name) = generated.target_name {
                println!("  Target: {name}");
            }
        }
    }
    Ok(())
}

fn write_dry_run(
    mode: &OutputMode,
    generated: &qgen::GeneratedQuery,
    content_hash: &str,
    file_path: &str,
    start_line: usize,
    end_line: usize,
    match_count: usize,
) -> Result<()> {
    match mode {
        OutputMode::Json => {
            write_json_success(&serde_json::json!({
                "dry_run": true,
                "node_type": generated.target_node_type,
                "name": generated.target_name,
                "file": file_path,
                "lines": format!("{start_line}-{end_line}"),
                "query": generated.query,
                "content_hash": content_hash,
                "unique": match_count == 1,
                "match_count": match_count,
            }))?;
        }
        _ => {
            println!("Dry run — bookmark would target:\n");
            println!("  Node type:  {}", generated.target_node_type);
            if let Some(ref name) = generated.target_name {
                println!("  Name:       {name}");
            }
            println!("  File:       {file_path}");
            println!("  Lines:      {start_line}-{end_line}");
            println!("  Hash:       {content_hash}");
            println!(
                "  Unique:     {} ({match_count} match{})",
                if match_count == 1 { "yes" } else { "no" },
                if match_count == 1 { "" } else { "es" }
            );
            println!("\n  Query:");
            for line in generated.query.lines() {
                println!("    {line}");
            }
            println!("\nNo bookmark created. Remove --dry-run to save.");
        }
    }
    Ok(())
}

fn handle_resolve(cli: &Cli, mode: &OutputMode, args: &ResolveArgs) -> Result<()> {
    let dbs = open_all_dbs(cli)?;

    if let Some(ref id) = args.id {
        // Single bookmark resolution — search across all DBs
        let (bm, db) = find_bookmark_across(&dbs, id)?;
        let lang: Language = bm.language.parse()?;
        let mut cache = ParseCache::new(lang)?;
        let ts_lang = lang.tree_sitter_language();

        let result = resolution::resolve(&bm, &mut cache, &ts_lang)?;
        let new_status = health::transition(bm.status, result.method, result.hash_matches);

        let stale_since = if new_status == BookmarkStatus::Stale {
            bm.stale_since.clone().or_else(|| Some(now_iso()))
        } else {
            None
        };

        db.update_bookmark_status(
            &bm.id,
            new_status,
            Some(result.method),
            Some(&now_iso()),
            stale_since.as_deref(),
        )?;

        if let Some(ref new_query) = result.new_query {
            db.update_bookmark_query(&bm.id, new_query, &result.file_path, &result.content_hash)?;
        }

        // Record resolution (deduped — skips if same commit + location + method)
        let res = Resolution {
            id: uuid::Uuid::new_v4().to_string(),
            bookmark_id: bm.id.clone(),
            resolved_at: now_iso(),
            commit_hash: git_context::detect_context(&std::env::current_dir()?)
                .and_then(|ctx| ctx.head_commit),
            method: result.method,
            match_count: Some(1),
            file_path: Some(result.file_path.clone()),
            byte_range: Some(format!("{}:{}", result.byte_range.0, result.byte_range.1)),
            line_range: Some(format!("{}:{}", result.start_line + 1, result.end_line + 1)),
            content_hash: Some(result.content_hash.clone()),
        };
        let config = load_config(cli);
        db.insert_resolution_if_changed(&res, config.storage.max_resolutions_per_bookmark)?;

        write_resolution_output(mode, &bm, &result)?;
    } else {
        // Batch resolution — fan out across all DBs
        let filter = BookmarkFilter {
            tag: args.tag.clone(),
            status: parse_status_filter(args.status.as_deref())
                .or(Some(vec![BookmarkStatus::Active, BookmarkStatus::Drifted])),
            file_path: args.file.as_ref().map(|p| p.to_string_lossy().to_string()),
            language: args.lang.clone(),
            collection: args.collection.clone(),
            ..Default::default()
        };
        let config = load_config(cli);
        for (_label, db) in &dbs {
            let bookmarks = db.list_bookmarks(&filter)?;
            resolve_batch(mode, db, &bookmarks, &config)?;
        }
    }
    Ok(())
}

fn handle_show(cli: &Cli, mode: &OutputMode, args: &ShowArgs) -> Result<()> {
    let dbs = open_all_dbs(cli)?;
    let (bm, db) = find_bookmark_across(&dbs, &args.id)?;
    let resolutions = db.list_resolutions(&bm.id, 5)?;

    match mode {
        OutputMode::Json => {
            write_json_success(&serde_json::json!({
                "bookmark": bm,
                "resolutions": resolutions,
            }))?;
        }
        _ => {
            println!("ID:          {}", bm.id);
            println!("File:        {}", bm.file_path);
            println!("Language:    {}", bm.language);
            println!("Status:      {}", bm.status);
            if !bm.tags.is_empty() {
                println!("Tags:        {}", bm.tags.join(", "));
            }
            if let Some(ref note) = bm.notes {
                println!("Note:        {note}");
            }
            if let Some(ref ctx) = bm.context {
                println!("Context:     {ctx}");
            }
            if let Some(ref method) = bm.resolution_method {
                println!("Resolution:  {method}");
            }
            if let Some(ref resolved) = bm.last_resolved_at {
                println!("Resolved at: {resolved}");
            }
            if let Some(ref commit) = bm.commit_hash {
                println!("Commit:      {}", &commit[..commit.len().min(8)]);
            }
            println!("Created:     {}", bm.created_at);
            println!("\nQuery:");
            println!("{}", bm.query);

            if !resolutions.is_empty() {
                println!("\nResolution history:");
                for r in &resolutions {
                    println!(
                        "  {} | {} | {}",
                        r.resolved_at,
                        r.method,
                        r.file_path.as_deref().unwrap_or("-")
                    );
                }
            }
        }
    }
    Ok(())
}

fn handle_remove(cli: &Cli, mode: &OutputMode, args: &RemoveArgs) -> Result<()> {
    let db = open_db(cli)?;
    let mut removed = 0;
    let mut not_found = 0;

    for id_input in &args.ids {
        let id = extract_id(id_input);
        match find_bookmark(&db, id) {
            Ok(bm) => {
                db.delete_bookmark(&bm.id)?;
                removed += 1;
            }
            Err(_) => {
                not_found += 1;
                eprintln!("codemark: bookmark not found: {id}");
            }
        }
    }

    write_success(
        mode,
        &format!("Removed {removed} bookmark{}", if removed == 1 { "" } else { "s" }),
    )?;

    if not_found > 0 {
        return Err(Error::Input(format!("{not_found} bookmark(s) not found")));
    }
    Ok(())
}

fn handle_validate(cli: &Cli, mode: &OutputMode, args: &ValidateArgs) -> Result<()> {
    let db = open_db(cli)?;
    let filter = BookmarkFilter {
        file_path: args.file.as_ref().map(|p| p.to_string_lossy().to_string()),
        language: args.lang.clone(),
        status: Some(vec![
            BookmarkStatus::Active,
            BookmarkStatus::Drifted,
            BookmarkStatus::Stale,
        ]),
        collection: args.collection.clone(),
        ..Default::default()
    };
    let bookmarks = db.list_bookmarks(&filter)?;

    let mut active = 0u32;
    let mut drifted = 0u32;
    let mut stale = 0u32;
    let mut archived = 0u32;

    for bm in &bookmarks {
        let Ok(lang) = bm.language.parse::<Language>() else {
            continue;
        };
        let mut cache = ParseCache::new(lang)?;
        let ts_lang = lang.tree_sitter_language();
        let result = resolution::resolve(bm, &mut cache, &ts_lang)?;
        let new_status = health::transition(bm.status, result.method, result.hash_matches);

        let stale_since = if new_status == BookmarkStatus::Stale {
            bm.stale_since.clone().or_else(|| Some(now_iso()))
        } else {
            None
        };

        // Auto-archive check
        let final_status = if args.auto_archive
            && new_status == BookmarkStatus::Stale
            && bm
                .stale_since
                .as_deref()
                .is_some_and(|s| health::should_auto_archive(s, args.archive_after))
        {
            BookmarkStatus::Archived
        } else {
            new_status
        };

        db.update_bookmark_status(
            &bm.id,
            final_status,
            Some(result.method),
            Some(&now_iso()),
            stale_since.as_deref(),
        )?;

        if let Some(ref new_query) = result.new_query {
            db.update_bookmark_query(&bm.id, new_query, &result.file_path, &result.content_hash)?;
        }

        match final_status {
            BookmarkStatus::Active => active += 1,
            BookmarkStatus::Drifted => drifted += 1,
            BookmarkStatus::Stale => stale += 1,
            BookmarkStatus::Archived => archived += 1,
        }
    }

    match mode {
        OutputMode::Json => {
            write_json_success(&serde_json::json!({
                "active": active,
                "drifted": drifted,
                "stale": stale,
                "archived": archived,
                "total": bookmarks.len(),
            }))?;
        }
        _ => {
            println!(
                "Validated {} bookmarks: {} active, {} drifted, {} stale, {} archived",
                bookmarks.len(),
                active,
                drifted,
                stale,
                archived
            );
        }
    }
    Ok(())
}

fn handle_status(cli: &Cli, mode: &OutputMode) -> Result<()> {
    let dbs = open_all_dbs(cli)?;
    let multi = dbs.len() > 1;

    let mut total_active = 0usize;
    let mut total_drifted = 0usize;
    let mut total_stale = 0usize;
    let mut total_archived = 0usize;

    for (label, db) in &dbs {
        let counts = db.count_by_status()?;
        let a = counts.get(&BookmarkStatus::Active).copied().unwrap_or(0);
        let d = counts.get(&BookmarkStatus::Drifted).copied().unwrap_or(0);
        let s = counts.get(&BookmarkStatus::Stale).copied().unwrap_or(0);
        let r = counts.get(&BookmarkStatus::Archived).copied().unwrap_or(0);

        if multi {
            match mode {
                OutputMode::Json => {}
                _ => println!("[{label}] {a} active  |  {d} drifted  |  {s} stale  |  {r} archived"),
            }
        }

        total_active += a;
        total_drifted += d;
        total_stale += s;
        total_archived += r;
    }

    match mode {
        OutputMode::Json => {
            write_json_success(&serde_json::json!({
                "active": total_active,
                "drifted": total_drifted,
                "stale": total_stale,
                "archived": total_archived,
            }))?;
        }
        _ => {
            if multi {
                println!("---");
            }
            println!("{total_active} active  |  {total_drifted} drifted  |  {total_stale} stale  |  {total_archived} archived");
        }
    }
    Ok(())
}

fn handle_list(cli: &Cli, mode: &OutputMode, args: &ListArgs) -> Result<()> {
    let dbs = open_all_dbs(cli)?;
    let filter = BookmarkFilter {
        tag: args.tag.clone(),
        status: parse_status_filter(args.status.as_deref())
            .or(Some(vec![BookmarkStatus::Active, BookmarkStatus::Drifted])),
        file_path: args.file.as_ref().map(|p| p.to_string_lossy().to_string()),
        language: args.lang.clone(),
        created_by: args.author.clone(),
        collection: args.collection.clone(),
        limit: args.limit,
    };

    if dbs.len() == 1 {
        let bookmarks = dbs[0].1.list_bookmarks(&filter)?;
        // For Tv mode, provide a closure to fetch line numbers
        if matches!(mode, OutputMode::Tv) {
            let db = &dbs[0].1;
            // Capture both full IDs and file paths
            let bookmark_data: std::collections::HashMap<String, (String, String)> = bookmarks
                .iter()
                .map(|bm| (short_id(&bm.id).to_string(), (bm.id.clone(), bm.file_path.clone())))
                .collect();
            output::write_bookmarks_with_line(mode, &bookmarks, |short_id| {
                let (full_id, file_path) = bookmark_data.get(short_id)?;
                get_bookmark_line(db, full_id, file_path)
            })?;
        } else {
            write_bookmarks(mode, &bookmarks)?;
        }
    } else {
        let mut all = Vec::new();
        for (label, db) in &dbs {
            let bookmarks = db.list_bookmarks(&filter)?;
            for bm in bookmarks {
                all.push((label.clone(), bm));
            }
        }
        let annotated: Vec<output::AnnotatedBookmark> = all
            .iter()
            .map(|(label, bm)| output::AnnotatedBookmark { source: label, bookmark: bm })
            .collect();
        output::write_annotated_bookmarks(mode, &annotated)?;
    }
    Ok(())
}

fn handle_preview(cli: &Cli, args: &PreviewArgs) -> Result<()> {
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
        // Default: most recent resolution
        let resolutions = db.list_resolutions(&bm.id, 1)?;
        match resolutions.first() {
            Some(r) => r.clone(),
            None => {
                return Err(Error::Input(format!(
                    "bookmark {} has no resolution history; run `codemark resolve {}` first",
                    output::short_id(&bm.id),
                    output::short_id(&bm.id)
                )));
            }
        }
    };

    // Output JSON with resolution data (using standard envelope)
    let data = serde_json::json!({
        "bookmark_id": bm.id,
        "file_path": resolution.file_path.as_ref().unwrap_or(&bm.file_path),
        "line_range": resolution.line_range,
        "byte_range": resolution.byte_range,
        "status": bm.status,
        "resolution_method": resolution.method,
        "resolved_at": resolution.resolved_at,
        "commit_hash": resolution.commit_hash,
        "content_hash": resolution.content_hash,
        "drifted": bm.status == BookmarkStatus::Drifted || bm.status == BookmarkStatus::Stale,
    });

    write_json_success(&data)?;
    Ok(())
}

fn handle_search(cli: &Cli, mode: &OutputMode, args: &SearchArgs) -> Result<()> {
    let dbs = open_all_dbs(cli)?;

    if dbs.len() == 1 {
        let bookmarks = dbs[0].1.search_bookmarks(
            args.query.as_deref(),
            args.note.as_deref(),
            args.context.as_deref(),
            args.lang.as_deref(),
            args.author.as_deref(),
            args.collection.as_deref(),
        )?;
        if matches!(mode, OutputMode::Tv) {
            let db = &dbs[0].1;
            let bookmark_data: std::collections::HashMap<String, (String, String)> = bookmarks
                .iter()
                .map(|bm| (short_id(&bm.id).to_string(), (bm.id.clone(), bm.file_path.clone())))
                .collect();
            output::write_bookmarks_with_line(mode, &bookmarks, |short_id| {
                let (full_id, file_path) = bookmark_data.get(short_id)?;
                get_bookmark_line(db, full_id, file_path)
            })?;
        } else {
            write_bookmarks(mode, &bookmarks)?;
        }
    } else {
        let mut all = Vec::new();
        for (label, db) in &dbs {
            let bookmarks = db.search_bookmarks(
                args.query.as_deref(),
                args.note.as_deref(),
                args.context.as_deref(),
                args.lang.as_deref(),
                args.author.as_deref(),
                args.collection.as_deref(),
            )?;
            for bm in bookmarks {
                all.push((label.clone(), bm));
            }
        }
        let annotated: Vec<output::AnnotatedBookmark> = all
            .iter()
            .map(|(label, bm)| output::AnnotatedBookmark { source: label, bookmark: bm })
            .collect();
        output::write_annotated_bookmarks(mode, &annotated)?;
    }
    Ok(())
}

fn handle_diff(cli: &Cli, mode: &OutputMode, args: &DiffArgs) -> Result<()> {
    let db = open_db(cli)?;
    let cwd = std::env::current_dir()?;

    let since = args
        .since
        .as_deref()
        .unwrap_or("HEAD~1");

    let changed_files = git_context::changed_files_since(&cwd, since)?;
    if changed_files.is_empty() {
        write_success(mode, "No files changed.")?;
        return Ok(());
    }

    // Find bookmarks that reference changed files
    let all_bookmarks = db.list_bookmarks(&BookmarkFilter {
        status: Some(vec![BookmarkStatus::Active, BookmarkStatus::Drifted, BookmarkStatus::Stale]),
        ..Default::default()
    })?;

    let affected: Vec<&Bookmark> = all_bookmarks
        .iter()
        .filter(|bm| changed_files.iter().any(|f| *f == bm.file_path))
        .collect();

    if affected.is_empty() {
        write_success(mode, &format!(
            "{} files changed since {since}, no bookmarks affected.",
            changed_files.len()
        ))?;
        return Ok(());
    }

    // Resolve affected bookmarks and report
    match mode {
        OutputMode::Json => {
            let mut results = Vec::new();
            for bm in &affected {
                let Ok(lang) = bm.language.parse::<Language>() else { continue };
                let mut cache = ParseCache::new(lang)?;
                let ts_lang = lang.tree_sitter_language();
                let result = resolution::resolve(bm, &mut cache, &ts_lang)?;
                let new_status = health::transition(bm.status, result.method, result.hash_matches);
                results.push(serde_json::json!({
                    "id": bm.id,
                    "file": bm.file_path,
                    "status_before": bm.status.to_string(),
                    "status_after": new_status.to_string(),
                    "method": result.method.to_string(),
                    "line": result.start_line + 1,
                }));
            }
            write_json_success(&results)?;
        }
        _ => {
            println!(
                "{} files changed since {since}, {} bookmarks affected:\n",
                changed_files.len(),
                affected.len()
            );
            for bm in &affected {
                let Ok(lang) = bm.language.parse::<Language>() else { continue };
                let mut cache = ParseCache::new(lang)?;
                let ts_lang = lang.tree_sitter_language();
                let result = resolution::resolve(bm, &mut cache, &ts_lang)?;
                let new_status = health::transition(bm.status, result.method, result.hash_matches);
                let status_change = if new_status != bm.status {
                    format!("{} -> {}", bm.status, new_status)
                } else {
                    new_status.to_string()
                };
                println!(
                    "  {}  {}:{}  [{}]  {}",
                    output::short_id(&bm.id),
                    result.file_path,
                    result.start_line + 1,
                    result.method,
                    status_change
                );
                if let Some(ref note) = bm.notes {
                    println!("    {note}");
                }
            }
        }
    }
    Ok(())
}

fn handle_gc(cli: &Cli, mode: &OutputMode, args: &GcArgs) -> Result<()> {
    let db = open_db(cli)?;
    let days = parse_duration_days(&args.older_than)?;
    let cutoff = chrono::Utc::now() - chrono::Duration::days(days);
    let cutoff_str = cutoff.format("%Y-%m-%dT%H:%M:%SZ").to_string();

    if args.dry_run {
        let filter = BookmarkFilter {
            status: Some(vec![BookmarkStatus::Archived]),
            ..Default::default()
        };
        let bookmarks = db.list_bookmarks(&filter)?;
        let would_remove: Vec<_> = bookmarks
            .iter()
            .filter(|b| b.created_at < cutoff_str)
            .collect();
        write_success(
            mode,
            &format!("Would remove {} archived bookmarks", would_remove.len()),
        )?;
    } else {
        let count = db.delete_archived_before(&cutoff_str)?;
        write_success(mode, &format!("Removed {count} archived bookmarks"))?;
    }
    Ok(())
}

fn handle_export(cli: &Cli, args: &ExportArgs) -> Result<()> {
    let dbs = open_all_dbs(cli)?;
    let filter = BookmarkFilter {
        tag: args.tag.clone(),
        status: parse_status_filter(args.status.as_deref()),
        ..Default::default()
    };
    let mut bookmarks = Vec::new();
    for (_label, db) in &dbs {
        bookmarks.extend(db.list_bookmarks(&filter)?);
    }

    match args.export_format {
        ExportFormat::Json => {
            let json = serde_json::to_string_pretty(&bookmarks)?;
            println!("{json}");
        }
        ExportFormat::Csv => {
            println!("id,file_path,language,status,tags,notes");
            for bm in &bookmarks {
                println!(
                    "{},{},{},{},{},{}",
                    bm.id,
                    bm.file_path,
                    bm.language,
                    bm.status,
                    tags_to_json(&bm.tags),
                    bm.notes.as_deref().unwrap_or("")
                );
            }
        }
    }
    Ok(())
}

fn handle_import(cli: &Cli, mode: &OutputMode, args: &ImportArgs) -> Result<()> {
    let db = open_db(cli)?;
    let content = std::fs::read_to_string(&args.file)
        .map_err(|e| Error::Input(format!("cannot read {}: {e}", args.file.display())))?;
    let bookmarks: Vec<Bookmark> = serde_json::from_str(&content)?;

    let mut imported = 0;
    let mut skipped = 0;
    for bm in &bookmarks {
        if db.get_bookmark(&bm.id)?.is_some() {
            skipped += 1;
        } else {
            db.insert_bookmark(bm)?;
            imported += 1;
        }
    }

    write_success(
        mode,
        &format!("Imported {imported} bookmarks ({skipped} duplicates skipped)"),
    )?;
    Ok(())
}

fn handle_completions(args: &CompletionsArgs) -> Result<()> {
    let mut cmd = Cli::command();
    generate(args.shell, &mut cmd, "codemark", &mut io::stdout());
    Ok(())
}

// --- Collection handlers ---

fn handle_collection_create(cli: &Cli, mode: &OutputMode, args: &CollectionCreateArgs) -> Result<()> {
    let db = open_db(cli)?;
    let collection = Collection {
        id: uuid::Uuid::new_v4().to_string(),
        name: args.name.clone(),
        description: args.description.clone(),
        created_at: now_iso(),
        created_by: None,
    };
    db.insert_collection(&collection)?;
    write_success(mode, &format!("Collection '{}' created", args.name))?;
    Ok(())
}

fn handle_collection_delete(cli: &Cli, mode: &OutputMode, args: &CollectionDeleteArgs) -> Result<()> {
    let db = open_db(cli)?;
    let count = db.delete_collection(&args.name)?;
    write_success(
        mode,
        &format!("Collection '{}' deleted ({count} bookmarks were in it)", args.name),
    )?;
    Ok(())
}

fn handle_collection_add(cli: &Cli, mode: &OutputMode, args: &CollectionAddArgs) -> Result<()> {
    let db = open_db(cli)?;
    // Auto-create collection if it doesn't exist
    let collection = match db.get_collection_by_name(&args.name)? {
        Some(c) => c,
        None => {
            let c = Collection {
                id: uuid::Uuid::new_v4().to_string(),
                name: args.name.clone(),
                description: None,
                created_at: now_iso(),
                created_by: None,
            };
            db.insert_collection(&c)?;
            c
        }
    };
    let added = db.add_to_collection_at(&collection.id, &args.bookmark_ids, args.at)?;
    write_success(mode, &format!("Added {added} bookmarks to '{}'", args.name))?;
    Ok(())
}

fn handle_collection_reorder(cli: &Cli, mode: &OutputMode, args: &CollectionReorderArgs) -> Result<()> {
    let db = open_db(cli)?;
    let collection = db
        .get_collection_by_name(&args.name)?
        .ok_or_else(|| Error::Input(format!("collection '{}' not found", args.name)))?;
    db.reorder_collection(&collection.id, &args.bookmark_ids)?;
    write_success(
        mode,
        &format!("Reordered {} bookmarks in '{}'", args.bookmark_ids.len(), args.name),
    )?;
    Ok(())
}

fn handle_collection_remove(cli: &Cli, mode: &OutputMode, args: &CollectionRemoveArgs) -> Result<()> {
    let db = open_db(cli)?;
    let collection = db
        .get_collection_by_name(&args.name)?
        .ok_or_else(|| Error::Input(format!("collection '{}' not found", args.name)))?;
    let removed = db.remove_from_collection(&collection.id, &args.bookmark_ids)?;
    write_success(
        mode,
        &format!("Removed {removed} bookmarks from '{}'", args.name),
    )?;
    Ok(())
}

fn handle_collection_list(cli: &Cli, mode: &OutputMode, args: &CollectionListArgs) -> Result<()> {
    let db = open_db(cli)?;

    if let Some(ref bookmark_id) = args.bookmark {
        let bm = find_bookmark(&db, bookmark_id)?;
        let collections = db.list_collections_for_bookmark(&bm.id)?;
        match mode {
            OutputMode::Json => write_json_success(&collections)?,
            OutputMode::Table => {
                let mut table = comfy_table::Table::new();
                table.set_header(vec!["Name", "Description", "Created"]);
                for c in &collections {
                    table.add_row(vec![
                        &c.name,
                        c.description.as_deref().unwrap_or(""),
                        &c.created_at,
                    ]);
                }
                println!("{table}");
            }
            _ => {
                let mut stdout = io::stdout().lock();
                for c in &collections {
                    writeln!(
                        stdout,
                        "{}\t{}",
                        c.name,
                        c.description.as_deref().unwrap_or("")
                    )?;
                }
            }
        }
    } else {
        let collections = db.list_collections()?;
        match mode {
            OutputMode::Json => write_json_success(&collections)?,
            OutputMode::Table => {
                let mut table = comfy_table::Table::new();
                table.set_header(vec!["Name", "Bookmarks", "Description", "Created"]);
                for (c, count) in &collections {
                    table.add_row(vec![
                        c.name.clone(),
                        count.to_string(),
                        c.description.clone().unwrap_or_default(),
                        c.created_at.clone(),
                    ]);
                }
                println!("{table}");
            }
            _ => {
                let mut stdout = io::stdout().lock();
                for (c, count) in &collections {
                    writeln!(
                        stdout,
                        "{}\t{}\t{}",
                        c.name,
                        count,
                        c.description.as_deref().unwrap_or("")
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn handle_collection_show(cli: &Cli, mode: &OutputMode, args: &CollectionShowArgs) -> Result<()> {
    let db = open_db(cli)?;
    let filter = BookmarkFilter {
        collection: Some(args.name.clone()),
        ..Default::default()
    };
    let bookmarks = db.list_bookmarks(&filter)?;
    if matches!(mode, OutputMode::Tv) {
        let bookmark_data: std::collections::HashMap<String, (String, String)> = bookmarks
            .iter()
            .map(|bm| (short_id(&bm.id).to_string(), (bm.id.clone(), bm.file_path.clone())))
            .collect();
        output::write_bookmarks_with_line(mode, &bookmarks, |short_id| {
            let (full_id, file_path) = bookmark_data.get(short_id)?;
            get_bookmark_line(&db, full_id, file_path)
        })?;
    } else {
        write_bookmarks(mode, &bookmarks)?;
    }
    Ok(())
}

fn handle_collection_resolve(cli: &Cli, mode: &OutputMode, args: &CollectionResolveArgs) -> Result<()> {
    let db = open_db(cli)?;
    let filter = BookmarkFilter {
        collection: Some(args.name.clone()),
        status: Some(vec![BookmarkStatus::Active, BookmarkStatus::Drifted]),
        ..Default::default()
    };
    let bookmarks = db.list_bookmarks(&filter)?;
    let config = load_config(cli);
    resolve_batch(mode, &db, &bookmarks, &config)?;
    Ok(())
}

// --- Shared helpers ---

fn resolve_batch(
    mode: &OutputMode,
    db: &Database,
    bookmarks: &[Bookmark],
    config: &Config,
) -> Result<()> {
    let mut results = Vec::new();

    for bm in bookmarks {
        let Ok(lang) = bm.language.parse::<Language>() else {
            continue;
        };
        let mut cache = ParseCache::new(lang)?;
        let ts_lang = lang.tree_sitter_language();
        let result = resolution::resolve(bm, &mut cache, &ts_lang)?;
        let new_status = health::transition(bm.status, result.method, result.hash_matches);

        let stale_since = if new_status == BookmarkStatus::Stale {
            bm.stale_since.clone().or_else(|| Some(now_iso()))
        } else {
            None
        };

        db.update_bookmark_status(
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
            byte_range: Some(format!("{}:{}", result.byte_range.0, result.byte_range.1)),
            line_range: Some(format!("{}:{}", result.start_line + 1, result.end_line + 1)),
            content_hash: Some(result.content_hash.clone()),
        };
        let _ = db.insert_resolution_if_changed(&res, config.storage.max_resolutions_per_bookmark);

        results.push(serde_json::json!({
            "id": output::short_id(&bm.id),
            "file": result.file_path,
            "line": result.start_line + 1,
            "method": result.method.to_string(),
            "status": new_status.to_string(),
        }));
    }

    match mode {
        OutputMode::Json => write_json_success(&results)?,
        OutputMode::Table => {
            let mut table = comfy_table::Table::new();
            table.set_header(vec!["ID", "File", "Line", "Method", "Status"]);
            for r in &results {
                table.add_row(vec![
                    r["id"].as_str().unwrap_or(""),
                    r["file"].as_str().unwrap_or(""),
                    &r["line"].to_string(),
                    r["method"].as_str().unwrap_or(""),
                    r["status"].as_str().unwrap_or(""),
                ]);
            }
            println!("{table}");
        }
        _ => {
            let mut stdout = io::stdout().lock();
            for r in &results {
                writeln!(
                    stdout,
                    "{}\t{}:{}\t{}\t{}",
                    r["id"].as_str().unwrap_or(""),
                    r["file"].as_str().unwrap_or(""),
                    r["line"],
                    r["method"].as_str().unwrap_or(""),
                    r["status"].as_str().unwrap_or(""),
                )?;
            }
        }
    }
    Ok(())
}

fn write_resolution_output(
    mode: &OutputMode,
    bm: &Bookmark,
    result: &resolution::ResolutionResult,
) -> Result<()> {
    match mode {
        OutputMode::Json => {
            write_json_success(&serde_json::json!({
                "id": bm.id,
                "file": result.file_path,
                "line": result.start_line + 1,
                "column": result.start_col,
                "byte_range": format!("{}:{}", result.byte_range.0, result.byte_range.1),
                "method": result.method.to_string(),
                "status": health::transition(bm.status, result.method, result.hash_matches).to_string(),
                "preview": result.matched_text.lines().next().unwrap_or(""),
                "note": bm.notes,
                "tags": bm.tags,
            }))?;
        }
        OutputMode::Line => {
            let mut stdout = io::stdout().lock();
            writeln!(
                stdout,
                "{}\t{}:{}\t{}\t{}\t{}",
                output::short_id(&bm.id),
                result.file_path,
                result.start_line + 1,
                result.method,
                bm.tags.join(","),
                bm.notes.as_deref().unwrap_or("")
            )?;
        }
        _ => {
            println!(
                "{}  {}:{}  [{}]",
                output::short_id(&bm.id),
                result.file_path,
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

fn parse_duration_days(duration: &str) -> Result<i64> {
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
