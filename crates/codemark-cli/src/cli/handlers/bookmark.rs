//! Bookmark CRUD operations: add, show, remove, annotate, resolve.

use std::io::Read;

use crate::cli::output::{
    self, OutputMode, short_id, write_bookmark_markdown, write_json_success, write_success,
};
use crate::cli::*;
use codemark_core::engine::bookmark::{
    Annotation, Bookmark, BookmarkFilter, BookmarkHealth, Resolution, ResolutionMethod, Tag,
};
use codemark_core::engine::{hash, health, resolution};
use codemark_core::error::{Error, Result};
use codemark_core::git::context as git_context;
use codemark_core::parser::languages::{Language, ParseCache};
use codemark_core::query::generator as qgen;

use super::{
    add_bookmark_to_collection, find_bookmark, find_bookmark_across,
    generate_embedding_for_bookmark, now_iso, open_all_dbs, open_db_for_write, resolve_batch,
    resolve_identity, resolve_or_create_repo_metadata, write_resolution_output,
};

/// Add a new bookmark from a file range or git hunk.
pub async fn handle_add(cli: &Cli, mode: &OutputMode, args: &AddArgsOriginal) -> Result<()> {
    let lang = resolve_language(args.lang.as_deref(), &args.file)?;
    let (abs_path, rel_path) = resolve_file_path(&args.file)?;

    let mut parser = codemark_core::parser::languages::Parser::new(lang)?;
    let provider = codemark_core::vfs::LocalFileProvider;
    let (tree, source) = parser.parse_file(&abs_path, &provider).await?;
    let ts_lang = lang.tree_sitter_language();

    // Resolve byte range from --range or --hunk
    let byte_range = if let Some(ref hunk) = args.hunk {
        let (start_line, end_line) = super::parse_hunk(hunk)?;
        super::line_range_to_bytes(&source, start_line, end_line)?
    } else if let Some(ref range) = args.range {
        super::parse_range(range, &source)?
    } else {
        return Err(Error::Input("either --range or --hunk is required".into()));
    };

    let generated = qgen::generate_query(&tree, source.as_bytes(), byte_range, &ts_lang)?;
    let content_hash = hash::content_hash(&source[generated.byte_range.0..generated.byte_range.1]);

    // Count matches for uniqueness info
    let match_count = codemark_core::query::matcher::run_query(
        &generated.query,
        &tree,
        source.as_bytes(),
        &ts_lang,
    )
    .map(|m| m.len())
    .unwrap_or(0);

    // Compute the line range of the target for display
    let target_start_line = super::byte_to_line(&source, generated.byte_range.0);
    let target_end_line = super::byte_to_line(&source, generated.byte_range.1.saturating_sub(1));

    if args.dry_run {
        return write_dry_run(
            mode,
            &generated,
            &content_hash,
            &rel_path,
            target_start_line,
            target_end_line,
            match_count,
        );
    }

    let db = open_db_for_write(cli)?;
    let cwd = std::env::current_dir()?;
    let commit_hash = git_context::detect_context(&cwd).and_then(|ctx| ctx.head_commit);

    // Resolve identity and create/update repo metadata
    let config = super::load_config(cli);
    let (db_owner_email, db_owner_name) = resolve_identity(&config);
    if let Err(e) =
        resolve_or_create_repo_metadata(&db, &config, &db_owner_email, db_owner_name.as_deref())
    {
        eprintln!("codemark: warning: failed to record repo metadata: {e}");
    }

    let bookmark_id = uuid::Uuid::new_v4().to_string();
    let bookmark = Bookmark {
        id: bookmark_id.clone(),
        query: generated.query.clone(),
        language: lang.to_string(),
        file_path: rel_path.clone(),
        content_hash: Some(content_hash.clone()),
        commit_hash: commit_hash.clone(),
        health: BookmarkHealth::Active,
        resolution_method: Some(ResolutionMethod::Exact),
        last_resolved_at: Some(now_iso()),
        stale_since: None,
        created_at: now_iso(),
        created_by: Some(args.created_by.clone()),
        tags: Vec::new(),

        annotations: vec![],
        comments: vec![],
    };

    // Insert bookmark - will return existing ID if duplicate
    let actual_bookmark_id = db.insert_bookmark(&bookmark)?;
    let is_new = actual_bookmark_id == bookmark_id;

    // Insert annotations with notes and context if provided
    if !args.note.is_empty() || args.context.is_some() {
        let mut annotations = Vec::new();
        // If context is provided, attach it to the first note
        let first_note = args.note.first();
        if first_note.is_some() || args.context.is_some() {
            annotations.push(Annotation {
                id: uuid::Uuid::new_v4().to_string(),
                bookmark_id: actual_bookmark_id.clone(),
                added_at: now_iso(),
                added_by: Some(args.created_by.clone()),
                notes: first_note.cloned(),
                context: args.context.clone(),
                source: Some("cli".to_string()),
            });
        }
        // Additional notes without context
        for note in args.note.iter().skip(1) {
            annotations.push(Annotation {
                id: uuid::Uuid::new_v4().to_string(),
                bookmark_id: actual_bookmark_id.clone(),
                added_at: now_iso(),
                added_by: Some(args.created_by.clone()),
                notes: Some(note.clone()),
                context: None,
                source: Some("cli".to_string()),
            });
        }
        db.insert_annotations(&annotations)?;
    }

    // Insert tags if provided
    if !args.tag.is_empty() {
        let tags: Vec<Tag> = args
            .tag
            .iter()
            .map(|t| Tag {
                bookmark_id: actual_bookmark_id.clone(),
                tag: t.clone(),
                added_at: now_iso(),
                added_by: Some(args.created_by.clone()),
            })
            .collect();
        db.insert_tags(&tags)?;
    }

    // For output, we need the full bookmark with metadata
    let bookmark = db.get_bookmark(&actual_bookmark_id)?.unwrap();

    // Generate embedding for semantic search
    let config = super::load_config(cli);
    // Ignore embedding errors - shouldn't block bookmark creation
    let _ = generate_embedding_for_bookmark(cli, &config, &bookmark).await;

    // Record initial resolution as baseline (only if new bookmark)
    if is_new {
        let breadcrumbs = if let Some(node) = tree
            .root_node()
            .descendant_for_byte_range(generated.byte_range.0, generated.byte_range.1)
        {
            codemark_core::engine::breadcrumbs::extract_breadcrumbs(node, &source, lang, 3)
        } else {
            Vec::new()
        };
        let breadcrumbs_json =
            if breadcrumbs.is_empty() { None } else { serde_json::to_string(&breadcrumbs).ok() };

        let initial_res = Resolution {
            id: uuid::Uuid::new_v4().to_string(),
            bookmark_id: actual_bookmark_id.clone(),
            resolved_at: now_iso(),
            commit_hash,
            method: ResolutionMethod::Exact,
            match_count: Some(match_count as i32),
            file_path: Some(bookmark.file_path.clone()),
            byte_range: Some(format!("{}-{}", generated.byte_range.0, generated.byte_range.1)),
            line_range: Some(format!("{}-{}", target_start_line, target_end_line)),
            content_hash: Some(content_hash.clone()),
            headline: None,
            snapshot: Some(source[generated.byte_range.0..generated.byte_range.1].to_string()),
            breadcrumbs: breadcrumbs_json,
        };
        db.insert_resolution_if_changed(&initial_res, config.storage.max_resolutions())?;
    }

    // Add to collection if specified
    let collection_name = if let Some(ref coll_name) = args.collection {
        add_bookmark_to_collection(&db, &actual_bookmark_id, coll_name)?
    } else {
        None
    };

    match mode {
        OutputMode::Json => {
            let mut json_data = serde_json::json!({
                "id": actual_bookmark_id,
                "query": generated.query,
                "node_type": generated.target_node_type,
                "name": generated.target_name,
                "lines": format!("{target_start_line}-{target_end_line}"),
                "content_hash": content_hash,
                "unique": match_count == 1,
                "created_by": bookmark.created_by,
                "new": is_new,
            });
            if let Some(ref coll) = collection_name {
                json_data["collection"] = serde_json::json!(coll);
            }
            write_json_success(&json_data)?;
        }
        _ => {
            let action = if is_new { "created" } else { "updated" };
            println!("Bookmark {action}: {}", output::short_id(&actual_bookmark_id));
            println!("  Node type: {}", generated.target_node_type);
            if let Some(ref name) = generated.target_name {
                println!("  Target: {name}");
            }
            println!("  Lines: {target_start_line}-{target_end_line}");
            if let Some(ref coll) = collection_name {
                println!("  Collection: {coll}");
            }
        }
    }
    Ok(())
}

/// Add a new bookmark from a text snippet provided on stdin.
pub async fn handle_add_from_snippet(
    cli: &Cli,
    mode: &OutputMode,
    args: &AddFromSnippetArgs,
) -> Result<()> {
    let lang = resolve_language(args.lang.as_deref(), &args.file)?;
    let (abs_path, rel_path) = resolve_file_path(&args.file)?;

    // Read snippet from stdin
    let mut snippet = String::new();
    std::io::stdin().read_to_string(&mut snippet)?;
    let snippet = snippet.trim();
    if snippet.is_empty() {
        return Err(Error::Input("no snippet provided on stdin".into()));
    }

    let mut parser = codemark_core::parser::languages::Parser::new(lang)?;
    let provider = codemark_core::vfs::LocalFileProvider;
    let (tree, source) = parser.parse_file(&abs_path, &provider).await?;
    let ts_lang = lang.tree_sitter_language();

    let offset =
        source.find(snippet).ok_or_else(|| Error::Input("snippet not found in file".into()))?;
    let byte_range = (offset, offset + snippet.len());

    let generated = qgen::generate_query(&tree, source.as_bytes(), byte_range, &ts_lang)?;
    let content_hash = hash::content_hash(&source[generated.byte_range.0..generated.byte_range.1]);

    let match_count = codemark_core::query::matcher::run_query(
        &generated.query,
        &tree,
        source.as_bytes(),
        &ts_lang,
    )
    .map(|m| m.len())
    .unwrap_or(0);

    let target_start_line = super::byte_to_line(&source, generated.byte_range.0);
    let target_end_line = super::byte_to_line(&source, generated.byte_range.1.saturating_sub(1));

    if args.dry_run {
        return write_dry_run(
            mode,
            &generated,
            &content_hash,
            &rel_path,
            target_start_line,
            target_end_line,
            match_count,
        );
    }

    let db = open_db_for_write(cli)?;
    let cwd = std::env::current_dir()?;
    let commit_hash = git_context::detect_context(&cwd).and_then(|ctx| ctx.head_commit);

    // Resolve identity and create/update repo metadata
    let config = super::load_config(cli);
    let (db_owner_email, db_owner_name) = resolve_identity(&config);
    if let Err(e) =
        resolve_or_create_repo_metadata(&db, &config, &db_owner_email, db_owner_name.as_deref())
    {
        eprintln!("codemark: warning: failed to record repo metadata: {e}");
    }

    let bookmark_id = uuid::Uuid::new_v4().to_string();
    let bookmark = Bookmark {
        id: bookmark_id.clone(),
        query: generated.query.clone(),
        language: lang.to_string(),
        file_path: rel_path.clone(),
        content_hash: Some(content_hash.clone()),
        commit_hash: commit_hash.clone(),
        health: BookmarkHealth::Active,
        resolution_method: Some(ResolutionMethod::Exact),
        last_resolved_at: Some(now_iso()),
        stale_since: None,
        created_at: now_iso(),
        created_by: Some(args.created_by.clone()),
        tags: Vec::new(),

        annotations: vec![],
        comments: vec![],
    };

    // Insert bookmark - will return existing ID if duplicate
    let actual_bookmark_id = db.insert_bookmark(&bookmark)?;
    let is_new = actual_bookmark_id == bookmark_id;

    // Insert annotations with notes and context if provided
    if !args.note.is_empty() || args.context.is_some() {
        let mut annotations = Vec::new();
        // If context is provided, attach it to the first note
        let first_note = args.note.first();
        if first_note.is_some() || args.context.is_some() {
            annotations.push(Annotation {
                id: uuid::Uuid::new_v4().to_string(),
                bookmark_id: actual_bookmark_id.clone(),
                added_at: now_iso(),
                added_by: Some(args.created_by.clone()),
                notes: first_note.cloned(),
                context: args.context.clone(),
                source: Some("cli".to_string()),
            });
        }
        // Additional notes without context
        for note in args.note.iter().skip(1) {
            annotations.push(Annotation {
                id: uuid::Uuid::new_v4().to_string(),
                bookmark_id: actual_bookmark_id.clone(),
                added_at: now_iso(),
                added_by: Some(args.created_by.clone()),
                notes: Some(note.clone()),
                context: None,
                source: Some("cli".to_string()),
            });
        }
        db.insert_annotations(&annotations)?;
    }

    // Insert tags if provided
    if !args.tag.is_empty() {
        let tags: Vec<Tag> = args
            .tag
            .iter()
            .map(|t| Tag {
                bookmark_id: actual_bookmark_id.clone(),
                tag: t.clone(),
                added_at: now_iso(),
                added_by: Some(args.created_by.clone()),
            })
            .collect();
        db.insert_tags(&tags)?;
    }

    // For output, we need the full bookmark with metadata
    let bookmark = db.get_bookmark(&actual_bookmark_id)?.unwrap();

    // Generate embedding for semantic search
    // Ignore embedding errors - shouldn't block bookmark creation
    let _ = generate_embedding_for_bookmark(cli, &config, &bookmark).await;

    // Record initial resolution as baseline (only if new bookmark)
    if is_new {
        let breadcrumbs = if let Some(node) = tree
            .root_node()
            .descendant_for_byte_range(generated.byte_range.0, generated.byte_range.1)
        {
            codemark_core::engine::breadcrumbs::extract_breadcrumbs(node, &source, lang, 3)
        } else {
            Vec::new()
        };
        let breadcrumbs_json =
            if breadcrumbs.is_empty() { None } else { serde_json::to_string(&breadcrumbs).ok() };

        let initial_res = Resolution {
            id: uuid::Uuid::new_v4().to_string(),
            bookmark_id: actual_bookmark_id.clone(),
            resolved_at: now_iso(),
            commit_hash,
            method: ResolutionMethod::Exact,
            match_count: Some(match_count as i32),
            file_path: Some(bookmark.file_path.clone()),
            byte_range: Some(format!("{}-{}", generated.byte_range.0, generated.byte_range.1)),
            line_range: Some(format!("{}-{}", target_start_line, target_end_line)),
            content_hash: Some(content_hash.clone()),
            headline: None,
            snapshot: Some(source[generated.byte_range.0..generated.byte_range.1].to_string()),
            breadcrumbs: breadcrumbs_json,
        };
        db.insert_resolution_if_changed(&initial_res, config.storage.max_resolutions())?;
    }

    // Add to collection if specified
    let collection_name = if let Some(ref coll_name) = args.collection {
        add_bookmark_to_collection(&db, &actual_bookmark_id, coll_name)?
    } else {
        None
    };

    match mode {
        OutputMode::Json => {
            let mut json_data = serde_json::json!({
                "id": actual_bookmark_id,
                "query": generated.query,
                "node_type": generated.target_node_type,
                "name": generated.target_name,
                "content_hash": content_hash,
                "created_by": bookmark.created_by,
                "new": is_new,
            });
            if let Some(ref coll) = collection_name {
                json_data["collection"] = serde_json::json!(coll);
            }
            write_json_success(&json_data)?;
        }
        _ => {
            let action = if is_new { "created" } else { "updated" };
            println!("Bookmark {action}: {}", output::short_id(&actual_bookmark_id));
            if let Some(ref name) = generated.target_name {
                println!("  Target: {name}");
            }
            if let Some(ref coll) = collection_name {
                println!("  Collection: {coll}");
            }
        }
    }
    Ok(())
}

/// Add a new bookmark using an explicit tree-sitter query.
pub async fn handle_add_from_query(
    cli: &Cli,
    mode: &OutputMode,
    args: &AddFromQueryArgs,
) -> Result<()> {
    let lang = resolve_language(args.lang.as_deref(), &args.file)?;
    let (abs_path, rel_path) = resolve_file_path(&args.file)?;

    let mut parser = codemark_core::parser::languages::Parser::new(lang)?;
    let provider = codemark_core::vfs::LocalFileProvider;
    let (tree, source) = parser.parse_file(&abs_path, &provider).await?;
    let ts_lang = lang.tree_sitter_language();

    // Validate the query by running it
    let matches =
        codemark_core::query::matcher::run_query(&args.query, &tree, source.as_bytes(), &ts_lang)
            .map_err(|e| Error::Input(format!("invalid tree-sitter query: {e}")))?;

    if matches.is_empty() {
        return Err(Error::Input("query does not match any nodes in the file".into()));
    }

    // Use the first match's content for hashing
    let first_match = &matches[0];
    let content_hash = hash::content_hash(&first_match.node_text);

    // Get the match info for output
    let target_start_line = first_match.start_point.0 + 1;
    let target_end_line = first_match.end_point.0 + 1;
    let byte_range = first_match.byte_range;

    // Extract node type from the query (first identifier after opening paren)
    let node_type = args
        .query
        .trim()
        .strip_prefix('(')
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or("unknown")
        .to_string();

    if args.dry_run {
        return write_dry_run(
            mode,
            &codemark_core::query::generator::GeneratedQuery {
                query: args.query.clone(),
                byte_range,
                target_node_type: node_type.clone(),
                target_name: None,
            },
            &content_hash,
            &rel_path,
            target_start_line,
            target_end_line,
            matches.len(),
        );
    }

    let db = open_db_for_write(cli)?;
    let cwd = std::env::current_dir()?;
    let commit_hash = git_context::detect_context(&cwd).and_then(|ctx| ctx.head_commit);

    // Resolve identity and create/update repo metadata
    let config = super::load_config(cli);
    let (db_owner_email, db_owner_name) = resolve_identity(&config);
    if let Err(e) =
        resolve_or_create_repo_metadata(&db, &config, &db_owner_email, db_owner_name.as_deref())
    {
        eprintln!("codemark: warning: failed to record repo metadata: {e}");
    }

    let bookmark_id = uuid::Uuid::new_v4().to_string();
    let bookmark = Bookmark {
        id: bookmark_id.clone(),
        query: args.query.clone(),
        language: lang.to_string(),
        file_path: rel_path.clone(),
        content_hash: Some(content_hash.clone()),
        commit_hash: commit_hash.clone(),
        health: BookmarkHealth::Active,
        resolution_method: Some(ResolutionMethod::Exact),
        last_resolved_at: Some(now_iso()),
        stale_since: None,
        created_at: now_iso(),
        created_by: Some(args.created_by.clone()),
        tags: Vec::new(),

        annotations: vec![],
        comments: vec![],
    };

    // Insert bookmark - will return existing ID if duplicate
    let actual_bookmark_id = db.insert_bookmark(&bookmark)?;
    let is_new = actual_bookmark_id == bookmark_id;

    // Insert annotations with notes and context if provided
    if !args.note.is_empty() || args.context.is_some() {
        let mut annotations = Vec::new();
        // If context is provided, attach it to the first note
        let first_note = args.note.first();
        if first_note.is_some() || args.context.is_some() {
            annotations.push(Annotation {
                id: uuid::Uuid::new_v4().to_string(),
                bookmark_id: actual_bookmark_id.clone(),
                added_at: now_iso(),
                added_by: Some(args.created_by.clone()),
                notes: first_note.cloned(),
                context: args.context.clone(),
                source: Some("cli".to_string()),
            });
        }
        // Additional notes without context
        for note in args.note.iter().skip(1) {
            annotations.push(Annotation {
                id: uuid::Uuid::new_v4().to_string(),
                bookmark_id: actual_bookmark_id.clone(),
                added_at: now_iso(),
                added_by: Some(args.created_by.clone()),
                notes: Some(note.clone()),
                context: None,
                source: Some("cli".to_string()),
            });
        }
        db.insert_annotations(&annotations)?;
    }

    // Insert tags if provided
    if !args.tag.is_empty() {
        let tags: Vec<Tag> = args
            .tag
            .iter()
            .map(|t| Tag {
                bookmark_id: actual_bookmark_id.clone(),
                tag: t.clone(),
                added_at: now_iso(),
                added_by: Some(args.created_by.clone()),
            })
            .collect();
        db.insert_tags(&tags)?;
    }

    // For output, we need the full bookmark with metadata
    let bookmark = db.get_bookmark(&actual_bookmark_id)?.unwrap();

    // Generate embedding for semantic search
    // Ignore embedding errors - shouldn't block bookmark creation
    let _ = generate_embedding_for_bookmark(cli, &config, &bookmark).await;

    // Record initial resolution as baseline (only if new bookmark)
    if is_new {
        let breadcrumbs = if let Some(node) =
            tree.root_node().descendant_for_byte_range(byte_range.0, byte_range.1)
        {
            codemark_core::engine::breadcrumbs::extract_breadcrumbs(node, &source, lang, 3)
        } else {
            Vec::new()
        };
        let breadcrumbs_json =
            if breadcrumbs.is_empty() { None } else { serde_json::to_string(&breadcrumbs).ok() };

        let initial_res = Resolution {
            id: uuid::Uuid::new_v4().to_string(),
            bookmark_id: actual_bookmark_id.clone(),
            resolved_at: now_iso(),
            commit_hash,
            method: ResolutionMethod::Exact,
            match_count: Some(matches.len() as i32),
            file_path: Some(bookmark.file_path.clone()),
            byte_range: Some(format!("{}-{}", byte_range.0, byte_range.1)),
            line_range: Some(format!("{}-{}", target_start_line, target_end_line)),
            content_hash: Some(content_hash.clone()),
            headline: None,
            snapshot: Some(source[byte_range.0..byte_range.1].to_string()),
            breadcrumbs: breadcrumbs_json,
        };
        db.insert_resolution_if_changed(&initial_res, config.storage.max_resolutions())?;
    }

    // Add to collection if specified
    let collection_name = if let Some(ref coll_name) = args.collection {
        add_bookmark_to_collection(&db, &actual_bookmark_id, coll_name)?
    } else {
        None
    };

    match mode {
        OutputMode::Json => {
            let mut json_data = serde_json::json!({
                "id": actual_bookmark_id,
                "query": args.query,
                "node_type": node_type,
                "content_hash": content_hash,
                "created_by": bookmark.created_by,
                "new": is_new,
            });
            if let Some(ref coll) = collection_name {
                json_data["collection"] = serde_json::json!(coll);
            }
            write_json_success(&json_data)?;
        }
        _ => {
            let action = if is_new { "created" } else { "updated" };
            println!("Bookmark {action}: {}", output::short_id(&actual_bookmark_id));
            println!("  Node type: {node_type}");
            if matches.len() > 1 {
                println!("  Warning: query matches {} nodes", matches.len());
            }
            if let Some(ref coll) = collection_name {
                println!("  Collection: {coll}");
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

/// Resolve one or more bookmarks and update their health and location.
pub async fn handle_resolve(cli: &Cli, mode: &OutputMode, args: &ResolveArgs) -> Result<()> {
    super::check_deprecated_status(&args.health, &args.status);
    let dbs = open_all_dbs(cli)?;

    if let Some(ref id) = args.id {
        // Single bookmark resolution — search across all DBs
        let (bm, db) = find_bookmark_across(&dbs, id)?;
        let lang: Language = bm.language.parse()?;
        let mut cache = ParseCache::new(lang)?;
        let ts_lang = lang.tree_sitter_language();
        let provider = codemark_core::vfs::LocalFileProvider;

        let result = resolution::resolve(&bm, &mut cache, &ts_lang, db.path(), &provider).await?;

        let config = super::load_config(cli);
        // In dry-run mode, skip database updates and just show the result
        if args.dry_run {
            return write_resolution_output(
                mode,
                &bm,
                &result,
                db.path(),
                config.health.stale_days(),
            );
        }

        let days_since = health::days_since_resolution(bm.last_resolved_at.as_deref());
        let new_status = health::transition(
            bm.health,
            result.method,
            result.hash_matches,
            days_since,
            config.health.stale_days(),
        );

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

        // Recompute health for affected collections
        if let Ok(ids) = db.list_collection_ids_for_bookmark(&bm.id) {
            for id in ids {
                if let Err(e) = db.recompute_collection_health(&id) {
                    eprintln!(
                        "codemark: warning: failed to recompute health for collection {}: {}",
                        id, e
                    );
                }
            }
        }

        if let Some(ref new_query) = result.new_query {
            db.update_bookmark_query(&bm.id, new_query, &result.file_path, &result.content_hash)?;
        }

        let breadcrumbs_json = if result.breadcrumbs.is_empty() {
            None
        } else {
            serde_json::to_string(&result.breadcrumbs).ok()
        };

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
            byte_range: Some(format!("{}-{}", result.byte_range.0, result.byte_range.1)),
            line_range: Some(format!("{}-{}", result.start_line + 1, result.end_line + 1)),
            content_hash: Some(result.content_hash.clone()),
            headline: None,
            snapshot: Some(result.matched_text.clone()),
            breadcrumbs: breadcrumbs_json,
        };
        let config = super::load_config(cli);
        db.insert_resolution_if_changed(&res, config.storage.max_resolutions())?;

        write_resolution_output(mode, &bm, &result, db.path(), config.health.stale_days())?;
    } else {
        // Batch resolution — fan out across all DBs
        let health_input = args.health.as_deref().or(args.status.as_deref());
        let filter = BookmarkFilter {
            tag: args.tag.clone(),
            health: super::parse_health_filter(health_input)?
                .or(Some(vec![BookmarkHealth::Active, BookmarkHealth::Drifted])),
            file_path: args.file.as_ref().map(|p| p.to_string_lossy().to_string()),
            language: args.lang.clone(),
            collection: args.collection.clone(),
            ..Default::default()
        };
        let config = super::load_config(cli);
        let mut all_results = Vec::new();
        for (_label, db) in &dbs {
            let bookmarks = db.list_bookmarks(&filter)?;
            let results = resolve_batch(db, &bookmarks, &config, args.dry_run).await?;
            all_results.extend(results);
        }
        super::write_batch_output(mode, &all_results)?;
    }
    Ok(())
}

/// Show detailed information and resolution history for a bookmark.
pub async fn handle_show(cli: &Cli, mode: &OutputMode, args: &ShowArgs) -> Result<()> {
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
        OutputMode::Markdown => {
            write_bookmark_markdown(&bm, &resolutions)?;
        }
        _ => {
            println!("ID:          {}", bm.id);
            println!("File:        {}", bm.file_path);
            println!("Language:    {}", bm.language);
            println!("Health:      {}", bm.health);
            if !bm.tags.is_empty() {
                println!("Tags:        {}", bm.tags.join(", "));
            }
            // Display annotations (notes and context)
            for ann in &bm.annotations {
                if let Some(ref note) = ann.notes {
                    println!("Note:        {note}");
                }
                if let Some(ref ctx) = ann.context {
                    println!("Context:     {ctx}");
                }
                if let Some(ref added_by) = ann.added_by {
                    println!("  (by {added_by}, {})", ann.added_at);
                }
            }
            if let Some(ref method) = bm.resolution_method {
                println!("Resolution:  {method}");
            }
            if let Some(ref resolved) = bm.last_resolved_at {
                println!("Resolved at: {resolved}");
            }
            if let Some(breadcrumbs) = resolutions
                .first()
                .and_then(|r| r.breadcrumbs.as_ref())
                .and_then(|s| {
                    serde_json::from_str::<Vec<codemark_core::engine::breadcrumbs::Breadcrumb>>(s)
                        .ok()
                })
                .filter(|bc| !bc.is_empty())
            {
                let bc_str = breadcrumbs
                    .iter()
                    .map(|b| format!("{}:{}", b.line, b.text))
                    .collect::<Vec<_>>()
                    .join(" › ");
                println!("Context:     {bc_str}");
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

/// Remove one or more bookmarks from the database.
pub async fn handle_remove(cli: &Cli, mode: &OutputMode, args: &RemoveArgs) -> Result<()> {
    let db = open_db_for_write(cli)?;
    let mut removed = 0;
    let mut not_found = 0;

    for id_input in &args.ids {
        let id = super::extract_id(id_input);
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

/// Add notes, context, or tags to an existing bookmark.
pub async fn handle_annotate(cli: &Cli, mode: &OutputMode, args: &AnnotateArgs) -> Result<()> {
    let db = open_db_for_write(cli)?;

    // Validate that at least one of note, context, or tag is provided
    if args.note.is_empty() && args.context.is_none() && args.tag.is_empty() {
        return Err(Error::Input(
            "At least one of --note, --context, or --tag must be provided".to_string(),
        ));
    }

    // Find the bookmark
    let id = super::extract_id(&args.id);
    let mut bm = find_bookmark(&db, id)?;

    // Track inserted annotations for output
    let mut inserted_annotation_count = 0;

    // Create annotations if notes or context is provided
    if !args.note.is_empty() || args.context.is_some() {
        let mut annotations = Vec::new();
        // If context is provided, attach it to the first note
        let first_note = args.note.first();
        if first_note.is_some() || args.context.is_some() {
            annotations.push(Annotation {
                id: uuid::Uuid::new_v4().to_string(),
                bookmark_id: bm.id.clone(),
                added_at: now_iso(),
                added_by: Some(args.added_by.clone()),
                notes: first_note.cloned(),
                context: args.context.clone(),
                source: Some(args.source.clone()),
            });
        }
        // Additional notes without context
        for note in args.note.iter().skip(1) {
            annotations.push(Annotation {
                id: uuid::Uuid::new_v4().to_string(),
                bookmark_id: bm.id.clone(),
                added_at: now_iso(),
                added_by: Some(args.added_by.clone()),
                notes: Some(note.clone()),
                context: None,
                source: Some(args.source.clone()),
            });
        }
        inserted_annotation_count = annotations.len();
        db.insert_annotations(&annotations)?;

        // Re-fetch bookmark to get updated annotations
        bm = find_bookmark(&db, id)?;
    }

    // Add tags if provided
    if !args.tag.is_empty() {
        let tags: Vec<Tag> = args
            .tag
            .iter()
            .map(|t| Tag {
                bookmark_id: bm.id.clone(),
                tag: t.clone(),
                added_at: now_iso(),
                added_by: Some(args.added_by.clone()),
            })
            .collect();
        db.insert_tags(&tags)?;

        // Re-fetch bookmark to get updated tags
        bm = find_bookmark(&db, id)?;
    }

    match mode {
        OutputMode::Json => {
            write_json_success(&serde_json::json!({
                "id": bm.id,
                "short_id": short_id(&bm.id),
                "file_path": bm.file_path,
                "language": bm.language,
                "health": bm.health,
                "status": bm.health,
                "tags": bm.tags,
                "annotations": bm.annotations,
                "created_at": bm.created_at,
            }))?;
        }
        _ => {
            println!("Annotated bookmark: {}", short_id(&bm.id));
            println!("  File: {}", bm.file_path);
            println!("  Language: {}", bm.language);
            println!("  Health: {}", bm.health);
            if !bm.tags.is_empty() {
                println!("  Tags: {}", bm.tags.join(", "));
            }
            // Show the count of newly added annotations
            if inserted_annotation_count > 0 {
                println!(
                    "  Added {} annotation{}",
                    inserted_annotation_count,
                    if inserted_annotation_count == 1 { "" } else { "s" }
                );
            }
            // Show the newly added annotation (last one)
            let latest_ann = bm.annotations.last();
            if let Some(ann) = latest_ann {
                if let Some(ref note) = ann.notes {
                    println!("  Latest note: {}", note);
                }
                if let Some(ref ctx) = ann.context {
                    println!("  Context: {}", ctx);
                }
                println!("  Added by: {}", ann.added_by.as_deref().unwrap_or("unknown"));
            }
            // Show newly added tags
            let added_tags: Vec<&str> = args.tag.iter().map(|t| t.as_str()).collect();
            if !added_tags.is_empty() {
                println!("  Tags: {}", added_tags.join(", "));
            }
        }
    }

    Ok(())
}

// Helper functions for the bookmark module

fn resolve_language(lang_flag: Option<&str>, file: &std::path::Path) -> Result<Language> {
    if let Some(lang) = lang_flag {
        return lang.parse();
    }
    let ext = file.extension().and_then(|e| e.to_str()).ok_or_else(|| {
        Error::Input(format!(
            "cannot infer language from '{}'; use --lang to specify",
            file.display()
        ))
    })?;
    Language::from_extension(ext).ok_or_else(|| {
        Error::Input(format!(
            "cannot infer language from extension '.{ext}'; use --lang to specify"
        ))
    })
}

fn resolve_file_path(file: &std::path::Path) -> Result<(std::path::PathBuf, String)> {
    let abs =
        if file.is_absolute() { file.to_path_buf() } else { std::env::current_dir()?.join(file) };
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
