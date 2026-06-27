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
#   codemark_tui_demo.gif           (the animated tour — the README's lead visual)
#   codemark_tui_theme_<slug>.png   (one per color scheme)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SHOTS_DIR="$REPO_ROOT/screenshots"
IMAGES_DIR="$SHOTS_DIR/images"
mkdir -p "$IMAGES_DIR"

command -v vhs   >/dev/null || { echo "error: vhs not found (brew install vhs)"; exit 1; }
command -v cargo >/dev/null || { echo "error: cargo not found"; exit 1; }

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

echo "==> Building binaries"
# Build both the TUI (what we screenshot) and the CLI (used below to seed the
# sandboxed global registry so the Repos tab is populated).
cargo build --release -p codemark-tui --bin codemark-tui
cargo build --release -p codemark-cli --bin codemark

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

# Seed the sandboxed global registry. The Repos tab reads the global registry.db
# (under CODEMARK_DATA_DIR), which the sandbox above points at an empty temp dir.
# The registry is only ever populated as a side effect of running a codemark CLI
# command in a repo — the TUI just reads it. So run one CLI command in the repo
# root to register this repo, otherwise the Repos tab renders empty.
echo "==> Seeding sandboxed registry (Repos tab)"
( cd "$REPO_ROOT" && "$BIN_DIR/codemark" init >/dev/null )

echo "==> Capturing screenshots"
# vhs does not interpolate env vars in `Screenshot`/`Output` paths, so we
# template the tapes instead: each uses a literal `__OUT__` token for its output
# dir, substituted with the absolute images dir into a rendered copy. vhs runs
# from the repo root (so the TUI auto-detects .codemark/codemark.db) while the
# rendered output path stays absolute — decoupling cwd from output location.
RENDER_DIR="$SANDBOX/tapes"
mkdir -p "$RENDER_DIR"

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

# Animated demo GIF: browse through bookmarks with the arrow keys. vhs records
# at full TUI resolution; we then downscale + repalette with ffmpeg to keep the
# committed GIF README-friendly (~1280px wide). Skipped if ffmpeg is missing.
echo "    - demo.tape (gif)"
GIF_FINAL="$IMAGES_DIR/codemark_tui_demo.gif"
if command -v ffmpeg >/dev/null; then
  GIF_RAW="$SANDBOX/demo_raw.gif"
  rendered="$RENDER_DIR/demo.tape"
  sed "s#__OUT__#$SANDBOX#g" "$SHOTS_DIR/demo.tape" > "$rendered"
  # demo.tape writes codemark_tui_demo.gif into the __OUT__ dir (the sandbox).
  ( cd "$REPO_ROOT" && PATH="$BIN_DIR:$PATH" vhs "$rendered" )
  mv "$SANDBOX/codemark_tui_demo.gif" "$GIF_RAW"
  # Two-pass palette for clean colors at the smaller size.
  PAL="$SANDBOX/palette.png"
  ffmpeg -y -loglevel error -i "$GIF_RAW" \
    -vf "scale=1280:-1:flags=lanczos,palettegen=stats_mode=diff" "$PAL"
  ffmpeg -y -loglevel error -i "$GIF_RAW" -i "$PAL" \
    -lavfi "scale=1280:-1:flags=lanczos[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=3" \
    "$GIF_FINAL"
else
  echo "      warning: ffmpeg not found; recording GIF at full size (large file)."
  rendered="$RENDER_DIR/demo.tape"
  sed "s#__OUT__#$IMAGES_DIR#g" "$SHOTS_DIR/demo.tape" > "$rendered"
  ( cd "$REPO_ROOT" && PATH="$BIN_DIR:$PATH" vhs "$rendered" )
fi

# Settings overlay demo GIF: open Settings and cycle its tabs. Same two-pass
# ffmpeg downscale as the main demo so the committed GIF stays README-friendly.
echo "    - settings.tape (gif)"
SETTINGS_GIF_FINAL="$IMAGES_DIR/codemark_tui_settings.gif"
if command -v ffmpeg >/dev/null; then
  SETTINGS_GIF_RAW="$SANDBOX/settings_raw.gif"
  rendered="$RENDER_DIR/settings.tape"
  sed "s#__OUT__#$SANDBOX#g" "$SHOTS_DIR/settings.tape" > "$rendered"
  # settings.tape writes codemark_tui_settings.gif into the __OUT__ dir (sandbox).
  ( cd "$REPO_ROOT" && PATH="$BIN_DIR:$PATH" vhs "$rendered" )
  mv "$SANDBOX/codemark_tui_settings.gif" "$SETTINGS_GIF_RAW"
  PAL="$SANDBOX/settings_palette.png"
  ffmpeg -y -loglevel error -i "$SETTINGS_GIF_RAW" \
    -vf "scale=1280:-1:flags=lanczos,palettegen=stats_mode=diff" "$PAL"
  ffmpeg -y -loglevel error -i "$SETTINGS_GIF_RAW" -i "$PAL" \
    -lavfi "scale=1280:-1:flags=lanczos[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=3" \
    "$SETTINGS_GIF_FINAL"
else
  echo "      warning: ffmpeg not found; recording GIF at full size (large file)."
  rendered="$RENDER_DIR/settings.tape"
  sed "s#__OUT__#$IMAGES_DIR#g" "$SHOTS_DIR/settings.tape" > "$rendered"
  ( cd "$REPO_ROOT" && PATH="$BIN_DIR:$PATH" vhs "$rendered" )
fi

# Settings thumbnail: a single still (Theme tab) used as the gallery poster.
echo "    - settings-thumb.tape (png)"
rendered="$RENDER_DIR/settings-thumb.tape"
sed "s#__OUT__#$IMAGES_DIR#g" "$SHOTS_DIR/settings-thumb.tape" > "$rendered"
( cd "$REPO_ROOT" && PATH="$BIN_DIR:$PATH" vhs "$rendered" )

# Search demo GIF: full-text vs semantic search. Same two-pass ffmpeg downscale
# as the other demo GIFs so the committed GIF stays README-friendly.
echo "    - search.tape (gif)"
SEARCH_GIF_FINAL="$IMAGES_DIR/codemark_tui_search.gif"
if command -v ffmpeg >/dev/null; then
  SEARCH_GIF_RAW="$SANDBOX/search_raw.gif"
  rendered="$RENDER_DIR/search.tape"
  sed "s#__OUT__#$SANDBOX#g" "$SHOTS_DIR/search.tape" > "$rendered"
  # search.tape writes codemark_tui_search.gif into the __OUT__ dir (sandbox).
  ( cd "$REPO_ROOT" && PATH="$BIN_DIR:$PATH" vhs "$rendered" )
  mv "$SANDBOX/codemark_tui_search.gif" "$SEARCH_GIF_RAW"
  PAL="$SANDBOX/search_palette.png"
  ffmpeg -y -loglevel error -i "$SEARCH_GIF_RAW" \
    -vf "scale=1280:-1:flags=lanczos,palettegen=stats_mode=diff" "$PAL"
  ffmpeg -y -loglevel error -i "$SEARCH_GIF_RAW" -i "$PAL" \
    -lavfi "scale=1280:-1:flags=lanczos[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=3" \
    "$SEARCH_GIF_FINAL"
else
  echo "      warning: ffmpeg not found; recording GIF at full size (large file)."
  rendered="$RENDER_DIR/search.tape"
  sed "s#__OUT__#$IMAGES_DIR#g" "$SHOTS_DIR/search.tape" > "$rendered"
  ( cd "$REPO_ROOT" && PATH="$BIN_DIR:$PATH" vhs "$rendered" )
fi

# Search thumbnail: a single still (Semantic results) used as the gallery poster.
echo "    - search-thumb.tape (png)"
rendered="$RENDER_DIR/search-thumb.tape"
sed "s#__OUT__#$IMAGES_DIR#g" "$SHOTS_DIR/search-thumb.tape" > "$rendered"
( cd "$REPO_ROOT" && PATH="$BIN_DIR:$PATH" vhs "$rendered" )

echo "==> Done. Updated:"
ls -1 "$IMAGES_DIR"/codemark_tui_*.png "$IMAGES_DIR"/codemark_tui_*.gif 2>/dev/null
