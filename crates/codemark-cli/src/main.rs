use clap::Parser;
use codemark_cli::cli::{Cli, handlers, templates};
use codemark_core::error::exit_with_error;

#[tokio::main]
async fn main() {
    // Ensure default templates exist in user's data directory.
    templates::ensure_default_template_exists();

    let cli = Cli::parse();

    if let Err(err) = handlers::dispatch(&cli).await {
        exit_with_error(&err);
    }
}
