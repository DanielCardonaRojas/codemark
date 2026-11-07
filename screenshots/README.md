# Screenshot gallery automation

Regenerates the TUI screenshots used in the project README from the live binary,
so the gallery stays faithful to the real interface.

## How it works

The gallery records against a **curated fixture knowledge base**, never the
repo's live working database (`.codemark/codemark.db`). We dogfood codemark for
issue context, so the live DB keeps gaining issue and WIP collections. Recording
against it would leak those into the README visuals and shift the tapes'
position-based keystrokes (e.g. which collection `Enter` opens).

The fixture is built fresh on every capture:

- **`seed-fixture.sh`** clones this repo at a pinned commit (`FIXTURE_REV`)
  into `capture.sh`'s sandbox. The clone shares the repo's object store, so
  this is instant and needs no network. It then seeds a small, known knowledge
  base into the clone with the CLI: about 18 bookmarks across the crates (tags, notes, markdown
  comments) and four collections (descriptions, tags, a link). It then commits
  two edits so the gallery shows a *Drifted* and a *Broken* bookmark, not only
  healthy ones.
- The tapes run from the clone, so previews resolve against real source files
  at the pinned commit.

Because the fixture is seeded by script rather than committed as a binary, it
always matches the current schema; there's nothing to migrate.

1. **`*.tape`** are [vhs](https://github.com/charmbracelet/vhs) scripts — one per
   screenshot — that launch `codemark-tui`, drive the keystrokes to reach a
   view, and capture a PNG.
2. **`capture.sh`** ties it together: build the TUI and CLI → clone and seed
   the fixture → run each tape from the fixture clone → write PNGs/GIFs to
   `screenshots/images/` (overwriting the committed gallery images). It
   sandboxes the *global* codemark state too, and deletes the sandbox when it
   finishes.

The generated images live in `screenshots/images/`; the tooling (tapes,
`capture.sh`, `seed-fixture.sh`) lives in `screenshots/` alongside it.

### Changing the fixture

- **Content** (bookmarks, collections, tags): edit `seed-fixture.sh`.
  Bookmarks are located by *function name*, and codemark generates their
  queries just as it does for a user. The TUI lists collections newest first, and
  `demo.tape` opens the top one and steps through it, so keep
  `bookmark-resolution` created last, with at least four steps.
- **Code shown in previews**: bump `FIXTURE_REV` in `seed-fixture.sh` (or
  override it with `FIXTURE_REV=<sha> ./screenshots/capture.sh`). If a bookmarked
  function was renamed or removed since the old pin, `seed-fixture.sh` fails
  and names the missing function.
- **Inspect it**: `CODEMARK=target/release/codemark screenshots/seed-fixture.sh
  /tmp/fixture` creates a fixture you can browse with `codemark-tui` from
  `/tmp/fixture`. (It writes to your real global registry, so sandbox
  `CODEMARK_DATA_DIR` as `capture.sh` does if that matters to you.)
- **CI**: the `Check demo fixture seeds` step in `.github/workflows/test.yml`
  runs the seed on every PR, so a renamed function or a changed CLI flag fails
  there and not at the next capture.

| Tape | Output (in `images/`) | View |
|------|------------------------|------|
| `demo.tape` | `codemark_tui_demo.gif` | Animated tour: browse collections, open one and step through it (including a Drifted step), Steps/Info/Query tabs, then the Bookmarks list (with a Broken bookmark) |
| `settings.tape` | `codemark_tui_settings.gif` | Animated tour of the Settings overlay: open with `,`, cycle the Configuration/Theme/About tabs with `]`/`[` |
| `settings-thumb.tape` | `codemark_tui_settings_thumb.png` | Static thumbnail (Theme tab) — the poster for the Settings demo in the [demo gallery](../dev-docs/guides/demo-gallery.md) |
| `search.tape` | `codemark_tui_search.gif` | Animated tour of search: focus with `s`, type a query, toggle FTS/Semantic with `Ctrl+S` |
| `search-thumb.tape` | `codemark_tui_search_thumb.png` | Static thumbnail (Semantic results) — the poster for the search demo in the [demo gallery](../dev-docs/guides/demo-gallery.md) |
| `filter.tape` | `codemark_tui_filter.gif` | Animated tour of pane filtering: `/` narrows the focused pane's list live (Bookmarks, then Tags) |
| `filter-thumb.tape` | `codemark_tui_filter_thumb.png` | Static thumbnail (filtered Bookmarks pane) — the poster for the filter demo in the [demo gallery](../dev-docs/guides/demo-gallery.md) |
| `layout.tape` | `codemark_tui_layout.gif` | Animated tour of pane resizing: focus a pane, cycle its size with `+`/`-` (preview, details, then the left pane) |
| `layout-thumb.tape` | `codemark_tui_layout_thumb.png` | Static thumbnail (full-width preview) — the poster for the layout demo in the [demo gallery](../dev-docs/guides/demo-gallery.md) |
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
local run never touches your real `~/.local/share/codemark`, and the repo's own
`.codemark/codemark.db` is never opened.

## CI

`.github/workflows/screenshots.yml` regenerates the gallery and opens a PR with
the updated images. It runs on `workflow_dispatch` (manual) and when a release
is published.

## Tuning the keystrokes

The tape keystrokes (number of `Tab`/`]` presses to reach each view) depend on
the TUI's current layout. If a captured screen isn't the intended view, adjust
the keys in the relevant `.tape` and re-run `capture.sh`. Each tape documents
which view it targets and where the counts may need confirmation.
