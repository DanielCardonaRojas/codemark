use chrono::Utc;
use clap::Parser;
use codetours_server::{
    cli::{Cli, Command},
    config::Config,
    handlers::health::BOOT_TIME,
    observability::init_tracing,
    router::{AppState, router},
    shutdown::shutdown_signal,
    storage::{StorageManager, StorageEngine, RegistryClient},
};
use std::sync::Arc;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    BOOT_TIME.set(Utc::now()).ok();
    let cli = Cli::parse();

    let config = Config::load(cli.config.as_deref())?;

    init_tracing(&config, cli.json_logs);

    let storage = StorageManager::new(config.data_dir.clone(), config.storage.clone())?;

    if let Some(Command::Migrate) = cli.command {
        tracing::info!("migrations complete");
        return Ok(());
    }

    // Initialize storage engine if registry mode is enabled (via CLI flag or config)
    let registry_mode = cli.registry_mode || config.storage.registry_mode;
    let registry_path = cli.registry_path.clone().or(config.storage.registry_path.clone());

    let storage_engine = if registry_mode {
        let registry_client = RegistryClient::new(registry_path)?;
        let engine = StorageEngine::new(registry_client);
        engine.refresh().await?;
        Some(Arc::new(engine))
    } else {
        None
    };

    let state = AppState {
        config: Arc::new(config.clone()),
        storage: Arc::new(storage),
        storage_engine,
    };

    let app = router(state);

    let addr = format!("{}:{}", config.host, config.port);
    let listener = TcpListener::bind(&addr).await?;

    tracing::info!("listening on {}", addr);

    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;

    tracing::info!("shutdown complete");

    Ok(())
}
