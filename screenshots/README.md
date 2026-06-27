# Screenshot gallery automation

Regenerates the TUI screenshots used in the project README from the live binary,
so the gallery stays faithful to the real interface.

## How it works

The gallery uses **this repository's own knowledge base** as its fixture: the
TUI is run against the tracked `.codemark/codemark.db` (committed to git), so the
screenshots show real bookmarks and collections and the previews resolve against
real source files. No synthetic demo data is generated — it's dogfooding.

1. **`*.tape`** are [vhs](https://github.com/charmbracelet/vhs) scripts — one per
   screenshot — that launch `codemark-tui`, drive the keystrokes to reach a
   view, and capture a PNG.
2. **`capture.sh`** ties it together: build the TUI → run each tape from the
   repo root → write PNGs to `screenshots/images/` (overwriting the committed
   gallery images). It sandboxes the *global* codemark state and snapshots the
   tracked db so a run leaves the fixture byte-identical.

The generated images live in `screenshots/images/`; the tooling (tapes and
`capture.sh`) lives in `screenshots/` alongside it.

Because the fixture is the live knowledge base, the gallery is **live**: it
reflects whatever bookmarks/collections currently exist. Regenerate it (locally
or via the workflow) whenever the knowledge base changes enough to matter.

| Tape | Output (in `images/`) | View |
|------|------------------------|------|
| `demo.tape` | `codemark_tui_demo.gif` | Animated tour: browse bookmarks, Content/Info/Query tabs, open a collection, step through it |
| `settings.tape` | `codemark_tui_settings.gif` | Animated tour of the Settings overlay: open with `,`, cycle the Configuration/Theme/About tabs with `]`/`[` |
| `settings-thumb.tape` | `codemark_tui_settings_thumb.png` | Static thumbnail (Theme tab) — the poster for the Settings demo in the [demo gallery](../dev-docs/guides/demo-gallery.md) |
| `theme.tape` | `codemark_tui_theme_<slug>.png` | Main view, once per color scheme |

The demo GIF is the README's lead visual. It's recorded at full TUI resolution
by vhs, then downscaled and repaletted with ffmpeg (~1280px wide) to keep the
committed file README-friendly; `capture.sh` skips the downscale if ffmpeg is
unavailable.

The per-theme gallery captures the main view once for each base16/base24
**scheme** (the themes that
re-theme the whole TUI chrome — syntect `.tmTheme` themes only recolor the code
preview, so they're excluded). The scheme list is read from the binary itself
(`codemark-tui --list-schemes`), so the gallery grows automatically as schemes
are added. `theme.tape` selects the scheme at runtime via the
`CODEMARK_TUI_THEME` env var and uses `Padding 0` so the TUI fills the frame —
which is why no vhs-theme mapping is needed.

## Run locally

```bash
brew install vhs                            # macOS; see vhs README for Linux
brew tap epk/epk                            # SF Mono Nerd Font tap
brew install font-sf-mono-nerd-font         # so the TUI icons render
./screenshots/capture.sh
```

The tapes render with **SF Mono Nerd Font** (family `SFMono Nerd Font`) so the
TUI's Nerd Font icons (file/health glyphs) appear; `capture.sh` warns if the
font is missing. The font, canvas size (`Set Width`/`Height`), and font size
(`Set FontSize`) are set identically at the top of each tape — change them there
to retune the look. (CI installs the font from the same `epk/SF-Mono-Nerd-Font`
source via its versioned archive.)

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
