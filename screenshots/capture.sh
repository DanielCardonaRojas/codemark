#!/usr/bin/env bash
#
# Regenerate the README screenshot gallery from the live TUI.
#
# The TUI is run against *this repository's own* knowledge base
# (.codemark/codemark.db, which is tracked in git), so the screenshots show
# real bookmarks and collections and the previews resolve against real source
# files. No synthetic demo data is generated.
#
# Pipeline:
#   1. build the `codemark-tui` (TUI) binary
#   2. run each vhs tape from the repo root, capturing PNGs into screenshots/images/
#
# Requirements: cargo, git, and `vhs` (https://github.com/charmbracelet/vhs).
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

[ -f "$REPO_ROOT/.codemark/codemark.db" ] || {
  echo "error: $REPO_ROOT/.codemark/codemark.db not found — the gallery uses the"
  echo "       repo's own knowledge base as its fixture."
  exit 1
}

# The tapes render with SF Mono Nerd Font so the TUI's Nerd Font icons appear.
# Warn (don't fail) if it's missing — vhs falls back to a default font, which
# produces iconless screenshots.
if command -v fc-list >/dev/null && ! fc-list | grep -qi "SFMono Nerd Font"; then
  echo "warning: 'SFMono Nerd Font' not found; icons will not render."
  echo "         Install it (macOS: brew tap epk/epk && brew install font-sf-mono-nerd-font)."
fi

echo "==> Building binary"
cargo build --release -p codemark-tui --bin codemark-tui

BIN_DIR="$REPO_ROOT/target/release"

# Sandbox the *global* codemark state (registry/config/cache) so capturing never
# touches the developer's real global state and stays reproducible. The repo's
# own .codemark/codemark.db (picked up from the working directory) is the data
# fixture and is intentionally not sandboxed.
SANDBOX="$(mktemp -d)"
export CODEMARK_DATA_DIR="$SANDBOX/data"
export XDG_CONFIG_HOME="$SANDBOX/config"
export XDG_DATA_HOME="$SANDBOX/data"
export XDG_CACHE_HOME="$SANDBOX/cache"

# The TUI's background live-health pass can write resolutions to the database.
# Snapshot the tracked db and restore it afterward so a capture run leaves it
# byte-identical (no spurious git diff on the fixture).
DB="$REPO_ROOT/.codemark/codemark.db"
DB_BACKUP="$SANDBOX/codemark.db.bak"
cp "$DB" "$DB_BACKUP"
restore_db() {
  rm -f "$DB-wal" "$DB-shm"
  cp "$DB_BACKUP" "$DB"
  rm -rf "$SANDBOX"
}
trap restore_db EXIT

echo "==> Capturing screenshots"
# vhs does not interpolate env vars in `Screenshot` paths, so we template the
# tapes instead: each tape uses a literal `__OUT__` token for its output dir,
# which we substitute with the absolute images dir into a rendered copy. vhs runs
# from the repo root (so the TUI auto-detects .codemark/codemark.db) while the
# rendered Screenshot path stays absolute — decoupling cwd from output location.
RENDER_DIR="$SANDBOX/tapes"
mkdir -p "$RENDER_DIR"

# The three-view gallery is captured on the default theme.
for tape in main query collections; do
  echo "    - $tape.tape"
  rendered="$RENDER_DIR/$tape.tape"
  sed "s#__OUT__#$IMAGES_DIR#g" "$SHOTS_DIR/$tape.tape" > "$rendered"
  ( cd "$REPO_ROOT" && PATH="$BIN_DIR:$PATH" vhs "$rendered" )
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
  ( cd "$REPO_ROOT" && PATH="$BIN_DIR:$PATH" CODEMARK_TUI_THEME="$scheme" vhs "$rendered" )
done < <("$BIN_DIR/codemark-tui" --list-schemes)

echo "==> Done. Updated:"
ls -1 "$IMAGES_DIR"/codemark_tui_*.png
