use tokio::signal;

#[cfg(unix)]
use tokio::signal::unix::{SignalKind, signal as unix_signal};

pub async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        let mut sig =
            unix_signal(SignalKind::terminate()).expect("failed to install signal handler");
        if sig.recv().await.is_none() {
            tracing::warn!("SIGTERM signal stream closed");
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("shutdown requested via Ctrl+C");
        },
        _ = terminate => {
            tracing::info!("shutdown requested via SIGTERM");
        },
    }

    tracing::info!("shutdown signal received, starting graceful drain");
}
