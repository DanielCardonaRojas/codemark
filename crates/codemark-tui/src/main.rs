//! Codemark TUI - Terminal interface for Codemark.
//!
//! This is the binary entry point for the standalone `codemark-tui` binary. The
//! actual run loop lives in [`codemark_tui::run`] so it can also be linked and
//! driven in-process by `codemark tui` (see `codemark-cli`'s `bundled-tui`
//! feature).

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    match codemark_tui::run().await? {
        Some(exit_code) => std::process::exit(exit_code),
        None => Ok(()),
    }
}
