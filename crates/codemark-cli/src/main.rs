use clap::Parser;
use codemark_cli::cli::{Cli, external, handlers, templates};
use codemark_core::error::exit_with_error;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // External/plugin subcommands (e.g. `codemark tui` -> `codemark-tui`) are
    // dispatched before any expensive initialization so the backing binary
    // launches immediately. On Unix this `exec()`s and never returns; otherwise
    // it returns only when the executable could not be launched.
    if let Some(err) = external::try_dispatch(&cli.command) {
        eprintln!("{}", err.message);
        std::process::exit(err.code);
    }

    // Ensure default templates exist in user's data directory.
    templates::ensure_default_template_exists();

    if let Err(err) = handlers::dispatch(&cli).await {
        exit_with_error(&err);
    }
}
