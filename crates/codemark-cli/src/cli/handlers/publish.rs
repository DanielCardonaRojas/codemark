use crate::cli::output::OutputMode;
use crate::cli::*;
use codemark_core::error::{Error, Result};

// Re-export auth resolution helpers
use crate::cli::handlers::auth_resolve::{detect_current_repo, resolve_server_and_token};
use crate::cli::handlers::sync::{SyncDirection, SyncOptions, sync};

pub async fn handle_publish(cli: &Cli, mode: &OutputMode, args: &PublishArgs) -> Result<()> {
    let db = super::open_db_for_write(cli)?;

    // 1. Find collection
    let collection = if let Some(col) = db.get_collection_by_name(&args.collection)? {
        col
    } else if let Some(col) = db.get_collection_by_id_prefix(&args.collection)? {
        col
    } else {
        return Err(Error::Input(format!("collection '{}' not found", args.collection)));
    };

    // 2. Detect current repo URL for server discovery
    let repo_url = detect_current_repo()?;

    // 3. Resolve server and token from registry (with CLI token as fallback)
    let (server_url, registry_token) =
        resolve_server_and_token(cli, args.server.as_deref(), repo_url.as_deref())?;

    // Use CLI token if provided, otherwise use registry token
    let token = args.token.as_ref().or(registry_token.as_ref()).cloned();

    // 4. Get project root
    let project_root = super::get_project_root(&db);

    // 5. Use unified sync interface
    let sync_opts = SyncOptions {
        collection_id: collection.id.clone(),
        server_url,
        direction: SyncDirection::Push,
        token,
        visibility: Some(args.visibility.clone()),
        title: args.title.clone(),
        description: args.description.clone(),
        dry_run: args.dry_run,
        save_name: None,
        db: Some(db),
        project_root: Some(project_root.to_string_lossy().to_string()),
        cli: cli as *const Cli,
    };

    sync(cli, mode, sync_opts).await
}
