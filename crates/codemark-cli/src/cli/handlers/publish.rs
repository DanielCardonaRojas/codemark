use crate::cli::output::OutputMode;
use crate::cli::*;
use codemark_core::error::{Error, Result};
use codemark_core::sync::{SyncDirection, SyncOptions, sync};

// Re-export auth resolution helpers
use crate::cli::handlers::auth_resolve::{detect_current_repo, resolve_server_and_token};

pub async fn handle_publish(cli: &Cli, mode: &OutputMode, args: &PublishArgs) -> Result<()> {
    if args.p2p {
        #[cfg(feature = "p2p")]
        {
            return handle_publish_p2p(cli, mode, args).await;
        }
        #[cfg(not(feature = "p2p"))]
        {
            return Err(Error::Input(
                "this build was compiled without p2p support; rebuild with `--features p2p`"
                    .to_string(),
            ));
        }
    }

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

    // 5. Load config
    let config = super::load_config(cli);

    // 6. Use unified sync interface from core crate
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
        config: Some(config),
    };

    sync(sync_opts).await?;

    // Show success message
    crate::cli::output::write_success(mode, &format!("Published collection: {}", collection.name))?;

    Ok(())
}

/// Serverless peer-to-peer publish: build the same portable pack used by the
/// registry, then serve it directly to a peer over iroh. Blocks until Ctrl+C.
#[cfg(feature = "p2p")]
async fn handle_publish_p2p(cli: &Cli, mode: &OutputMode, args: &PublishArgs) -> Result<()> {
    let db = super::open_db_for_write(cli)?;

    let collection = if let Some(col) = db.get_collection_by_name(&args.collection)? {
        col
    } else if let Some(col) = db.get_collection_by_id_prefix(&args.collection)? {
        col
    } else {
        return Err(Error::Input(format!("collection '{}' not found", args.collection)));
    };

    let project_root = super::get_project_root(&db);
    let config = super::load_config(cli);

    // Build the transport-agnostic pack bytes, then hand them to the p2p layer.
    let bytes = codemark_core::sync::build_pack_bytes(
        &db,
        &collection.id,
        &project_root,
        &config,
        args.title.as_deref(),
        args.description.as_deref(),
    )
    .await?;

    let (ticket, mut provider) = codemark_p2p::push_bytes(bytes)
        .await
        .map_err(|e| Error::Operation(format!("p2p push failed: {e:#}")))?;

    let pull_command = format!("codemark tour pull --p2p {ticket}");
    match mode {
        OutputMode::Json => crate::cli::output::write_json_success(&serde_json::json!({
            "collection": collection.name,
            "ticket": ticket,
            "pull_command": pull_command,
        }))?,
        _ => println!(
            "Serving '{}' over p2p — keep this running until the pull completes (Ctrl+C to stop).\n\n\
             Run this on the receiving machine:\n\n    {pull_command}",
            collection.name
        ),
    }

    // Serve until the peer has pulled (then exit cleanly) or the user cancels.
    tokio::select! {
        _ = provider.recv_delivery() => {
            crate::cli::output::write_success(mode, "Downloaded by peer — done.")?;
        }
        res = tokio::signal::ctrl_c() => {
            res.map_err(|e| Error::Operation(format!("failed waiting for Ctrl+C: {e}")))?;
        }
    }

    provider
        .shutdown()
        .await
        .map_err(|e| Error::Operation(format!("failed to shut down p2p provider: {e:#}")))?;

    Ok(())
}
