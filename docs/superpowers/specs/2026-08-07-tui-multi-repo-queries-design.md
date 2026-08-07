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
- **Semantic search across repos** is the primary use case and does **not exist
  today** (both the CLI and TUI semantic paths are single-DB). A new core
  primitive embeds the query **once** and searches each DB's connection,
  merging by distance. It is wired into **both** the TUI and the CLI (parity).
- **Model comparability:** cross-repo semantic search **assumes all checked
  repos are indexed with the same embedding model/metric** — no guard for v1.
  Distances from a differently-indexed repo would rank incorrectly. This is an
  accepted known limitation; see "Known limitations" for the future guard.

## Non-goals

- No cross-repo *tours* (Tours is hidden in multi-repo mode).
- No new "create bookmark" flow. TUI writes remain item-scoped (delete, heal,
  publish/sync of a selected item).
- No embedding-model **compatibility guard** for cross-repo semantic search (v1
  assumes a shared model; see Known limitations).
- No general "multi-repo query engine" in core — only the two targeted shared
  primitives (`find_bookmark_across`, the embed-once semantic search).

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

### Step 0 — shared core primitives

Two targeted extractions/additions into `codemark-core`, done first so both the
CLI and TUI consume them:

**0a. Relocate `find_bookmark_across`.** Move it (currently
`crates/codemark-cli/src/cli/handlers.rs`) into `codemark-core` (e.g.
`Workspace::find_bookmark_across(dbs, id) -> Result<(Bookmark, &Database)>`,
matching full-id then ≥4-char prefix). The CLI calls the relocated version so
its tests stay green; the TUI reuses it for open-by-id and resolving a selected
row's DB. Everything else in the CLI stays put. The two DB filters
(`filter_dbs_by_user_email`, `filter_dbs_by_repo_owner`) may move too if
convenient — optional, not required.

**0b. Embed-once semantic primitive.** Today `SemanticRepo::search(conn, query,
limit)` rebuilds the embedding provider (loads the model) and re-embeds the
query on every call — so looping it over N repos would load the model N times.
Split embedding from per-connection search:

- `SemanticRepo::embed_query(query) -> Result<Vec<f32>>` — builds the provider
  once and returns the query embedding.
- `SemanticRepo::search_prepared(conn, &embedding, limit, threshold) ->
  Result<Vec<SearchResult>>` — runs `VecStore::search_with_threshold` against a
  single connection using a pre-computed embedding (no model load).
  `VecStore::search_with_threshold` already accepts a `&[f32]`, so this is a
  clean extraction. Add the collection-target variant too
  (`search_collections_prepared`).

The existing `search`/`search_collections` become thin wrappers
(`embed_query` + `search_prepared`) so current single-DB callers are unchanged.

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

### Component 5 — search (FTS and semantic)

Search fans out across all checked DBs and merges results, each tagged by repo.
The existing `request_id` / generation guards are preserved (a merged result set
belongs to one request_id). Results carry the repo so opening one resolves the
correct DB.

**FTS.** `Database::search_bookmarks` is a per-DB `LIKE` scan with no global
score, so the merge is a concatenation. The TUI's `execute_bookmark_search` FTS
arm (and the collection equivalent) loops `workspace.dbs()`, runs the per-DB
search, tags each row with its repo, and concatenates. It stays synchronous.

**Semantic (primary use case).** Built on the Step 0b primitive so the model
loads once per query, not once per repo:

1. Resolve model/metric/threshold **once** from the **focus repo's** config
   (`Config::load_layered`). Per the model-comparability decision, this single
   config is applied to all checked repos (v1 assumes a shared model).
2. `embed_query(query)` once.
3. For each checked DB: `search_prepared(db.conn(), &embedding, limit,
   threshold)`, tagging each `SearchResult` with its repo. A repo with no vec
   index / no embeddings simply returns nothing.
4. Merge all hits, **global sort by distance ascending, truncate to
   `SEARCH_RESULT_LIMIT`**, then resolve each surviving hit to its full bookmark
   from its own DB.

This runs on the existing blocking task (model load + embed). The same embed-
once/merge flow applies to collection semantic search.

**CLI parity.** `handle_semantic_search` (currently `open_db` single-DB) is
rewired to `open_all_dbs*` + the same embed-once/merge flow, so `codemark search
--semantic --repo a/b --repo c/d` works and matches the TUI. FTS CLI search
already fans out and is unchanged.

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
- **Repo without a semantic index** contributes no semantic hits (its
  `search_prepared` returns empty); the other repos still return results.

## Known limitations

- **Cross-repo semantic ranking assumes a shared embedding model/metric.** v1
  applies one model (from the focus repo) to every checked repo and merges raw
  distances. If a repo was indexed with a different model or dimension, its
  distances are not comparable and it will rank incorrectly (or, on a dimension
  mismatch, the underlying `VecStore` query may error for that DB). A future
  guard could resolve each repo's configured model and either skip or warn on
  mismatch (the vec-table dimension is the reliable signal; per-embedding model
  identity is not currently stored). Out of scope for v1 by decision.

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
- `SemanticRepo::embed_query` + `search_prepared` produce the same results as the
  old `search` for a single DB (refactor-parity), and `search_prepared` runs
  without reloading the model.

**Integration**
- Two temp repos, each with bookmarks → check both → merged list shows both,
  each labeled with its repo name.
- Select a repo-B bookmark → preview, heal, and delete all target repo B.
- FTS search across both repos merges and tags results by repo.
- **Semantic search across both repos**: index both, run one semantic query,
  assert hits from both repos appear, ordered by ascending distance, capped at
  the limit — and the embedding model is loaded once (not once per repo).
- CLI regression: `find_bookmark_across` relocation keeps existing CLI tests
  green; `codemark search --semantic` with multiple `--repo` returns merged,
  distance-ordered hits.
