# Multi-repo queries in the TUI

## Problem

The TUI browses one repository at a time. Selecting a repo in the Repos panel
calls `switch_database()`, which *replaces* the single `self.db: Database`
wholesale. The entire browser — panel refresh, right-pane rendering, heal,
live-health, search — reads from that one `self.db` and derives the repo root
from `self.db.path().parent().parent()`.

The CLI already supports cross-repo queries: `codemark list --repo a/b --repo
c/d` opens several databases and merges the results. Connecting related code
across repositories is valuable, but the TUI can't do it.

Enabling multi-repo in the TUI raises two UX problems the design must solve:

1. Once bookmarks from several repos share one pane, it's hard to tell which
   repo a row belongs to.
2. Tours are backed by per-repo local collections plus a remote Codetours
   server scoped to a repo set; merging tours across repos is more involved than
   it's worth for a first version.

## Goal

Let the user query several repositories at once in the TUI by checking multiple
repos in the Repos panel. Merged bookmark/collection rows must show which repo
each belongs to. When exactly one repo is checked, behavior is identical to
today.

## Decisions (settled during design)

- **Selection:** the Repos panel becomes a multi-select (like the existing
  Owners panel). Space toggles a repo in/out of the query scope. There is **no
  single "active" DB** — the query spans every checked repo.
- **Row labeling:** when more than one repo is in scope, the bookmark/collection
  row replaces the **author** field with the **bare repo name** (e.g.
  `codemark`). Single-repo mode keeps the author exactly as today.
- **Tours:** the Tours tab is **hidden whenever more than one repo is checked**.
  With exactly one repo checked it works fully (local + remote), unchanged.
- **DB architecture:** a stateful, TUI-side workspace owns one open `Database`
  per checked repo (approach A). Reuses core primitives; does **not** reuse
  `Workspace::open_all` (that resolves CLI flags, not panel selections).
- **Core consolidation:** no broad "move CLI logic to core" pass. Only the one
  genuinely shared, non-trivial helper — `find_bookmark_across` — moves into
  core. The fan-out/tagging loops stay in each consumer because their outputs
  diverge (CLI stdout vs. TUI `PanelItem`s).

## Non-goals

- No cross-repo *tours* (Tours is hidden in multi-repo mode).
- No new "create bookmark" flow. TUI writes remain item-scoped (delete, heal,
  publish/sync of a selected item).
- No change to CLI behavior beyond calling the relocated `find_bookmark_across`.
- No merged/shared "multi-repo query engine" in core.

## Architecture

### What core already provides (and what it does not)

`crates/codemark-core/src/storage/workspace.rs` provides multi-repo
**opening/discovery** only:

- `Workspace::open_all(opts) -> Vec<(String label, Database)>` — a list of
  independent single-repo connections, built once from CLI options.
- `source_label_from_path`, `project_root`, `Database::open` — primitives.

There is **no** cross-repo query/merge API. Every query method
(`list_bookmarks`, `search`, `get_bookmark`, …) is a method on a single
`Database`. The CLI does the fan-out itself: iterate the Vec, run the per-DB
query, tag each result with the label, and format for stdout.

The TUI reuses the primitives (`Database::open`, `source_label_from_path`,
`project_root`) but **not** `open_all`, because the TUI's scope is the set of
*checked repos in the Repos panel* (registry-backed roots), not CLI
`--db`/`--repo` flags, and it mutates at runtime.

### Step 0 — relocate `find_bookmark_across` into core

Move `find_bookmark_across` (currently
`crates/codemark-cli/src/cli/handlers.rs`) into `codemark-core` (e.g.
`Workspace::find_bookmark_across(dbs, id) -> Result<(Bookmark, &Database)>`,
matching full-id then ≥4-char prefix). The CLI calls the relocated version so
its tests stay green; the TUI reuses it for open-by-id and resolving a selected
row's DB. Everything else in the CLI stays put.

The two DB filters (`filter_dbs_by_user_email`, `filter_dbs_by_repo_owner`) may
move too if convenient, but that is optional and not required by this feature.

### Component 1 — `RepoWorkspace` (new, TUI-side)

A small stateful owning container that replaces `self.db`:

- Owns the open `Database` for each **checked** repo, keyed by repo root, plus a
  `focus` repo root. Container type (Vec vs. IndexMap) is an implementation
  detail — at ≤50 repos a linear scan is fine; what matters is that it is
  *stateful* (opens/drops on toggle) and supports *lookup by repo root*.
- `set_scope(checked_roots)`: opens any newly-checked DB
  (`<root>/.codemark/codemark.db`) and drops any no-longer-checked DB. Does
  **not** reopen unchanged connections — toggling one repo must not reopen the
  rest (avoids nav-path lag).
- `dbs()` (ordered iteration), `get(root) -> Option<&Database>`,
  `focus_db() -> &Database`, `is_multi() -> bool`.
- **Invariant:** at least one repo is always checked. Attempting to uncheck the
  last checked repo is a no-op.
- `focus` = the row under the cursor if it is checked, else the first checked
  repo. Used **only** for: config/server/token resolution (remote tours, sync),
  single-repo Tours, and the default preview when nothing is selected. In
  single-repo mode `focus_db()` is the one DB, so those paths are unchanged.

Startup seeds the scope with the auto-detected current repo checked — identical
to today's single-repo default.

### Component 2 — merged row building

`build_content_items(db, …)` stays per-DB and unchanged. A new merge layer:

- iterates `workspace.dbs()`, calls the existing per-DB builder for each, tags
  every produced `PanelItem` with its repo (name + root), and concatenates.
- sorts the merged list by `created_at` descending, so repos interleave by
  recency rather than clumping by repo.
- when `workspace.is_multi()`, replaces the row's author/metadata field with the
  bare repo name. In single-repo mode the author is left exactly as today.

Collections get the same treatment (repo tag + author→repo swap in multi mode).

**Row repo identity.** Each row must carry its repo so selection, live-health
caching, and per-item DB lookup can't collide on ids shared across repos. The
encoding (a dedicated `repo` field on `PanelItem` vs. encoding the root into
`user_data`) is deferred to implementation. Whatever is chosen must key:
selection restoration, the live-health cache, and `db_for_item` by
`(repo_root, id)` rather than bare `id`.

### Component 3 — per-item DB resolution

A helper `db_for_item(item) -> &Database` looks up the item's repo root in the
workspace. It is threaded into every path that currently assumes `self.db`:

- right-pane preview (`load_bookmark_live` / `load_tour_live`), using the item's
  repo root as the markdown `repo_path`;
- heal target;
- delete (including the collection-cascade bookmark count);
- live-health apply;
- open-by-id / search-result open (via the relocated `find_bookmark_across`).

### Component 4 — live-health caches

The bookmark and collection live-health `HashMap`s become keyed by
`(repo_root, id)` instead of bare `id`, so colliding ids across repos don't
cross-contaminate. The existing generation guard is unchanged.
`spawn_live_health_task` fans out across all checked repos.

### Component 5 — search

Search fans out across all checked DBs and merges results, each tagged by repo.
The existing `request_id` / generation guards are preserved. Results carry the
repo so opening one resolves the correct DB.

### Component 6 — Tours visibility

`update_tours_tab_visibility` additionally hides the Tours tab whenever
`workspace.is_multi()`. With exactly one repo checked, Tours works fully (local +
remote) as today. The remote-tour fetch already scopes by the active repos in
the Repos panel, so no change is needed beyond the visibility gate.

### Component 7 — selection flow

For the Repos tab, `activate_context_selection` toggles the item (multi-select,
like Owners) and calls `workspace.set_scope(checked_roots)` followed by
`refresh_all_panels`, replacing today's `switch_database` + focus-move. The
`switch_database` method is removed (or reduced to the seed-one-repo path).

## Data flow

1. User toggles a repo in the Repos panel → `set_scope` opens/drops DBs → panels
   refresh.
2. Refresh merges rows across `workspace.dbs()`, tags each with its repo, swaps
   author→repo when `is_multi()`, sorts by recency.
3. Selecting a row → `db_for_item` resolves the owning DB → right pane renders
   from that DB with the item's repo root as `repo_path`.
4. Heal / delete / live-health operate through the resolved per-item DB.
5. `is_multi()` gates Tours-tab visibility.

## Edge cases

- **Uncheck-last** is a no-op; the scope never empties.
- **Owner filter hides a checked repo:** it stays in scope (it can't be
  unchecked while hidden). Minor, accepted for v1.
- **Empty repos** (no bookmarks/collections) merge cleanly — they contribute
  nothing.
- **Id collision across repos** is handled by keying selection, live-health, and
  `db_for_item` on `(repo_root, id)`.

## Testing

**Unit**
- `RepoWorkspace`: `set_scope` opens newly-checked and drops unchecked without
  reopening unchanged DBs; uncheck-last is a no-op; `focus_db` resolves per the
  cursor/first-checked rule.
- Merged builder: tags each row with its repo; swaps author→repo when multi;
  exactly-one-checked output is identical to today (parity test).
- `db_for_item` resolves the correct DB by repo root.
- Tours visibility flips with `is_multi()`.
- Relocated `find_bookmark_across` still matches full id and ≥4-char prefix.

**Integration**
- Two temp repos, each with bookmarks → check both → merged list shows both,
  each labeled with its repo name.
- Select a repo-B bookmark → preview, heal, and delete all target repo B.
- Search across both repos merges results tagged by repo.
- CLI regression: `find_bookmark_across` relocation keeps existing CLI tests
  green.
