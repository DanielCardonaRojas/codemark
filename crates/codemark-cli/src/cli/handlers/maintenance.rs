//! Database maintenance: heal, status, diff, gc, export, import.

use crate::cli::output::{
    self, ByteLocation, HealOutput, HealUpdate, OutputMode, write_heal_output, write_json_success,
    write_success,
};
use crate::cli::*;
use codemark_core::embeddings::config::EmbeddingModel;
use codemark_core::engine::bookmark::{
    Bookmark, BookmarkFilter, BookmarkHealth, Resolution, ResolutionMethod, Tag,
};
use codemark_core::engine::{health, resolution};
use codemark_core::error::{Error, Result};
use codemark_core::git::context as git_context;
use codemark_core::parser::languages::{Language, ParseCache};
use codemark_core::storage::SemanticRepo;

use super::{load_config, now_iso, open_all_dbs, open_db, open_db_for_write};

pub async fn handle_heal(cli: &Cli, mode: &OutputMode, args: &HealArgs) -> Result<()> {
    super::check_deprecated_status(&args.health, &args.status);
    let db = open_db_for_write(cli)?;
    let health_input = args.health.as_deref().or(args.status.as_deref());
    let filter = BookmarkFilter {
        file_path: args.file.as_ref().map(|p| p.to_string_lossy().to_string()),
        language: args.lang.clone(),
        health: super::parse_health_filter(health_input)?.or(Some(vec![
            BookmarkHealth::Active,
            BookmarkHealth::Drifted,
            BookmarkHealth::Stale,
        ])),
        collection: args.collection.clone(),
        ..Default::default()
    };
    let bookmarks = db.list_bookmarks(&filter)?;
    let config = load_config(cli);

    // Get current HEAD once for all bookmarks
    let cwd = std::env::current_dir()?;
    let current_head = git_context::detect_context(&cwd).and_then(|ctx| ctx.head_commit);

    let mut updates = Vec::new();
    let mut skipped = 0usize;
    let mut affected_collections = std::collections::HashSet::new();

    for bm in &bookmarks {
        // Get previous resolution for location tracking
        let previous_resolution = db.list_resolutions(&bm.id, 1).ok();
        let previous_location = previous_resolution
            .as_ref()
            .and_then(|r| r.first())
            .and_then(|r| r.byte_range.as_ref())
            .and_then(|s| ByteLocation::from_str(s));

        // Skip heal if HEAD is before latest resolution (unless --force is set)
        if !args.force
            && let Some(ref head) = current_head
            && let Some(res) = previous_resolution.as_ref().and_then(|r| r.first())
            && let Some(ref res_commit) = res.commit_hash
        {
            match git_context::is_ancestor(&cwd, head, res_commit) {
                Ok(true) => {
                    // HEAD is ancestor of resolution (resolution is ahead)
                    skipped += 1;
                    eprintln!(
                        "codemark: skipping {} (resolution at {} is ahead of HEAD {})",
                        output::short_id(&bm.id),
                        &res_commit[..8.min(res_commit.len())],
                        &head[..8.min(head.len())]
                    );
                    continue;
                }
                Ok(false) => {
                    // HEAD is ahead or unrelated, proceed with heal
                }
                Err(_) => {
                    // git error, proceed with heal (graceful degradation)
                }
            }
        }

        let Ok(lang) = bm.language.parse::<Language>() else {
            continue;
        };
        let mut cache = ParseCache::new(lang)?;
        let ts_lang = lang.tree_sitter_language();
        let provider = codemark_core::vfs::LocalFileProvider;
        let result = resolution::resolve(bm, &mut cache, &ts_lang, db.path(), &provider).await?;
        let days_since = health::days_since_resolution(bm.last_resolved_at.as_deref());
        let new_status = health::transition(
            bm.health,
            result.method,
            result.hash_matches,
            days_since,
            config.health.stale_days(),
        );
        let previous_status = bm.health;

        let stale_since = if new_status == BookmarkHealth::Stale {
            bm.stale_since.clone().or_else(|| Some(now_iso()))
        } else {
            None
        };

        // Auto-archive check
        let final_status = if args.auto_archive
            && new_status == BookmarkHealth::Stale
            && bm
                .stale_since
                .as_deref()
                .is_some_and(|s| health::should_auto_archive(s, args.archive_after))
        {
            BookmarkHealth::Archived
        } else {
            new_status
        };

        let breadcrumbs_json = if result.breadcrumbs.is_empty() {
            None
        } else {
            serde_json::to_string(&result.breadcrumbs).ok()
        };

        db.update_bookmark_health(
            &bm.id,
            final_status,
            Some(result.method),
            Some(&now_iso()),
            stale_since.as_deref(),
        )?;

        // Track affected collections for health recompute
        if let Ok(ids) = db.list_collection_ids_for_bookmark(&bm.id) {
            for id in ids {
                affected_collections.insert(id);
            }
        }

        if let Some(ref new_query) = result.new_query {
            db.update_bookmark_query(&bm.id, new_query, &result.file_path, &result.content_hash)?;
        }

        // Track the resolution ID that was created (if any)
        let resolution_id = if !args.validate_only {
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
            let res_id = res.id.clone();
            let _ = db.insert_resolution_if_changed(&res, config.storage.max_resolutions());
            Some(res_id)
        } else {
            None
        };

        // Build the new location (null if failed)
        let new_location = if result.method != ResolutionMethod::Failed {
            Some(ByteLocation { start_byte: result.byte_range.0, end_byte: result.byte_range.1 })
        } else {
            None
        };

        // Generate a name for the bookmark (extract from annotations or use file)
        let name = bm
            .annotations
            .first()
            .and_then(|a| a.notes.as_deref())
            .or_else(|| {
                // Try to extract function name from query
                bm.query.lines().find(|l| l.contains("#eq?")).and_then(|l| l.split('"').nth(1))
            })
            .unwrap_or(&bm.file_path)
            .to_string();

        updates.push(HealUpdate {
            bookmark_id: bm.id.clone(),
            resolution_id,
            name,
            file_path: bm.file_path.clone(),
            previous_health: previous_status.to_string(),
            new_health: final_status.to_string(),
            previous_health_alias: previous_status.to_string(),
            new_health_alias: final_status.to_string(),
            resolution_method: result.method.to_string(),
            previous_location,
            new_location,
        });
    }

    // Recompute health for all affected collections
    for collection_id in affected_collections {
        if let Err(e) = db.recompute_collection_health(&collection_id) {
            eprintln!(
                "codemark: warning: failed to recompute health for collection {}: {}",
                collection_id, e
            );
        }
    }

    let total_processed = updates.len();
    let output = HealOutput { total_processed, skipped, updates };

    write_heal_output(mode, &output)?;
    Ok(())
}

pub async fn handle_status(cli: &Cli, mode: &OutputMode) -> Result<()> {
    let dbs = open_all_dbs(cli)?;
    let multi = dbs.len() > 1;

    let mut total_active = 0usize;
    let mut total_drifted = 0usize;
    let mut total_stale = 0usize;
    let mut total_archived = 0usize;

    for (label, db) in &dbs {
        let counts = db.count_by_health()?;
        let a = counts.get(&BookmarkHealth::Active).copied().unwrap_or(0);
        let d = counts.get(&BookmarkHealth::Drifted).copied().unwrap_or(0);
        let s = counts.get(&BookmarkHealth::Stale).copied().unwrap_or(0);
        let r = counts.get(&BookmarkHealth::Archived).copied().unwrap_or(0);

        if multi {
            match mode {
                OutputMode::Json => {}
                _ => {
                    println!("[{label}] {a} active  |  {d} drifted  |  {s} stale  |  {r} archived")
                }
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
            println!(
                "{total_active} active  |  {total_drifted} drifted  |  {total_stale} stale  |  {total_archived} archived"
            );
        }
    }
    Ok(())
}

pub async fn handle_diff(cli: &Cli, mode: &OutputMode, args: &DiffArgs) -> Result<()> {
    let db = open_db(cli)?;
    let config = super::load_config(cli);
    let cwd = std::env::current_dir()?;

    let since = args.since.as_deref().unwrap_or("HEAD~1");

    let changed_files = git_context::changed_files_since(&cwd, since)?;
    if changed_files.is_empty() {
        write_success(mode, "No files changed.")?;
        return Ok(());
    }

    // Find bookmarks that reference changed files
    let all_bookmarks = db.list_bookmarks(&BookmarkFilter {
        health: Some(vec![BookmarkHealth::Active, BookmarkHealth::Drifted, BookmarkHealth::Stale]),
        ..Default::default()
    })?;

    let affected: Vec<&Bookmark> =
        all_bookmarks.iter().filter(|bm| changed_files.contains(&bm.file_path)).collect();

    if affected.is_empty() {
        write_success(
            mode,
            &format!("{} files changed since {since}, no bookmarks affected.", changed_files.len()),
        )?;
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
                let provider = codemark_core::vfs::LocalFileProvider;
                let result =
                    resolution::resolve(bm, &mut cache, &ts_lang, db.path(), &provider).await?;
                let days_since = health::days_since_resolution(bm.last_resolved_at.as_deref());
                let new_status = health::transition(
                    bm.health,
                    result.method,
                    result.hash_matches,
                    days_since,
                    config.health.stale_days(),
                );
                results.push(serde_json::json!({
                    "id": bm.id,
                    "file": bm.file_path,
                    "status_before": bm.health.to_string(),
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
                let provider = codemark_core::vfs::LocalFileProvider;
                let result =
                    resolution::resolve(bm, &mut cache, &ts_lang, db.path(), &provider).await?;
                let days_since = health::days_since_resolution(bm.last_resolved_at.as_deref());
                let new_status = health::transition(
                    bm.health,
                    result.method,
                    result.hash_matches,
                    days_since,
                    config.health.stale_days(),
                );
                let status_change = if new_status != bm.health {
                    format!("{} -> {}", bm.health, new_status)
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
                // Display first annotation's notes if available
                if let Some(ann) = bm.annotations.first()
                    && let Some(ref note) = ann.notes
                {
                    println!("    {note}");
                }
            }
        }
    }
    Ok(())
}

pub async fn handle_gc(cli: &Cli, mode: &OutputMode, args: &GcArgs) -> Result<()> {
    let db = open_db_for_write(cli)?;
    let days = super::parse_duration_days(&args.older_than)?;
    let cutoff = chrono::Utc::now() - chrono::Duration::days(days);
    let cutoff_str = cutoff.format("%Y-%m-%dT%H:%M:%SZ").to_string();

    if args.dry_run {
        let filter =
            BookmarkFilter { health: Some(vec![BookmarkHealth::Archived]), ..Default::default() };
        let bookmarks = db.list_bookmarks(&filter)?;
        let would_remove: Vec<_> = bookmarks.iter().filter(|b| b.created_at < cutoff_str).collect();
        write_success(mode, &format!("Would remove {} archived bookmarks", would_remove.len()))?;
    } else {
        let count = db.delete_archived_before(&cutoff_str)?;
        write_success(mode, &format!("Removed {count} archived bookmarks"))?;
    }
    Ok(())
}

pub async fn handle_export(cli: &Cli, args: &ExportArgs) -> Result<()> {
    super::check_deprecated_status(&args.health, &args.status);
    let dbs = open_all_dbs(cli)?;
    let health_input = args.health.as_deref().or(args.status.as_deref());
    let filter = BookmarkFilter {
        tag: args.tag.clone(),
        health: super::parse_health_filter(health_input)?,
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
            println!("id,file_path,language,health,tags,notes");
            for bm in &bookmarks {
                // For CSV, we use the first annotation's notes
                let notes = bm.annotations.first().and_then(|a| a.notes.as_deref()).unwrap_or("");
                println!(
                    "{},{},{},{},{},{}",
                    bm.id,
                    bm.file_path,
                    bm.language,
                    bm.health,
                    serde_json::to_string(&bm.tags).unwrap_or_else(|_| "[]".to_string()),
                    notes
                );
            }
        }
    }
    Ok(())
}

/// Import bookmarks from a JSON file.
pub async fn handle_import(cli: &Cli, mode: &OutputMode, args: &ImportArgs) -> Result<()> {
    let mut db = open_db_for_write(cli)?;
    let content = std::fs::read_to_string(&args.file)
        .map_err(|e| Error::Input(format!("cannot read {}: {e}", args.file.display())))?;
    let bookmarks: Vec<Bookmark> = serde_json::from_str(&content)?;

    let mut imported = 0;
    let mut skipped = 0;
    let mut imported_bookmarks = Vec::new();
    for bm in &bookmarks {
        if db.get_bookmark(&bm.id)?.is_some() {
            skipped += 1;
        } else {
            // Insert the bookmark (without annotations/tags in the main table)
            let bookmark_id = db.insert_bookmark(bm)?;

            // Insert annotations if present
            for ann in &bm.annotations {
                let mut new_ann = ann.clone();
                new_ann.bookmark_id = bookmark_id.clone();
                db.insert_annotation(&new_ann)?;
            }

            // Insert tags if present
            if !bm.tags.is_empty() {
                let tags: Vec<Tag> = bm
                    .tags
                    .iter()
                    .map(|t| Tag {
                        bookmark_id: bookmark_id.clone(),
                        tag: t.clone(),
                        added_at: now_iso(),
                        added_by: bm.created_by.clone(),
                    })
                    .collect();
                db.insert_tags(&tags)?;
            }

            // Fetch the full bookmark with metadata for embedding generation
            if let Ok(Some(full_bm)) = db.get_bookmark(&bookmark_id) {
                imported_bookmarks.push(full_bm);
            }

            imported += 1;
        }
    }

    // Generate embeddings for imported bookmarks (if semantic search is enabled)
    if !imported_bookmarks.is_empty() {
        let config = load_config(cli);
        if config.semantic.is_enabled() {
            let models_dir = config.semantic.get_models_dir();

            let model = config
                .semantic
                .model
                .as_deref()
                .and_then(|m| m.parse::<EmbeddingModel>().ok())
                .unwrap_or(EmbeddingModel::AllMiniLmL6V2);

            let distance_metric = config.semantic.get_distance_metric();
            let threshold = config.semantic.threshold;

            let semantic_repo =
                SemanticRepo::with_config(models_dir, model, distance_metric, threshold);
            let conn = db.conn_mut();
            // Ignore errors - embedding generation failure shouldn't block import
            let _ = semantic_repo.store_embeddings(conn, &imported_bookmarks).await;
        }
    }

    write_success(mode, &format!("Imported {imported} bookmarks ({skipped} duplicates skipped)"))?;
    Ok(())
}
