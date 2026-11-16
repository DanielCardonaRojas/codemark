# Demo Gallery

Animated walkthroughs of the Codemark TUI. They live here instead of inline in the
README because each GIF is full-resolution and runs to several megabytes.

Every clip is recorded from the real binary against a curated fixture knowledge
base: bookmarks and collections seeded into a pinned checkout of this repo, so
the previews show real source. See
[screenshots/README.md](../../screenshots/README.md) if you want to regenerate
them.

---

## Guided tour

Browse collections with the arrow keys, open one with `Enter`, and step through
its bookmarks with `→`. On a step, `]` flips between the Steps (code) / Info /
Query preview tabs. The fixture includes one **Drifted** step, whose code
changed after it was bookmarked, and one **Broken** bookmark, whose function
was removed. That way you see how health shows up, not just the happy path.

![Animated tour of the Codemark TUI](../../screenshots/images/codemark_tui_demo.gif)

---

## Comments — agents and people

Comments point at specific lines inside a bookmark, and each has its own
structural anchor, so it follows that code when the file changes. An agent can
leave one from the CLI:

```bash
codemark comment add <bookmark> --range 24-25 --created-by claude \
  --body "This compiles the query on every call. Cache compiled queries per language?"
```

Open the bookmark in the TUI and the agent's comment sits right under the lines
it's about. With the code preview focused:

- `n` / `N` jump to the next / previous comment.
- `r` replies to the comment under the cursor.
- `c` comments on the line under the cursor (press `v` first to select a
  range).
- `E` edits your latest comment in the thread.
- `d` deletes the thread, after asking first.
- `Ctrl+S` saves a draft and `Esc` cancels it.

There are two ways to show comments. **Inline** (the default) opens each thread
in the code, right below the lines it points at. **Side by side** puts the
threads in a column next to the code, each lined up with its line, so the code
stays together. With the preview focused, `+` cycles the layouts: full width,
then side by side, then preview only, then back to the split. `-` goes the other
way.

![Codemark TUI anchored comments](../../screenshots/images/codemark_tui_comments.gif)

---

## Settings

Press `,` to open the Settings overlay, then move across its tabs with `]` / `[`
(Configuration → Theme → About). The Theme tab previews each color scheme as you
scroll through it with `j` / `k`.

![Codemark TUI settings overlay](../../screenshots/images/codemark_tui_settings.gif)

---

## Search — full-text & semantic

Press `s` to focus the search bar, type a query, and hit `Enter` to run it (focus
then moves to the results). With the bar focused, `Ctrl+S` switches between two
modes: **FTS** (SQLite full-text, matches exact terms) and
**Semantic** (local vector embeddings, matches by meaning, no API key). Semantic
mode handles intent-style questions like *"where do we resolve a bookmark after the
code moves"*; it's slower, so you'll see a brief loading spinner. `Esc` in the
search bar clears the query.

![Codemark TUI full-text and semantic search](../../screenshots/images/codemark_tui_search.gif)

---

## Filtering pane contents

This is separate from search. Press `/` to filter the **focused pane's** visible
list; it narrows as you type, no `Enter` needed. Each pane keeps its own filter —
the Bookmarks pane and the Tags pane don't share one — and `Esc` clears the
active filter.

![Codemark TUI pane filtering](../../screenshots/images/codemark_tui_filter.gif)

---

## Resizing & cycling layouts

The dashboard isn't a fixed three-pane grid. Focus a pane with its number key
(`1`–`5`), then grow it with `+` and shrink it with `-`. The preview and details
panes cycle through their own sizes on each press, so you can go from the default
split to a full-width preview and back without touching your mouse.

- `4` focuses the preview pane; `+` cycles it through full-width and
  preview-only, then back to the regular split.
- `5` focuses the details pane; `+` grows it to fill the right side, then the
  whole screen.
- `3` focuses the bookmarks pane; `+` widens the left side until it takes over the
  window.

![Codemark TUI layout resizing](../../screenshots/images/codemark_tui_layout.gif)
