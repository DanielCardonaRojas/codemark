use chrono::Utc;
use clap::Parser;
use codetours_server::{
    cli::{Cli, Command},
    config::Config,
    handlers::health::BOOT_TIME,
    observability::init_tracing,
    router::{AppState, router},
    shutdown::shutdown_signal,
    storage::StorageManager,
};
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

    let state = AppState::new(config.clone(), storage)?;

    let app = router(state);

    let addr = format!("{}:{}", config.host, config.port);
    let listener = TcpListener::bind(&addr).await?;

    tracing::info!("listening on {}", addr);

    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;

    tracing::info!("shutdown complete");

    Ok(())
}
