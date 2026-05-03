//! Collection management: create, delete, add, remove, list, show, resolve, reorder.

use std::io::Write;

use crate::cli::output::{
    self, CollectionWithCount, OutputMode, write_json_success, write_success,
};
use crate::cli::*;
use codemark_core::engine::bookmark::{BookmarkFilter, BookmarkStatus, Collection, Visibility};
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
        updated_at: None,
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
                updated_at: None,
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
                table.set_header(vec!["Name", "Branch", "Description", "Created"]);
                for c in &collections {
                    table.add_row(vec![
                        &c.name,
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
                table.set_header(vec!["Name", "Bookmarks", "Branch", "Description", "Created"]);
                for (c, count) in &collections {
                    table.add_row(vec![
                        c.name.clone(),
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
        status: Some(vec![BookmarkStatus::Active, BookmarkStatus::Drifted]),
        ..Default::default()
    };
    let bookmarks = db.list_bookmarks(&filter)?;
    let config = super::load_config(cli);
    let results = resolve_batch(&db, &bookmarks, &config, false).await?;
    super::write_batch_output(mode, &results)?;
    Ok(())
}
