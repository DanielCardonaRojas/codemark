# Screenshot gallery automation

Regenerates the TUI screenshots used in the project README from the live binary,
so the gallery stays faithful to the real interface.

## How it works

1. **`seed.sh`** builds a self-contained demo git repo with realistic source
   files and seeds it with bookmarks + a collection via the `codemark` CLI, so
   the screens are populated and meaningful.
2. **`*.tape`** are [vhs](https://github.com/charmbracelet/vhs) scripts — one per
   screenshot — that launch `codemark-tui`, drive the keystrokes to reach a
   view, and capture a PNG.
3. **`capture.sh`** ties it together: build binaries → seed → run each tape,
   writing the PNGs to the repo root (overwriting the committed gallery images).

| Tape | Output | View |
|------|--------|------|
| `main.tape` | `codemark_tui_screenshot.png` | Bookmarks list + content preview |
| `query.tape` | `codemark_tui_query_screenshot.png` | Tree-sitter Query tab |
| `collections.tape` | `codemark_tui_collections_screenshot.png` | Collections view |
| `theme.tape` | `codemark_tui_theme_<slug>.png` | Main view, once per color scheme |

The three-view gallery is captured on the default theme. The per-theme gallery
captures the main view once for each base16/base24 **scheme** (the themes that
re-theme the whole TUI chrome — syntect `.tmTheme` themes only recolor the code
preview, so they're excluded). The scheme list is read from the binary itself
(`codemark-tui --list-schemes`), so the gallery grows automatically as schemes
are added. `theme.tape` selects the scheme at runtime via the
`CODEMARK_TUI_THEME` env var and uses `Padding 0` so the TUI fills the frame —
which is why no vhs-theme mapping is needed.

## Run locally

```bash
brew install vhs                              # macOS; see vhs README for Linux
brew install --cask font-fira-code-nerd-font  # so the TUI icons render
./screenshots/capture.sh
```

The tapes render with **FiraCode Nerd Font Mono** so the TUI's Nerd Font icons
(file/health glyphs) appear; `capture.sh` warns if the font is missing. The
font, canvas size (`Set Width`/`Height`), and font size (`Set FontSize`) are set
identically at the top of each tape — change them there to retune the look.

All global codemark state (registry/config) is sandboxed into a temp dir, so a
local run never touches your real `~/.local/share/codemark`.

## CI

`.github/workflows/screenshots.yml` regenerates the gallery and opens a PR with
the updated images. It runs on `workflow_dispatch` (manual) and when a release
is published.

## Tuning the keystrokes

The tape keystrokes (number of `Tab`/`]` presses to reach each view) depend on
the TUI's current layout. If a captured screen isn't the intended view, adjust
the keys in the relevant `.tape` and re-run `capture.sh`. Each tape documents
which view it targets and where the counts may need confirmation.
