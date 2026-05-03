use clap::Parser;
use codetours_server::{
    cli::Cli,
    config::Config,
    handlers::health::BOOT_TIME,
    observability::init_tracing,
    router::{router, AppState},
    shutdown::shutdown_signal,
};
use std::sync::Arc;
use tokio::net::TcpListener;
use chrono::Utc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    BOOT_TIME.set(Utc::now()).ok();
    let cli = Cli::parse();
    
    let config = Config::load(cli.config.as_deref())?;
    
    init_tracing(&config, cli.json_logs);
    
    let state = AppState {
        config: Arc::new(config.clone()),
    };
    
    let app = router(state);
    
    let addr = format!("{}:{}", config.host, config.port);
    let listener = TcpListener::bind(&addr).await?;
    
    tracing::info!("listening on {}", addr);
    
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
        
    tracing::info!("shutdown complete");
    
    Ok(())
}
