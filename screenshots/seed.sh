#!/usr/bin/env bash
#
# Seed a self-contained demo repository with realistic codemark data so the TUI
# screenshots show populated, meaningful screens (bookmarks, a query preview,
# and a collection) rather than an empty database.
#
# Usage:
#   screenshots/seed.sh <demo_dir> <codemark_cli_bin>
#
# - <demo_dir>         where the demo git repo + .codemark/ will be created
#                      (wiped and recreated each run for determinism)
# - <codemark_cli_bin> path to the `codemark` CLI binary
#
# The TUI auto-detects its database from the current directory's git root
# (<repo_root>/.codemark/codemark.db), so the vhs tapes launch the TUI from
# <demo_dir>.

set -euo pipefail

DEMO_DIR="${1:?usage: seed.sh <demo_dir> <codemark_cli_bin>}"
CM="${2:?usage: seed.sh <demo_dir> <codemark_cli_bin>}"

# Force deterministic output and a stable identity so seeded data (and thus the
# screenshots) don't vary by machine/user.
export CODEMARK_FORMAT="json"
export GIT_AUTHOR_NAME="Codemark Demo"
export GIT_AUTHOR_EMAIL="demo@codemark.dev"
export GIT_COMMITTER_NAME="Codemark Demo"
export GIT_COMMITTER_EMAIL="demo@codemark.dev"

# Start from a clean slate.
rm -rf "$DEMO_DIR"
mkdir -p "$DEMO_DIR/src"

# --- Demo source files (the code that bookmarks point at) -------------------

cat > "$DEMO_DIR/src/main.rs" <<'EOF'
//! Demo application entry point.

mod config;
mod server;

use anyhow::Result;

fn main() -> Result<()> {
    let config = config::Config::load()?;
    server::Server::new(config).run()
}
EOF

cat > "$DEMO_DIR/src/config.rs" <<'EOF'
//! Application configuration loading and validation.

use anyhow::Result;

/// Runtime configuration for the demo server.
pub struct Config {
    pub host: String,
    pub port: u16,
    pub workers: usize,
}

impl Config {
    /// Load configuration from the environment, falling back to defaults.
    pub fn load() -> Result<Self> {
        Ok(Self {
            host: std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into()),
            port: std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8080),
            workers: num_cpus(),
        })
    }
}

fn num_cpus() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}
EOF

cat > "$DEMO_DIR/src/server.rs" <<'EOF'
//! HTTP server wiring and the request handling loop.

use crate::config::Config;
use anyhow::Result;

/// The demo HTTP server.
pub struct Server {
    config: Config,
}

impl Server {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Bind the listener and serve requests until shutdown.
    pub fn run(self) -> Result<()> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        println!("listening on {addr} with {} workers", self.config.workers);
        Ok(())
    }
}
EOF

# --- Git repo (codemark init detects the git root) --------------------------

git -C "$DEMO_DIR" init -q
git -C "$DEMO_DIR" add -A
git -C "$DEMO_DIR" commit -q -m "Initial demo project"

# --- Initialize codemark and seed bookmarks ---------------------------------

( cd "$DEMO_DIR" && "$CM" init >/dev/null )

# Helper: add a bookmark and echo its id (parsed from JSON output).
# Ranges target whole definitions (fn/struct/impl) so the derived tree-sitter
# query resolves to a single clean node.
add_bookmark() {
  ( cd "$DEMO_DIR" && "$CM" add "$@" ) | jq -r '.data.id'
}

BM_MAIN=$(add_bookmark --file src/main.rs --range 8-11 \
  --tag entrypoint --note "Application entry point and module wiring")

BM_CONFIG=$(add_bookmark --file src/config.rs --range 13-22 \
  --tag config --note "Config loading from env with defaults")

BM_SERVER=$(add_bookmark --file src/server.rs --range 16-21 \
  --tag server --note "Request serving loop")

# --- Build a collection so the Collections view is populated -----------------
# (collection-level notes are not yet persisted, so they're omitted here.)

( cd "$DEMO_DIR" && "$CM" collection add "Startup Path" \
    "$BM_MAIN" "$BM_CONFIG" "$BM_SERVER" \
    >/dev/null )

echo "Seeded demo repo at: $DEMO_DIR"
echo "  bookmarks: $BM_MAIN $BM_CONFIG $BM_SERVER"
