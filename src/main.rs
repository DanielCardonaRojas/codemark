mod cli;
mod config;
mod embeddings;
mod engine;
mod error;
mod git;
mod parser;
mod query;
mod storage;

use clap::Parser;

use cli::Cli;
use error::exit_with_error;

fn main() {
    // Initialize sqlite-vec extension early, before any connections are opened.
    // This must happen before Database::open() is called anywhere.
    embeddings::VecStore::init_extension();

    // Ensure default templates exist in user's data directory.
    cli::templates::ensure_default_template_exists();

    let cli = Cli::parse();

    if let Err(err) = cli::handlers::dispatch(&cli) {
        exit_with_error(&err);
    }
}
