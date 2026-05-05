//! Search and indexing: FTS search, semantic search, reindex.

use crate::cli::output::{
    self, OutputMode, short_id, write_bookmarks_with_line, write_json_success, write_success,
};
use crate::cli::*;
use codemark_core::embeddings::config::EmbeddingModel;
use codemark_core::engine::bookmark::{Bookmark, BookmarkFilter};
use codemark_core::error::{Error, Result};
use codemark_core::storage::{SemanticRepo, db::Database};

use super::{
    filter_dbs_by_repo_owner, filter_dbs_by_user_email, get_bookmark_line, load_config,
    open_all_dbs_with_extra, open_db,
};

/// Search for bookmarks using full-text search or semantic search.
pub async fn handle_search(cli: &Cli, mode: &OutputMode, args: &SearchArgs) -> Result<()> {
    super::check_deprecated_status(&args.health, &args.status);
    let config = load_config(cli);

    // Semantic search requires a query
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
    let dbs = open_all_dbs_with_extra(cli, &args.add_db)?;
    let dbs = filter_dbs_by_user_email(dbs, args.user_email.as_deref());
    let dbs = filter_dbs_by_repo_owner(dbs, args.repo_owner.as_deref());

    let health_input = args.health.as_deref().or(args.status.as_deref());
    let health_filter = super::parse_status_filter(health_input);

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

            write_bookmarks_with_line(mode, &bookmarks, args.line_format.as_deref(), get_line_fn)?;
        } else {
            output::write_bookmarks(mode, &bookmarks, args.line_format.as_deref())?;
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
            )?;
        } else {
            output::write_annotated_bookmarks(
                mode,
                &annotated,
                args.line_format.as_deref(),
                None as Option<&fn(&str) -> Option<usize>>,
            )?;
        }
    }
    Ok(())
}

/// Handle semantic search using vector embeddings.
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

    // Fetch full bookmark details for results
    let mut bookmarks = Vec::new();
    for result in results {
        if let Ok(Some(bm)) = db.get_bookmark(&result.bookmark_id) {
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
            )?;
        } else {
            output::write_bookmarks(mode, &bookmarks_only, args.line_format.as_deref())?;
        }
    }

    Ok(())
}

/// Handle reindex command to rebuild embeddings.
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

    // Get bookmarks to reindex
    let filter = BookmarkFilter {
        language: args.lang.as_deref().map(|l| l.to_string()),
        collection: args.collection.as_deref().map(|c| c.to_string()),
        ..Default::default()
    };

    let bookmarks = db.list_bookmarks(&filter)?;

    if bookmarks.is_empty() {
        write_success(mode, "No bookmarks to reindex")?;
        return Ok(());
    }

    if args.verbose {
        eprintln!("Reindexing {} bookmarks...", bookmarks.len());
    }

    // Store embeddings
    let count = {
        let conn = db.conn_mut();
        semantic_repo.store_embeddings(conn, &bookmarks).await
    }?;

    let message = format!("Generated embeddings for {count} bookmarks");
    write_success(mode, &message)?;

    Ok(())
}
