//! Collection management: create, delete, add, remove, list, show, resolve, reorder.

use std::io::Write;

use crate::cli::output::{
    self, CollectionWithCount, OutputMode, write_json_success, write_success,
};
use crate::cli::*;
use codemark_core::engine::bookmark::{
    BookmarkFilter, BookmarkHealth, Collection, CollectionLink, CollectionLinkKind, CollectionTag,
    Visibility,
};
use codemark_core::error::{Error, Result};

use super::{find_bookmark, now_iso, open_db, open_db_for_write, resolve_batch};

/// Create a new bookmark collection.
pub async fn handle_collection_create(
    cli: &Cli,
    mode: &OutputMode,
    args: &CollectionCreateArgs,
) -> Result<()> {
    let db = open_db_for_write(cli)?;

    let cwd = std::env::current_dir()?;
    let git_ctx = codemark_core::git::context::detect_context(&cwd);
    let created_branch = git_ctx.and_then(|ctx| ctx.branch_name);

    let collection = Collection {
        id: uuid::Uuid::new_v4().to_string(),
        name: args.name.clone(),
        description: args.description.clone(),
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
    db.insert_collection(&collection)?;
    write_success(mode, &format!("Collection '{}' created", args.name))?;
    Ok(())
}

/// Delete a bookmark collection and optionally its bookmarks.
pub async fn handle_collection_delete(
    cli: &Cli,
    mode: &OutputMode,
    args: &CollectionDeleteArgs,
) -> Result<()> {
    let mut db = open_db_for_write(cli)?;
    // Try by name first, then by ID prefix
    let collection = if let Some(c) = db.get_collection_by_name(&args.name)? {
        c
    } else {
        db.get_collection_by_id_prefix(&args.name)?
            .ok_or_else(|| Error::Input(format!("collection '{}' not found", args.name)))?
    };

    if args.with_bookmarks {
        let bm_count = db.delete_collection_recursive(&collection.id)?;
        write_success(
            mode,
            &format!("Collection '{}' and its {bm_count} bookmarks deleted", collection.name),
        )?;
    } else {
        let count = db.delete_collection_by_id(&collection.id)?;
        write_success(
            mode,
            &format!("Collection '{}' deleted ({count} bookmarks were in it)", collection.name),
        )?;
    }

    Ok(())
}

/// Add one or more bookmarks to a collection.
pub async fn handle_collection_add(
    cli: &Cli,
    mode: &OutputMode,
    args: &CollectionAddArgs,
) -> Result<()> {
    let db = open_db_for_write(cli)?;
    // Auto-create collection if it doesn't exist
    let collection = match db.get_collection_by_name(&args.name)? {
        Some(c) => c,
        None => {
            let cwd = std::env::current_dir()?;
            let git_ctx = codemark_core::git::context::detect_context(&cwd);
            let created_branch = git_ctx.and_then(|ctx| ctx.branch_name);

            let c = Collection {
                id: uuid::Uuid::new_v4().to_string(),
                name: args.name.clone(),
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
    let added = db.add_to_collection_at(&collection.id, &args.bookmark_ids, args.at)?;
    write_success(mode, &format!("Added {added} bookmarks to '{}'", args.name))?;
    Ok(())
}

/// Reorder bookmarks within a collection.
pub async fn handle_collection_reorder(
    cli: &Cli,
    mode: &OutputMode,
    args: &CollectionReorderArgs,
) -> Result<()> {
    let db = open_db_for_write(cli)?;
    // Try by name first, then by ID prefix
    let collection = if let Some(c) = db.get_collection_by_name(&args.name)? {
        c
    } else {
        db.get_collection_by_id_prefix(&args.name)?
            .ok_or_else(|| Error::Input(format!("collection '{}' not found", args.name)))?
    };

    db.reorder_collection(&collection.id, &args.bookmark_ids)?;
    write_success(
        mode,
        &format!("Reordered {} bookmarks in '{}'", args.bookmark_ids.len(), collection.name),
    )?;
    Ok(())
}

/// Remove one or more bookmarks from a collection.
pub async fn handle_collection_remove(
    cli: &Cli,
    mode: &OutputMode,
    args: &CollectionRemoveArgs,
) -> Result<()> {
    let db = open_db_for_write(cli)?;
    // Try by name first, then by ID prefix
    let collection = if let Some(c) = db.get_collection_by_name(&args.name)? {
        c
    } else {
        db.get_collection_by_id_prefix(&args.name)?
            .ok_or_else(|| Error::Input(format!("collection '{}' not found", args.name)))?
    };

    let removed = db.remove_from_collection(&collection.id, &args.bookmark_ids)?;
    if let Err(e) = db.recompute_collection_health(&collection.id) {
        eprintln!(
            "codemark: warning: failed to recompute health for collection {}: {}",
            collection.id, e
        );
    }
    write_success(mode, &format!("Removed {removed} bookmarks from '{}'", collection.name))?;
    Ok(())
}

/// List all bookmark collections.
pub async fn handle_collection_list(
    cli: &Cli,
    mode: &OutputMode,
    args: &CollectionListArgs,
) -> Result<()> {
    let db = open_db(cli)?;

    if let Some(ref bookmark_id) = args.bookmark {
        let bm = find_bookmark(&db, bookmark_id)?;
        let collections = db.list_collections_for_bookmark(&bm.id)?;

        // Custom line format for bookmark's collections
        if let Some(ref template) = args.line_format {
            let mut stdout = std::io::stdout().lock();
            for c in &collections {
                let short_id = output::short_id(&c.id);
                let line = template
                    .replace("{ID}", short_id)
                    .replace("{id}", short_id)
                    .replace("{NAME}", &c.name)
                    .replace("{name}", &c.name)
                    .replace(
                        "{HEALTH}",
                        &c.health
                            .as_ref()
                            .map(|h| h.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                    )
                    .replace(
                        "{health}",
                        &c.health
                            .as_ref()
                            .map(|h| h.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                    )
                    .replace("{DESCRIPTION}", c.description.as_deref().unwrap_or(""))
                    .replace("{description}", c.description.as_deref().unwrap_or(""))
                    .replace("{CREATED}", &c.created_at)
                    .replace("{created}", &c.created_at)
                    .replace("{BRANCH}", c.created_branch.as_deref().unwrap_or(""))
                    .replace("{branch}", c.created_branch.as_deref().unwrap_or(""));
                writeln!(stdout, "{line}")?;
            }
            return Ok(());
        }

        match mode {
            OutputMode::Json => write_json_success(&collections)?,
            OutputMode::Table => {
                let mut table = comfy_table::Table::new();
                table.set_header(vec!["Name", "Health", "Branch", "Description", "Created"]);
                for c in &collections {
                    table.add_row(vec![
                        &c.name,
                        &c.health
                            .as_ref()
                            .map(|h| h.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                        c.created_branch.as_deref().unwrap_or(""),
                        c.description.as_deref().unwrap_or(""),
                        &c.created_at,
                    ]);
                }
                println!("{table}");
            }
            _ => {
                let mut stdout = std::io::stdout().lock();
                for c in &collections {
                    writeln!(
                        stdout,
                        "{}\t{}\t{}",
                        c.name,
                        c.created_branch.as_deref().unwrap_or(""),
                        c.description.as_deref().unwrap_or("")
                    )?;
                }
            }
        }
    } else {
        let collections = db.list_collections()?;

        // Custom line format for collections with counts
        if let Some(ref template) = args.line_format {
            let mut stdout = std::io::stdout().lock();
            for (c, count) in &collections {
                let short_id = output::short_id(&c.id);
                let line = template
                    .replace("{ID}", short_id)
                    .replace("{id}", short_id)
                    .replace("{NAME}", &c.name)
                    .replace("{name}", &c.name)
                    .replace(
                        "{HEALTH}",
                        &c.health
                            .as_ref()
                            .map(|h| h.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                    )
                    .replace(
                        "{health}",
                        &c.health
                            .as_ref()
                            .map(|h| h.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                    )
                    .replace("{COUNT}", &count.to_string())
                    .replace("{count}", &count.to_string())
                    .replace("{DESCRIPTION}", c.description.as_deref().unwrap_or(""))
                    .replace("{description}", c.description.as_deref().unwrap_or(""))
                    .replace("{CREATED}", &c.created_at)
                    .replace("{created}", &c.created_at)
                    .replace("{BRANCH}", c.created_branch.as_deref().unwrap_or(""))
                    .replace("{branch}", c.created_branch.as_deref().unwrap_or(""));
                writeln!(stdout, "{line}")?;
            }
            return Ok(());
        }

        match mode {
            OutputMode::Json => {
                let with_counts: Vec<CollectionWithCount> =
                    collections.iter().map(CollectionWithCount::from).collect();
                write_json_success(&with_counts)?
            }
            OutputMode::Table => {
                let mut table = comfy_table::Table::new();
                table.set_header(vec![
                    "Name",
                    "Health",
                    "Bookmarks",
                    "Branch",
                    "Description",
                    "Created",
                ]);
                for (c, count) in &collections {
                    table.add_row(vec![
                        c.name.clone(),
                        c.health.as_ref().map(|h| h.to_string()).unwrap_or_else(|| "-".to_string()),
                        count.to_string(),
                        c.created_branch.clone().unwrap_or_default(),
                        c.description.clone().unwrap_or_default(),
                        c.created_at.clone(),
                    ]);
                }
                println!("{table}");
            }
            _ => {
                let mut stdout = std::io::stdout().lock();
                for (c, count) in &collections {
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

/// Show all bookmarks in a collection.
pub async fn handle_collection_show(
    cli: &Cli,
    mode: &OutputMode,
    args: &CollectionShowArgs,
) -> Result<()> {
    let db = open_db(cli)?;
    // Try by name first, then by ID prefix
    let collection = if let Some(c) = db.get_collection_by_name(&args.name)? {
        c
    } else {
        db.get_collection_by_id_prefix(&args.name)?
            .ok_or_else(|| Error::Input(format!("collection '{}' not found", args.name)))?
    };

    let filter = BookmarkFilter { collection_id: Some(collection.id), ..Default::default() };
    let bookmarks = db.list_bookmarks(&filter)?;
    output::write_bookmarks(mode, &bookmarks, None)?;
    Ok(())
}

/// Resolve all bookmarks in a collection.
pub async fn handle_collection_resolve(
    cli: &Cli,
    mode: &OutputMode,
    args: &CollectionResolveArgs,
) -> Result<()> {
    let db = open_db_for_write(cli)?;
    // Try by name first, then by ID prefix
    let collection = if let Some(c) = db.get_collection_by_name(&args.name)? {
        c
    } else {
        db.get_collection_by_id_prefix(&args.name)?
            .ok_or_else(|| Error::Input(format!("collection '{}' not found", args.name)))?
    };

    let filter = BookmarkFilter {
        collection_id: Some(collection.id),
        health: Some(vec![BookmarkHealth::Active, BookmarkHealth::Drifted]),
        ..Default::default()
    };
    let bookmarks = db.list_bookmarks(&filter)?;
    let config = super::load_config(cli);
    let results = resolve_batch(&db, &bookmarks, &config, false).await?;
    super::write_batch_output(mode, &results)?;
    Ok(())
}

/// Handle collection tag subcommands.
pub async fn handle_collection_tag(
    cli: &Cli,
    mode: &OutputMode,
    args: &CollectionTagArgs,
) -> Result<()> {
    match &args.command {
        CollectionTagCommand::Add(add_args) => {
            let db = open_db_for_write(cli)?;
            let collection = db
                .get_collection_by_name(&add_args.name)?
                .ok_or_else(|| Error::Input(format!("collection '{}' not found", add_args.name)))?;

            let now = now_iso();
            let tags: Vec<CollectionTag> = add_args
                .tags
                .iter()
                .map(|t| CollectionTag {
                    collection_id: collection.id.clone(),
                    tag: t.clone(),
                    added_at: now.clone(),
                    added_by: None,
                })
                .collect();

            db.insert_collection_tags(&tags)?;
            write_success(
                mode,
                &format!("Added {} tags to collection '{}'", add_args.tags.len(), add_args.name),
            )?;
        }
        CollectionTagCommand::Rm(rm_args) => {
            let db = open_db_for_write(cli)?;
            let collection = db
                .get_collection_by_name(&rm_args.name)?
                .ok_or_else(|| Error::Input(format!("collection '{}' not found", rm_args.name)))?;

            for tag in &rm_args.tags {
                db.delete_collection_tag(&collection.id, tag)?;
            }
            write_success(
                mode,
                &format!("Removed {} tags from collection '{}'", rm_args.tags.len(), rm_args.name),
            )?;
        }
        CollectionTagCommand::List(list_args) => {
            let db = open_db(cli)?;
            let collection = db.get_collection_by_name(&list_args.name)?.ok_or_else(|| {
                Error::Input(format!("collection '{}' not found", list_args.name))
            })?;

            let tags = db.list_tags_for_collection(&collection.id)?;
            let filtered_tags: Vec<_> = if list_args.include_auto {
                tags
            } else {
                tags.into_iter().filter(|t| t.added_by.as_deref() != Some("__auto__")).collect()
            };

            match mode {
                OutputMode::Json => write_json_success(&filtered_tags)?,
                _ => {
                    let mut stdout = std::io::stdout().lock();
                    for t in filtered_tags {
                        if list_args.include_auto {
                            writeln!(stdout, "{}\t{}", t.tag, t.added_by.as_deref().unwrap_or(""))?;
                        } else {
                            writeln!(stdout, "{}", t.tag)?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Handle collection link subcommands.
pub async fn handle_collection_link(
    cli: &Cli,
    mode: &OutputMode,
    args: &CollectionLinkArgs,
) -> Result<()> {
    match &args.command {
        CollectionLinkCommand::Add(add_args) => {
            let db = open_db_for_write(cli)?;
            let collection = db
                .get_collection_by_name(&add_args.name)?
                .ok_or_else(|| Error::Input(format!("collection '{}' not found", add_args.name)))?;

            let kind: CollectionLinkKind = add_args
                .kind
                .parse()
                .map_err(|_| Error::Input(format!("invalid link kind: {}", add_args.kind)))?;

            let link = CollectionLink {
                id: uuid::Uuid::new_v4().to_string(),
                collection_id: collection.id.clone(),
                kind,
                label: add_args.label.clone(),
                url: add_args.url.clone(),
                sort_order: add_args.position.unwrap_or(0),
                added_at: now_iso(),
                added_by: None,
            };

            db.insert_collection_link(&link)?;
            write_success(
                mode,
                &format!(
                    "Added link to collection '{}' (ID: {})",
                    add_args.name,
                    output::short_id(&link.id)
                ),
            )?;
        }
        CollectionLinkCommand::Rm(rm_args) => {
            let db = open_db_for_write(cli)?;
            let collection = db
                .get_collection_by_name(&rm_args.name)?
                .ok_or_else(|| Error::Input(format!("collection '{}' not found", rm_args.name)))?;

            let deleted = db.delete_collection_link(&rm_args.id, &collection.id)?;
            if deleted {
                write_success(mode, &format!("Removed link from collection '{}'", rm_args.name))?;
            } else {
                return Err(Error::Input(format!(
                    "link '{}' not found in collection '{}'",
                    rm_args.id, rm_args.name
                )));
            }
        }
        CollectionLinkCommand::List(list_args) => {
            let db = open_db(cli)?;
            let collection = db.get_collection_by_name(&list_args.name)?.ok_or_else(|| {
                Error::Input(format!("collection '{}' not found", list_args.name))
            })?;

            let links = db.list_links_for_collection(&collection.id)?;
            match mode {
                OutputMode::Json => write_json_success(&links)?,
                OutputMode::Table => {
                    let mut table = comfy_table::Table::new();
                    table.set_header(vec!["ID", "Kind", "Label", "URL"]);
                    for l in links {
                        table.add_row(vec![
                            output::short_id(&l.id),
                            &l.kind.to_string(),
                            &l.label,
                            &l.url,
                        ]);
                    }
                    println!("{table}");
                }
                _ => {
                    let mut stdout = std::io::stdout().lock();
                    for l in links {
                        writeln!(
                            stdout,
                            "{}\t{}\t{}\t{}",
                            output::short_id(&l.id),
                            l.kind,
                            l.label,
                            l.url
                        )?;
                    }
                }
            }
        }
        CollectionLinkCommand::Reorder(reorder_args) => {
            let db = open_db_for_write(cli)?;
            let collection = db.get_collection_by_name(&reorder_args.name)?.ok_or_else(|| {
                Error::Input(format!("collection '{}' not found", reorder_args.name))
            })?;

            db.reorder_collection_links(&collection.id, &reorder_args.ids)?;
            write_success(mode, &format!("Reordered links in collection '{}'", reorder_args.name))?;
        }
    }
    Ok(())
}
