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

## Run locally

```bash
brew install vhs        # macOS; see vhs README for Linux
./screenshots/capture.sh
```

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
