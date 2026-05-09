//! Repository registry management handlers.

use crate::cli::output::{OutputMode, write_json_success};
use crate::cli::*;
use codemark_core::error::{Error, Result};
use codemark_core::storage::registry;
use std::path::PathBuf;

/// Handle the `codemark repo` subcommand.
pub async fn handle_repo(
    cli: &Cli,
    mode: &OutputMode,
    args: &RepoArgs,
) -> Result<()> {
    match &args.command {
        RepoCommand::List => handle_repo_list(mode),
        RepoCommand::ShowRepo(args) => handle_repo_show(cli, mode, args),
        RepoCommand::SetServer(args) => handle_repo_set_server(cli, mode, args),
    }
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
                    println!("  {}/{}  {}", repo.repo_owner, repo.repo_name, repo.repo_root.display());
                    if let Some(ref url) = repo.origin_url {
                        println!("    URL: {}", url);
                    }
                    if let Some(ref server) = repo.server_url {
                        println!("    Server: {}", server);
                    }
                    println!("    DB Owner: {}{}\n", repo.db_owner_email,
                        repo.db_owner_name.as_ref().map(|n| format!(" ({})", n)).unwrap_or_default());
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
        registry::find_repo_by_owner_name(&conn, repo_ref)?
            .ok_or_else(|| Error::Input(format!("Repository '{}' not found in registry", repo_ref)))?
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
                Error::Input(format!("Repository '{}' not found in global registry.\n\
                    Run any codemark command to sync it.", repo_root_str))
            })?
        } else {
            return Err(Error::Input(
                "Current repository not yet tracked by codemark.\n\
                Run any codemark command (like `codemark list`) to register it.".into()
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
            println!("Database Owner: {}{}", repo.db_owner_email,
                repo.db_owner_name.as_ref().map(|n| format!(" ({})", n)).unwrap_or_default());
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
        let repo = registry::find_repo_by_owner_name(&conn, repo_ref)?
            .ok_or_else(|| Error::Input(format!("Repository '{}' not found in registry", repo_ref)))?;
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
            .map_err(|_| Error::Input(
                "Current repository not yet tracked by codemark.\n\
                Run any codemark command (like `codemark list`) to register it.".into()
            ))?.ok_or_else(|| {
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
