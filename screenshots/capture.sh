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
# Output (overwrites the committed gallery images in screenshots/images/):
#   codemark_tui_screenshot.png
#   codemark_tui_query_screenshot.png
#   codemark_tui_collections_screenshot.png
#   codemark_tui_theme_<slug>.png   (one per color scheme)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SHOTS_DIR="$REPO_ROOT/screenshots"
IMAGES_DIR="$SHOTS_DIR/images"
mkdir -p "$IMAGES_DIR"

command -v vhs >/dev/null || { echo "error: vhs not found (brew install vhs)"; exit 1; }
command -v jq  >/dev/null || { echo "error: jq not found"; exit 1; }

# The tapes render with FiraCode Nerd Font Mono so the TUI's Nerd Font icons
# appear. Warn (don't fail) if it's missing — vhs falls back to a default font,
# which produces iconless screenshots.
if command -v fc-list >/dev/null && ! fc-list | grep -qi "FiraCode Nerd Font Mono"; then
  echo "warning: 'FiraCode Nerd Font Mono' not found; icons will not render."
  echo "         Install it (macOS: brew install --cask font-fira-code-nerd-font)."
fi

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
# vhs does not interpolate env vars in `Screenshot` paths, so we template the
# tapes instead: each tape uses a literal `__OUT__` token for its output dir,
# which we substitute with the absolute images dir into a rendered copy. vhs runs
# from the seeded demo repo (so the TUI, resolved from PATH, auto-detects its
# database) while the rendered Screenshot path stays absolute — keeping the
# working directory and output location decoupled.
RENDER_DIR="$SANDBOX/tapes"
mkdir -p "$RENDER_DIR"

# The three-view gallery is captured on the default theme.
for tape in main query collections; do
  echo "    - $tape.tape"
  rendered="$RENDER_DIR/$tape.tape"
  sed "s#__OUT__#$IMAGES_DIR#g" "$SHOTS_DIR/$tape.tape" > "$rendered"
  ( cd "$DEMO_DIR" && PATH="$BIN_DIR:$PATH" vhs "$rendered" )
done

# Per-theme gallery: one main-view shot per base16/base24 scheme (the themes
# that re-theme the whole TUI). The scheme list comes from the binary itself, so
# the gallery grows automatically as schemes are added. CODEMARK_TUI_THEME
# selects the scheme at runtime; theme.tape uses Padding 0 so vhs's own theme is
# irrelevant.
echo "    - theme.tape (per scheme)"
slugify() { echo "$1" | tr '[:upper:]' '[:lower:]' | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g'; }
while IFS= read -r scheme; do
  [ -n "$scheme" ] || continue
  slug="$(slugify "$scheme")"
  shot="codemark_tui_theme_${slug}.png"
  echo "        - $scheme -> $shot"
  rendered="$RENDER_DIR/theme-$slug.tape"
  sed -e "s#__OUT__#$IMAGES_DIR#g" -e "s#__SHOT__#$shot#g" "$SHOTS_DIR/theme.tape" > "$rendered"
  ( cd "$DEMO_DIR" && PATH="$BIN_DIR:$PATH" CODEMARK_TUI_THEME="$scheme" vhs "$rendered" )
done < <("$BIN_DIR/codemark-tui" --list-schemes)

echo "==> Done. Updated:"
ls -1 "$IMAGES_DIR"/codemark_tui_*.png
