//! Repository registry management handlers.

use crate::cli::output::{OutputMode, write_json_success};
use crate::cli::*;
use codemark_core::error::{Error, Result};
use codemark_core::storage::registry;
use std::path::PathBuf;

/// Handle the `codemark repo` subcommand.
pub async fn handle_repo(cli: &Cli, mode: &OutputMode, args: &RepoArgs) -> Result<()> {
    match &args.command {
        RepoCommand::List(_) => handle_repo_list(mode),
        RepoCommand::ShowRepo(args) => handle_repo_show(cli, mode, args),
        RepoCommand::SetServer(args) => handle_repo_set_server(cli, mode, args),
        RepoCommand::Sync(_) => handle_repo_sync(cli, mode),
        RepoCommand::Prune(args) => handle_repo_prune(mode, args),
    }
}

/// Handle `codemark repo sync` - reconcile the current repository's path in the registry.
///
/// Intended to be run from the repository's new location after it has been moved on
/// disk. It refreshes the local repos table and reconciles the global registry (keyed
/// on `repo_root` and, for repos with an origin, matched by `(repo_owner, repo_name)`
/// plus `origin_url`), so a moved or not-yet-registered repo is recorded at its current
/// path without recreating .codemark/.
fn handle_repo_sync(cli: &Cli, mode: &OutputMode) -> Result<()> {
    let db = super::open_db(cli)?;
    let config = super::load_config(cli);
    let (db_owner_email, db_owner_name) = super::resolve_identity(&config);

    // The path the registry row should end up at after the sync. Captured before the
    // sync so the success check below can require the row to actually point here.
    let cwd = std::env::current_dir()?;
    let expected_root = codemark_core::git::context::detect_context(&cwd).map(|ctx| ctx.repo_root);

    let repo_id = super::resolve_or_create_repo_metadata(
        &db,
        &config,
        &db_owner_email,
        db_owner_name.as_deref(),
    )?
    .ok_or_else(|| Error::Input("Not in a git repository".into()))?;

    let conn = registry::open_registry()?;
    // The registry write happens via the (intentionally non-fatal) sync inside
    // resolve_or_create_repo_metadata. Confirm the row actually landed at the current
    // path before reporting success — `repo sync` is run precisely when the registry is
    // broken. Requiring the path (not just the id) guards against a silently-failed
    // write where a stale row sharing this local id still points at the old location.
    let repo = registry::list_repos(&conn)?
        .into_iter()
        .find(|r| r.id == repo_id && expected_root.as_ref().is_none_or(|er| &r.repo_root == er))
        .ok_or_else(|| {
            Error::Operation("Failed to write repository to the global registry".into())
        })?;

    match mode {
        OutputMode::Json => {
            write_json_success(&serde_json::json!({ "synced": true, "repo": repo }))?;
        }
        _ => {
            println!("Synced repository to registry:");
            println!("  {}/{}", repo.repo_owner, repo.repo_name);
            println!("  Root: {}", repo.repo_root.display());
        }
    }

    Ok(())
}

/// Handle `codemark repo prune` - remove registry entries whose path no longer exists.
fn handle_repo_prune(mode: &OutputMode, args: &RepoPruneArgs) -> Result<()> {
    let conn = registry::open_registry()?;

    let removed = if args.dry_run {
        registry::find_stale_repos(&conn)?
    } else {
        registry::prune_repos(&conn)?
    };

    match mode {
        OutputMode::Json => {
            write_json_success(&serde_json::json!({
                "dry_run": args.dry_run,
                "removed": removed,
            }))?;
        }
        _ => {
            if removed.is_empty() {
                println!("No stale repositories to prune.");
            } else {
                let verb = if args.dry_run { "Would remove" } else { "Removed" };
                println!(
                    "{} {} stale repositor{}:",
                    verb,
                    removed.len(),
                    if removed.len() == 1 { "y" } else { "ies" }
                );
                for repo in &removed {
                    println!(
                        "  {}/{} — {}",
                        repo.repo_owner,
                        repo.repo_name,
                        repo.repo_root.display()
                    );
                }
            }
        }
    }

    Ok(())
}

/// Handle `codemark repo list` - list all known repositories in the global registry.
fn handle_repo_list(mode: &OutputMode) -> Result<()> {
    let conn = registry::open_registry()?;
    let repos = registry::list_repos(&conn)?;

    match mode {
        OutputMode::Json => {
            write_json_success(&serde_json::json!({ "repos": repos }))?;
        }
        _ => {
            if repos.is_empty() {
                println!("No repositories registered in the global registry.");
                println!("Run any codemark command in a git repo to automatically register it.");
            } else {
                println!("Registered repositories ({}):\n", repos.len());
                for repo in &repos {
                    println!(
                        "  {}/{}  {}",
                        repo.repo_owner,
                        repo.repo_name,
                        repo.repo_root.display()
                    );
                    if let Some(ref url) = repo.origin_url {
                        println!("    URL: {}", url);
                    }
                    if let Some(ref server) = repo.server_url {
                        println!("    Server: {}", server);
                    }
                    println!(
                        "    DB Owner: {}{}\n",
                        repo.db_owner_email,
                        repo.db_owner_name
                            .as_ref()
                            .map(|n| format!(" ({})", n))
                            .unwrap_or_default()
                    );
                }
            }
        }
    }

    Ok(())
}

/// Handle `codemark repo show` - show details for a repository.
fn handle_repo_show(cli: &Cli, mode: &OutputMode, args: &RepoShowArgs) -> Result<()> {
    let conn = registry::open_registry()?;

    let repo = if let Some(ref repo_ref) = args.repo {
        // Look up by owner/name
        registry::find_repo_by_owner_name(&conn, repo_ref)?.ok_or_else(|| {
            Error::Input(format!("Repository '{}' not found in registry", repo_ref))
        })?
    } else {
        // Show current repo
        let db = super::open_db(cli)?;
        let cwd = std::env::current_dir()?;

        let git_ctx = codemark_core::git::context::detect_context(&cwd)
            .ok_or_else(|| Error::Input("Not in a git repository".into()))?;

        let repo_root_str = git_ctx.repo_root.to_string_lossy().to_string();

        // First try to find in local repos table
        if let Ok(Some(_local_repo)) = db.get_repo_by_root(&repo_root_str) {
            // Get full info from registry
            registry::find_repo_by_root(&conn, &repo_root_str)?.ok_or_else(|| {
                Error::Input(format!(
                    "Repository '{}' not found in global registry.\n\
                    Run any codemark command to sync it.",
                    repo_root_str
                ))
            })?
        } else {
            return Err(Error::Input(
                "Current repository not yet tracked by codemark.\n\
                Run any codemark command (like `codemark list`) to register it."
                    .into(),
            ));
        }
    };

    match mode {
        OutputMode::Json => {
            write_json_success(&serde_json::json!({ "repo": repo }))?;
        }
        _ => {
            println!("Repository: {}/{}", repo.repo_owner, repo.repo_name);
            println!("ID: {}", repo.id);
            println!("Root: {}", repo.repo_root.display());
            if let Some(ref url) = repo.origin_url {
                println!("Origin: {}", url);
            }
            println!(
                "Database Owner: {}{}",
                repo.db_owner_email,
                repo.db_owner_name.as_ref().map(|n| format!(" ({})", n)).unwrap_or_default()
            );
            println!("Detected: {}", repo.detected_at);
            println!("Last Seen: {}", repo.last_seen_at);
            if let Some(ref server) = repo.server_url {
                println!("Server: {}", server);
            } else {
                println!("Server: (not configured)");
            }
        }
    }

    Ok(())
}

/// Handle `codemark repo set-server` - set the server URL for a repository.
fn handle_repo_set_server(cli: &Cli, mode: &OutputMode, args: &RepoSetServerArgs) -> Result<()> {
    let conn = registry::open_registry()?;

    let repo_root = if let Some(ref repo_ref) = args.repo {
        // Look up by owner/name to get repo_root
        let repo = registry::find_repo_by_owner_name(&conn, repo_ref)?.ok_or_else(|| {
            Error::Input(format!("Repository '{}' not found in registry", repo_ref))
        })?;
        repo.repo_root.clone()
    } else {
        // Use current repo
        let db = super::open_db(cli)?;
        let cwd = std::env::current_dir()?;

        let git_ctx = codemark_core::git::context::detect_context(&cwd)
            .ok_or_else(|| Error::Input("Not in a git repository".into()))?;

        let repo_root_str = git_ctx.repo_root.to_string_lossy().to_string();

        // Verify it exists in local db
        db.get_repo_by_root(&repo_root_str)
            .map_err(|_| {
                Error::Input(
                    "Current repository not yet tracked by codemark.\n\
                Run any codemark command (like `codemark list`) to register it."
                        .into(),
                )
            })?
            .ok_or_else(|| {
                Error::Input("Current repository not found in local database.".into())
            })?;

        PathBuf::from(repo_root_str)
    };

    let repo_root_str = repo_root.to_string_lossy().to_string();
    registry::set_server_url(&conn, &repo_root_str, Some(&args.server))?;

    match mode {
        OutputMode::Json => {
            write_json_success(&serde_json::json!({
                "repo_root": repo_root_str,
                "server_url": args.server
            }))?;
        }
        _ => {
            println!("Server URL set for repository:");
            println!("  Root: {}", repo_root_str);
            println!("  Server: {}", args.server);
        }
    }

    Ok(())
}
