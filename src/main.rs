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
    let cli = Cli::parse();

    if let Err(err) = cli::handlers::dispatch(&cli) {
        exit_with_error(&err);
    }
}
