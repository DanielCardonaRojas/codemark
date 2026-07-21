//! Search and indexing: FTS search, semantic search, reindex.

#[cfg(feature = "semantic")]
use crate::cli::output::write_success;
use crate::cli::output::{
    self, OutputMode, short_id, write_bookmarks_with_line, write_json_success,
};
use crate::cli::*;
#[cfg(feature = "semantic")]
use codemark_core::embeddings::config::EmbeddingModel;
#[cfg(feature = "semantic")]
use codemark_core::engine::bookmark::{Bookmark, BookmarkFilter};
use codemark_core::error::{Error, Result};
#[cfg(feature = "semantic")]
use codemark_core::storage::SemanticRepo;
use codemark_core::storage::db::Database;

use super::{
    filter_dbs_by_repo_owner, filter_dbs_by_user_email, get_bookmark_line, load_config,
    open_all_dbs_with_extra_and_repos, open_db,
};

/// Search for bookmarks using full-text search or semantic search.
pub async fn handle_search(cli: &Cli, mode: &OutputMode, args: &SearchArgs) -> Result<()> {
    super::check_deprecated_status(&args.health, &args.status);
    let config = load_config(cli);

    // Collection search is a separate target (name/description/tags) rather than
    // a bookmark filter, so it gets its own code path for both FTS and semantic.
    if args.collections {
        return handle_collection_search(cli, mode, args, &config).await;
    }

    // `--tag` only filters collection search; reject it on the bookmark path
    // rather than silently ignoring it, so a forgotten `--collections` surfaces.
    if args.tag.is_some() {
        return Err(Error::Input("--tag requires --collections".to_string()));
    }

    // Semantic search requires a query
    #[cfg(feature = "semantic")]
    if args.semantic {
        if !config.semantic.is_enabled() {
            return Err(Error::Input("Semantic search is not enabled in config".to_string()));
        }
        let query = args
            .query
            .as_ref()
            .or(args.note.as_ref())
            .or(args.context.as_ref())
            .ok_or_else(|| Error::Input("Semantic search requires a query".to_string()))?;

        return handle_semantic_search(cli, mode, query, args).await;
    }

    // Regular FTS search
    let dbs = open_all_dbs_with_extra_and_repos(cli, &args.add_db, &args.repo)?;
    let dbs = filter_dbs_by_user_email(dbs, args.user_email.as_deref());
    let dbs = filter_dbs_by_repo_owner(dbs, args.repo_owner.as_deref());

    let health_input = args.health.as_deref().or(args.status.as_deref());
    let health_filter = super::parse_health_filter(health_input)?;

    if dbs.len() == 1 {
        let bookmarks = dbs[0].1.search_bookmarks(
            args.query.as_deref(),
            args.note.as_deref(),
            args.context.as_deref(),
            args.lang.as_deref(),
            args.author.as_deref(),
            args.collection.as_deref(),
            health_filter.clone(),
        )?;
        let db = &dbs[0].1;

        // Check if we need line numbers
        let needs_line = mode.needs_line()
            || args.line_format.as_deref().is_some_and(output::template_needs_line);

        if needs_line {
            let get_line_fn = |id: &str| -> Option<usize> {
                for bm in &bookmarks {
                    if short_id(&bm.id) == id {
                        return get_bookmark_line(db, &bm.id, &bm.file_path);
                    }
                }
                None
            };

            write_bookmarks_with_line(
                mode,
                &bookmarks,
                args.line_format.as_deref(),
                get_line_fn,
                None,
            )?;
        } else {
            output::write_bookmarks(mode, &bookmarks, args.line_format.as_deref(), None)?;
        }
    } else {
        let mut all = Vec::new();
        // Keep track of which database each bookmark belongs to for line resolution
        let mut db_map: std::collections::HashMap<String, &Database> =
            std::collections::HashMap::new();

        for (label, db) in &dbs {
            db_map.insert(label.clone(), db);
            let bookmarks = db.search_bookmarks(
                args.query.as_deref(),
                args.note.as_deref(),
                args.context.as_deref(),
                args.lang.as_deref(),
                args.author.as_deref(),
                args.collection.as_deref(),
                health_filter.clone(),
            )?;
            for bm in bookmarks {
                all.push((label.clone(), bm));
            }
        }
        let annotated: Vec<output::AnnotatedBookmark> = all
            .iter()
            .map(|(label, bm)| output::AnnotatedBookmark { source: label, bookmark: bm })
            .collect();

        // Check if we need line numbers
        let needs_line = mode.needs_line()
            || args.line_format.as_deref().is_some_and(output::template_needs_line);

        if needs_line {
            let bookmark_data: std::collections::HashMap<String, (String, String, String)> = all
                .iter()
                .map(|(label, bm)| {
                    (
                        short_id(&bm.id).to_string(),
                        (label.clone(), bm.id.clone(), bm.file_path.clone()),
                    )
                })
                .collect();

            let get_line_fn = |short_id: &str| -> Option<usize> {
                let (label, full_id, file_path) = bookmark_data.get(short_id)?;
                let db = db_map.get(label)?;
                get_bookmark_line(db, full_id, file_path)
            };

            output::write_annotated_bookmarks(
                mode,
                &annotated,
                args.line_format.as_deref(),
                Some(&get_line_fn),
                None,
            )?;
        } else {
            output::write_annotated_bookmarks(
                mode,
                &annotated,
                args.line_format.as_deref(),
                None as Option<&fn(&str) -> Option<usize>>,
                None,
            )?;
        }
    }
    Ok(())
}

/// Handle semantic search using vector embeddings.
#[cfg(feature = "semantic")]
async fn handle_semantic_search(
    cli: &Cli,
    mode: &OutputMode,
    query: &str,
    args: &SearchArgs,
) -> Result<()> {
    let db = open_db(cli)?;
    let config = load_config(cli);

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

    // Get models directory from config (defaults to global cache)
    let models_dir = config.semantic.get_models_dir();

    let semantic_repo = SemanticRepo::with_config(models_dir, model, distance_metric, threshold);

    // Perform semantic search
    let results = semantic_repo.search(db.conn(), query, args.limit).await?;

    // Build health filter
    let health_input = args.health.as_deref().or(args.status.as_deref());
    let health_filter = super::parse_health_filter(health_input)?;

    // Fetch full bookmark details for results and apply health filter
    let mut bookmarks = Vec::new();
    for result in results {
        if let Ok(Some(bm)) = db.get_bookmark(&result.id) {
            // Apply health filter if specified
            if let Some(ref filter) = health_filter
                && !filter.contains(&bm.health)
            {
                continue;
            }
            bookmarks.push((result.distance, bm));
        }
    }

    // Output results
    if matches!(mode, OutputMode::Json) {
        let data: Vec<serde_json::Value> = bookmarks
            .into_iter()
            .map(|(distance, bm)| {
                // Collect all annotations for JSON output
                let annotations: Vec<_> = bm.annotations.iter().collect();
                serde_json::json!({
                    "id": bm.id,
                    "short_id": short_id(&bm.id),
                    "query": bm.query,
                    "language": bm.language,
                    "file_path": bm.file_path,
                    "health": bm.health,
                    "status": bm.health,
                    "tags": bm.tags,
                    "annotations": annotations,
                    "created_at": bm.created_at,
                    "created_by": bm.created_by,
                    "distance": distance,
                })
            })
            .collect();
        write_json_success(&data)?;
    } else {
        // For non-JSON modes, use standard bookmark output functions
        let bookmarks_only: Vec<Bookmark> =
            bookmarks.iter().map(|(_, bm): &(f64, Bookmark)| bm.clone()).collect();

        // Check if we need line numbers
        let needs_line = mode.needs_line()
            || args.line_format.as_deref().is_some_and(output::template_needs_line);

        if needs_line {
            let get_line_fn = |id: &str| -> Option<usize> {
                for bm in &bookmarks_only {
                    if short_id(&bm.id) == id {
                        return get_bookmark_line(&db, &bm.id, &bm.file_path);
                    }
                }
                None
            };

            write_bookmarks_with_line(
                mode,
                &bookmarks_only,
                args.line_format.as_deref(),
                get_line_fn,
                None,
            )?;
        } else {
            output::write_bookmarks(mode, &bookmarks_only, args.line_format.as_deref(), None)?;
        }
    }

    Ok(())
}

/// Search collections by text or semantic similarity (name, description, tags).
async fn handle_collection_search(
    cli: &Cli,
    mode: &OutputMode,
    args: &SearchArgs,
    config: &codemark_core::config::Config,
) -> Result<()> {
    let db = open_db(cli)?;

    #[cfg(feature = "semantic")]
    let results: Vec<(codemark_core::engine::bookmark::Collection, usize)> = if args.semantic {
        collection_semantic_search(&db, args, config).await?
    } else {
        let mut collections = db.search_collections(args.query.as_deref(), args.tag.as_deref())?;
        collections.truncate(args.limit);
        collections
    };
    #[cfg(not(feature = "semantic"))]
    let results: Vec<(codemark_core::engine::bookmark::Collection, usize)> = {
        let _ = config; // only the semantic branch reads config
        let mut collections = db.search_collections(args.query.as_deref(), args.tag.as_deref())?;
        collections.truncate(args.limit);
        collections
    };

    write_collection_results(mode, &results)?;
    Ok(())
}

/// Rank collections by semantic similarity, resolving each hit to its full
/// collection with bookmark count and enforcing the `--tag` filter.
#[cfg(feature = "semantic")]
async fn collection_semantic_search(
    db: &Database,
    args: &SearchArgs,
    config: &codemark_core::config::Config,
) -> Result<Vec<(codemark_core::engine::bookmark::Collection, usize)>> {
    if !config.semantic.is_enabled() {
        return Err(Error::Input("Semantic search is not enabled in config".to_string()));
    }
    let query = args
        .query
        .as_ref()
        .ok_or_else(|| Error::Input("Semantic search requires a query".to_string()))?;

    let model = config
        .semantic
        .model
        .as_deref()
        .and_then(|m| m.parse::<EmbeddingModel>().ok())
        .unwrap_or(EmbeddingModel::AllMiniLmL6V2);
    let distance_metric = config.semantic.get_distance_metric();
    let threshold = config.semantic.threshold;
    let models_dir = config.semantic.get_models_dir();

    let semantic_repo = SemanticRepo::with_config(models_dir, model, distance_metric, threshold);
    let hits = semantic_repo.search_collections(db.conn(), query, args.limit).await?;

    // Resolve each hit to its full collection with bookmark count, applying
    // the same `--tag` filter the FTS branch does (semantic search ranks on
    // embeddings, so tag membership has to be enforced here).
    let mut out = Vec::new();
    for hit in hits {
        if let Some(c) = db.get_collection_by_id(&hit.id)? {
            if let Some(tag) = args.tag.as_deref() {
                let has_tag = db.list_tags_for_collection(&c.id)?.iter().any(|t| t.tag == tag);
                if !has_tag {
                    continue;
                }
            }
            let count = db.list_bookmarks_in_collection(&c.id).map(|b| b.len()).unwrap_or(0);
            out.push((c, count));
        }
    }
    Ok(out)
}

/// Render collection search results in the requested output mode.
fn write_collection_results(
    mode: &OutputMode,
    results: &[(codemark_core::engine::bookmark::Collection, usize)],
) -> Result<()> {
    use std::io::Write;

    match mode {
        OutputMode::Json => {
            let data: Vec<serde_json::Value> = results
                .iter()
                .map(|(c, count)| {
                    serde_json::json!({
                        "id": c.id,
                        "short_id": short_id(&c.id),
                        "name": c.name,
                        "description": c.description,
                        "health": c.health.as_ref().map(|h| h.to_string()),
                        "bookmark_count": count,
                        "created_at": c.created_at,
                        "created_by": c.created_by,
                        "created_branch": c.created_branch,
                    })
                })
                .collect();
            write_json_success(&data)?;
        }
        OutputMode::Table => {
            let mut table = comfy_table::Table::new();
            table.set_header(vec!["Name", "Health", "Bookmarks", "Branch", "Description"]);
            for (c, count) in results {
                table.add_row(vec![
                    c.name.clone(),
                    c.health.as_ref().map(|h| h.to_string()).unwrap_or_else(|| "-".to_string()),
                    count.to_string(),
                    c.created_branch.clone().unwrap_or_default(),
                    c.description.clone().unwrap_or_default(),
                ]);
            }
            println!("{table}");
        }
        _ => {
            let mut stdout = std::io::stdout().lock();
            for (c, count) in results {
                writeln!(
                    stdout,
                    "{}\t{}\t{}\t{}",
                    c.name,
                    count,
                    c.created_branch.as_deref().unwrap_or(""),
                    c.description.as_deref().unwrap_or("")
                )?;
            }
        }
    }
    Ok(())
}

/// Handle reindex command to rebuild embeddings.
#[cfg(feature = "semantic")]
pub async fn handle_reindex(cli: &Cli, mode: &OutputMode, args: &ReindexArgs) -> Result<()> {
    let config = load_config(cli);
    if !config.semantic.is_enabled() {
        return Err(Error::Input("Semantic search is not enabled in config".to_string()));
    }

    let mut db = open_db(cli)?;

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

    // Get models directory from config (defaults to global cache)
    let models_dir = config.semantic.get_models_dir();

    let semantic_repo = SemanticRepo::with_config(models_dir, model, distance_metric, threshold);

    // Decide what to reindex:
    // - `--collections` restricts to collections only.
    // - Otherwise bookmarks are reindexed; collections are also refreshed on a
    //   full reindex (no bookmark-specific `--lang`/`--collection` filter set).
    let bookmark_filtered = args.lang.is_some() || args.collection.is_some();
    let do_bookmarks = !args.collections;
    let do_collections = args.collections || !bookmark_filtered;

    let mut messages = Vec::new();

    if do_bookmarks {
        let filter = BookmarkFilter {
            language: args.lang.as_deref().map(|l| l.to_string()),
            collection: args.collection.as_deref().map(|c| c.to_string()),
            ..Default::default()
        };

        let bookmarks = db.list_bookmarks(&filter)?;
        if bookmarks.is_empty() {
            messages.push("No bookmarks to reindex".to_string());
        } else {
            if args.verbose {
                eprintln!("Reindexing {} bookmarks...", bookmarks.len());
            }
            let count = {
                let conn = db.conn_mut();
                semantic_repo.store_embeddings(conn, &bookmarks).await
            }?;
            messages.push(format!("Generated embeddings for {count} bookmarks"));
        }
    }

    if do_collections {
        let collections: Vec<_> = db.list_collections()?.into_iter().map(|(c, _count)| c).collect();
        if collections.is_empty() {
            messages.push("No collections to reindex".to_string());
        } else {
            if args.verbose {
                eprintln!("Reindexing {} collections...", collections.len());
            }
            let count = {
                let conn = db.conn_mut();
                semantic_repo.store_collection_embeddings(conn, &collections).await
            }?;
            messages.push(format!("Generated embeddings for {count} collections"));
        }
    }

    write_success(mode, &messages.join("; "))?;

    Ok(())
}
