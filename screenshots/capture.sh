#!/usr/bin/env bash
#
# Regenerate the README screenshot gallery from the live TUI.
#
# Pipeline:
#   1. build the `codemark` (CLI) and `codemark-tui` (TUI) binaries
#   2. seed a self-contained demo repo with realistic data (screenshots/seed.sh)
#   3. run each vhs tape from the demo repo, capturing PNGs into the repo root
#
# Requirements: cargo, jq, git, and `vhs` (https://github.com/charmbracelet/vhs).
# Install vhs locally with: brew install vhs   (Linux: see the vhs README).
#
# Usage: screenshots/capture.sh
#
# Output (overwrites the committed gallery images at the repo root):
#   codemark_tui_screenshot.png
#   codemark_tui_query_screenshot.png
#   codemark_tui_collections_screenshot.png

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SHOTS_DIR="$REPO_ROOT/screenshots"

command -v vhs >/dev/null || { echo "error: vhs not found (brew install vhs)"; exit 1; }
command -v jq  >/dev/null || { echo "error: jq not found"; exit 1; }

echo "==> Building binaries"
cargo build --release -p codemark-cli --bin codemark
cargo build --release -p codemark-tui --bin codemark-tui

BIN_DIR="$REPO_ROOT/target/release"
CM="$BIN_DIR/codemark"

# Sandbox all global codemark state so capture never touches the developer's
# real registry/config. Cleaned up on exit.
SANDBOX="$(mktemp -d)"
export CODEMARK_DATA_DIR="$SANDBOX/data"
export XDG_CONFIG_HOME="$SANDBOX/config"
export XDG_DATA_HOME="$SANDBOX/data"
export XDG_CACHE_HOME="$SANDBOX/cache"
DEMO_DIR="$SANDBOX/demo"
trap 'rm -rf "$SANDBOX"' EXIT

echo "==> Seeding demo repo"
"$SHOTS_DIR/seed.sh" "$DEMO_DIR" "$CM"

echo "==> Capturing screenshots"
# Run vhs from the seeded demo repo so the TUI (`codemark-tui`, resolved from
# PATH) auto-detects its database. The tapes write their PNGs to absolute paths
# under $OUT_DIR (the repo root) via the OUT_DIR env var, so the working
# directory and the output location stay decoupled.
export OUT_DIR="$REPO_ROOT"
for tape in main query collections; do
  echo "    - $tape.tape"
  ( cd "$DEMO_DIR" && PATH="$BIN_DIR:$PATH" \
      vhs "$SHOTS_DIR/$tape.tape" )
done

echo "==> Done. Updated:"
ls -1 "$REPO_ROOT"/codemark_tui_*.png
