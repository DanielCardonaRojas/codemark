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
            // Pre-resolve line numbers, keyed by (source_label, short_id) to unambiguously
            // handle bookmarks that share an 8-character ID prefix across repositories.
            let mut line_cache: std::collections::HashMap<(String, String), usize> =
                std::collections::HashMap::new();
            for (label, bm) in &all {
                let sid = short_id(&bm.id).to_string();
                let key = (label.clone(), sid);
                if !line_cache.contains_key(&key) {
                    if let Some(db) = db_map.get(label) {
                        if let Some(line) = get_bookmark_line(db, &bm.id, &bm.file_path) {
                            line_cache.insert(key, line);
                        }
                    }
                }
            }
            let get_line_fn = |label: &str, short_id: &str| -> Option<usize> {
                line_cache.get(&(label.to_string(), short_id.to_string())).copied()
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
                None as Option<&fn(&str, &str) -> Option<usize>>,
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
    let dbs = open_all_dbs_with_extra_and_repos(cli, &args.add_db, &args.repo)?;
    let dbs = filter_dbs_by_user_email(dbs, args.user_email.as_deref());
    let dbs = filter_dbs_by_repo_owner(dbs, args.repo_owner.as_deref());
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
    let threshold = config.semantic.effective_threshold();

    // Get models directory from config (defaults to global cache)
    let models_dir = config.semantic.get_models_dir();

    let semantic_repo = SemanticRepo::with_config(models_dir, model, distance_metric, threshold);

    // Embed the query ONCE (loads the model once), then reuse the vector across
    // every database's vec index — this is the embed-once fan-out.
    let embedding = semantic_repo.embed_query(query).await?;

    // Build health filter
    let health_input = args.health.as_deref().or(args.status.as_deref());
    let health_filter = super::parse_health_filter(health_input)?;

    let single_db = dbs.len() == 1;

    // Fetch full bookmark details for results and apply health filter. Each
    // hit carries its source label so multi-db output can be annotated.
    // A db whose vec index errors (missing/dimension mismatch) is skipped so
    // one bad repo doesn't abort the whole command; if EVERY db errors we
    // surface the last error below.
    let mut bookmarks: Vec<(f64, String, Bookmark)> = Vec::new();
    let mut any_ok = false;
    let mut last_err: Option<Error> = None;
    for (label, db) in &dbs {
        match semantic_repo.search_prepared(db.conn(), &embedding, args.limit, threshold) {
            Ok(results) => {
                any_ok = true;
                for result in results {
                    if let Ok(Some(bm)) = db.get_bookmark(&result.id) {
                        // Apply health filter if specified
                        if let Some(ref filter) = health_filter
                            && !filter.contains(&bm.health)
                        {
                            continue;
                        }
                        bookmarks.push((result.distance, label.clone(), bm));
                    }
                }
            }
            Err(e) => {
                eprintln!("warning: skipping '{label}' (semantic search failed: {e})");
                last_err = Some(e);
            }
        }
    }

    // Only fail hard if no db could be searched at all.
    if !any_ok && let Some(e) = last_err {
        return Err(e);
    }

    // Merge across dbs by ascending distance and cap at the limit.
    bookmarks.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    bookmarks.truncate(args.limit);

    // Output results
    if matches!(mode, OutputMode::Json) {
        let data: Vec<serde_json::Value> = bookmarks
            .into_iter()
            .map(|(distance, label, bm)| {
                // Collect all annotations for JSON output
                let annotations: Vec<_> = bm.annotations.iter().collect();
                let mut obj = serde_json::json!({
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
                });
                // For multi-db results, annotate the source repo label alongside
                // the existing fields; single-db output stays byte-for-byte the same.
                if !single_db && let Some(map) = obj.as_object_mut() {
                    map.insert("source".to_string(), serde_json::json!(label));
                }
                obj
            })
            .collect();
        write_json_success(&data)?;
    } else if single_db {
        // Single-db non-JSON: use the standard (unannotated) bookmark output.
        let db = &dbs[0].1;
        let bookmarks_only: Vec<Bookmark> = bookmarks.iter().map(|(_, _, bm)| bm.clone()).collect();

        // Check if we need line numbers
        let needs_line = mode.needs_line()
            || args.line_format.as_deref().is_some_and(output::template_needs_line);

        if needs_line {
            let get_line_fn = |id: &str| -> Option<usize> {
                for bm in &bookmarks_only {
                    if short_id(&bm.id) == id {
                        return get_bookmark_line(db, &bm.id, &bm.file_path);
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
    } else {
        // Multi-db non-JSON: annotate each hit with its source repo label,
        // mirroring the FTS multi-db branch.
        let db_map: std::collections::HashMap<&str, &Database> =
            dbs.iter().map(|(label, db)| (label.as_str(), db)).collect();

        let annotated: Vec<output::AnnotatedBookmark> = bookmarks
            .iter()
            .map(|(_, label, bm)| output::AnnotatedBookmark { source: label, bookmark: bm })
            .collect();

        let needs_line = mode.needs_line()
            || args.line_format.as_deref().is_some_and(output::template_needs_line);

        if needs_line {
            // Pre-resolve line numbers, keyed by (source_label, short_id) to unambiguously
            // handle bookmarks that share an 8-character ID prefix across repositories.
            let mut line_cache: std::collections::HashMap<(String, String), usize> =
                std::collections::HashMap::new();
            for (_, label, bm) in &bookmarks {
                let sid = short_id(&bm.id).to_string();
                let key = (label.clone(), sid);
                if !line_cache.contains_key(&key) {
                    if let Some(db) = db_map.get(label.as_str()) {
                        if let Some(line) = get_bookmark_line(db, &bm.id, &bm.file_path) {
                            line_cache.insert(key, line);
                        }
                    }
                }
            }
            let get_line_fn = |label: &str, short_id: &str| -> Option<usize> {
                line_cache.get(&(label.to_string(), short_id.to_string())).copied()
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
                None as Option<&fn(&str, &str) -> Option<usize>>,
                None,
            )?;
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
    // A source label is only meaningful when more than one db is in scope; it
    // stays `None` on the single-db path so output is byte-for-byte unchanged.
    #[cfg(feature = "semantic")]
    let results: Vec<(Option<String>, codemark_core::engine::bookmark::Collection, usize)> = if args
        .semantic
    {
        collection_semantic_search(cli, args, config).await?
    } else {
        let dbs = open_all_dbs_with_extra_and_repos(cli, &args.add_db, &args.repo)?;
        let dbs = filter_dbs_by_user_email(dbs, args.user_email.as_deref());
        let dbs = filter_dbs_by_repo_owner(dbs, args.repo_owner.as_deref());
        let single_db = dbs.len() == 1;

        let mut out: Vec<(Option<String>, codemark_core::engine::bookmark::Collection, usize)> = Vec::new();
        let mut any_ok = false;
        let mut last_err: Option<Error> = None;

        for (label, db) in &dbs {
            match db.search_collections(args.query.as_deref(), args.tag.as_deref()) {
                Ok(mut collections) => {
                    any_ok = true;
                    collections.truncate(args.limit);
                    for (c, count) in collections {
                        let source = if single_db { None } else { Some(label.clone()) };
                        out.push((source, c, count));
                    }
                }
                Err(e) => {
                    eprintln!("warning: skipping '{label}' (collection search failed: {e})");
                    last_err = Some(e);
                }
            }
        }
        if !any_ok && let Some(e) = last_err {
            return Err(e);
        }
        out.truncate(args.limit);
        out
    };

    #[cfg(not(feature = "semantic"))]
    let results: Vec<(Option<String>, codemark_core::engine::bookmark::Collection, usize)> = {
        let _ = config; // only the semantic branch reads config
        let dbs = open_all_dbs_with_extra_and_repos(cli, &args.add_db, &args.repo)?;
        let dbs = filter_dbs_by_user_email(dbs, args.user_email.as_deref());
        let dbs = filter_dbs_by_repo_owner(dbs, args.repo_owner.as_deref());
        let single_db = dbs.len() == 1;

        let mut out: Vec<(Option<String>, codemark_core::engine::bookmark::Collection, usize)> = Vec::new();
        let mut any_ok = false;
        let mut last_err: Option<Error> = None;

        for (label, db) in &dbs {
            match db.search_collections(args.query.as_deref(), args.tag.as_deref()) {
                Ok(mut collections) => {
                    any_ok = true;
                    collections.truncate(args.limit);
                    for (c, count) in collections {
                        let source = if single_db { None } else { Some(label.clone()) };
                        out.push((source, c, count));
                    }
                }
                Err(e) => {
                    eprintln!("warning: skipping '{label}' (collection search failed: {e})");
                    last_err = Some(e);
                }
            }
        }
        if !any_ok && let Some(e) = last_err {
            return Err(e);
        }
        out.truncate(args.limit);
        out
    };

    write_collection_results(mode, &results)?;
    Ok(())
}

/// Rank collections by semantic similarity across all specified repos.
///
/// Embeds the query once, searches each db's collection vec index with the same
/// vector, resolves each hit to its full collection with bookmark count (applying
/// the `--tag` filter), merges by ascending distance and caps at the limit. When
/// more than one db is in scope, each result carries its source repo label.
#[cfg(feature = "semantic")]
async fn collection_semantic_search(
    cli: &Cli,
    args: &SearchArgs,
    config: &codemark_core::config::Config,
) -> Result<Vec<(Option<String>, codemark_core::engine::bookmark::Collection, usize)>> {
    if !config.semantic.is_enabled() {
        return Err(Error::Input("Semantic search is not enabled in config".to_string()));
    }
    let query = args
        .query
        .as_ref()
        .ok_or_else(|| Error::Input("Semantic search requires a query".to_string()))?;

    let dbs = open_all_dbs_with_extra_and_repos(cli, &args.add_db, &args.repo)?;
    let dbs = filter_dbs_by_user_email(dbs, args.user_email.as_deref());
    let dbs = filter_dbs_by_repo_owner(dbs, args.repo_owner.as_deref());
    let single_db = dbs.len() == 1;

    let model = config
        .semantic
        .model
        .as_deref()
        .and_then(|m| m.parse::<EmbeddingModel>().ok())
        .unwrap_or(EmbeddingModel::AllMiniLmL6V2);
    let distance_metric = config.semantic.get_distance_metric();
    let threshold = config.semantic.effective_threshold();
    let models_dir = config.semantic.get_models_dir();

    let semantic_repo = SemanticRepo::with_config(models_dir, model, distance_metric, threshold);

    // Embed the query ONCE (loads the model once), reuse across every db.
    let embedding = semantic_repo.embed_query(query).await?;

    // Resolve each hit to its full collection with bookmark count, applying the
    // same `--tag` filter the FTS branch does (semantic search ranks on
    // embeddings, so tag membership has to be enforced here). A db whose vec
    // index errors is skipped; if EVERY db errors we surface the last error.
    let mut out: Vec<(f64, Option<String>, codemark_core::engine::bookmark::Collection, usize)> =
        Vec::new();
    let mut any_ok = false;
    let mut last_err: Option<Error> = None;
    for (label, db) in &dbs {
        let hits = match semantic_repo.search_collections_prepared(
            db.conn(),
            &embedding,
            args.limit,
            threshold,
        ) {
            Ok(hits) => hits,
            Err(e) => {
                eprintln!("warning: skipping '{label}' (collection semantic search failed: {e})");
                last_err = Some(e);
                continue;
            }
        };
        any_ok = true;
        for hit in hits {
            // Use match-and-continue instead of `?` so a single DB failure
            // doesn't abort the entire multi-repo command.
            let Some(c) = db.get_collection_by_id(&hit.id).ok().flatten() else {
                continue;
            };
            if let Some(tag) = args.tag.as_deref() {
                let has_tag = db
                    .list_tags_for_collection(&c.id)
                    .map(|tags| tags.iter().any(|t| t.tag == tag))
                    .unwrap_or(false);
                if !has_tag {
                    continue;
                }
            }
            let count = db.list_bookmarks_in_collection(&c.id).map(|b| b.len()).unwrap_or(0);
            let source = if single_db { None } else { Some(label.clone()) };
            out.push((hit.distance, source, c, count));
        }
    }

    if !any_ok && let Some(e) = last_err {
        return Err(e);
    }

    // Merge across dbs by ascending distance and cap at the limit.
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(args.limit);

    Ok(out.into_iter().map(|(_, source, c, count)| (source, c, count)).collect())
}

/// Render collection search results in the requested output mode.
///
/// A `Some` source label (only present on multi-db results) is annotated
/// alongside the existing fields; single-db output is byte-for-byte unchanged.
fn write_collection_results(
    mode: &OutputMode,
    results: &[(Option<String>, codemark_core::engine::bookmark::Collection, usize)],
) -> Result<()> {
    use std::io::Write;

    let multi_db = results.iter().any(|(source, _, _)| source.is_some());

    match mode {
        OutputMode::Json => {
            let data: Vec<serde_json::Value> = results
                .iter()
                .map(|(source, c, count)| {
                    let mut obj = serde_json::json!({
                        "id": c.id,
                        "short_id": short_id(&c.id),
                        "name": c.name,
                        "description": c.description,
                        "health": c.health.as_ref().map(|h| h.to_string()),
                        "bookmark_count": count,
                        "created_at": c.created_at,
                        "created_by": c.created_by,
                        "created_branch": c.created_branch,
                    });
                    if let Some(label) = source
                        && let Some(map) = obj.as_object_mut()
                    {
                        map.insert("source".to_string(), serde_json::json!(label));
                    }
                    obj
                })
                .collect();
            write_json_success(&data)?;
        }
        OutputMode::Table => {
            let mut table = comfy_table::Table::new();
            if multi_db {
                table.set_header(vec![
                    "Source",
                    "Name",
                    "Health",
                    "Bookmarks",
                    "Branch",
                    "Description",
                ]);
            } else {
                table.set_header(vec!["Name", "Health", "Bookmarks", "Branch", "Description"]);
            }
            for (source, c, count) in results {
                let mut row = Vec::new();
                if multi_db {
                    row.push(source.clone().unwrap_or_default());
                }
                row.push(c.name.clone());
                row.push(
                    c.health.as_ref().map(|h| h.to_string()).unwrap_or_else(|| "-".to_string()),
                );
                row.push(count.to_string());
                row.push(c.created_branch.clone().unwrap_or_default());
                row.push(c.description.clone().unwrap_or_default());
                table.add_row(row);
            }
            println!("{table}");
        }
        _ => {
            let mut stdout = std::io::stdout().lock();
            for (source, c, count) in results {
                if let Some(label) = source {
                    writeln!(
                        stdout,
                        "{}\t{}\t{}\t{}\t{}",
                        label,
                        c.name,
                        count,
                        c.created_branch.as_deref().unwrap_or(""),
                        c.description.as_deref().unwrap_or("")
                    )?;
                } else {
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
    let threshold = config.semantic.effective_threshold();

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
