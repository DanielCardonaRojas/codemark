use clap::Parser;
use codemark::cli::{Cli, handlers, templates};
use codemark::error::exit_with_error;

#[tokio::main]
async fn main() {
    // Ensure default templates exist in user's data directory.
    templates::ensure_default_template_exists();

    let cli = Cli::parse();

    if let Err(err) = handlers::dispatch(&cli).await {
        exit_with_error(&err);
    }
}
