# Multi-repo queries in the TUI — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Let the TUI query several repositories at once (multi-select in the Repos panel), merging bookmarks/collections/semantic+FTS search across their databases while keeping single-repo behavior identical to today.

**Architecture:** Replace the single `self.db: Database` in the browser with a stateful `RepoWorkspace` that owns one open `Database` per *checked* repo. Rows are tagged with their repo; per-item operations resolve the owning DB. Two shared primitives move into `codemark-core`: `find_bookmark_across` and an embed-once semantic search (so the model loads once and each DB is searched with the same query vector). The CLI's single-DB semantic search is rewired to the same primitive for parity.

**Tech Stack:** Rust workspace (`codemark-core`, `codemark-cli`, `codemark-tui`), rusqlite + sqlite-vec, ratatui, tokio, candle (behind the default-on `semantic` feature).

**Spec:** `docs/superpowers/specs/2026-08-07-tui-multi-repo-queries-design.md`

**Conventions:**
- Tests: `cargo test -p <crate> <name>`. The `semantic` feature is default-on, so semantic tests run under a normal `cargo test -p codemark-core`. To prove the no-semantic build still compiles use `--no-default-features` where noted.
- Format after edits: `cargo fmt`. Lint: `cargo clippy -p <crate>`.
- Commit after every green step.

---

## Phase 0 — Shared core primitives

### Task 0.1: Relocate `find_bookmark_across` into core

**Files:**
- Modify: `crates/codemark-core/src/storage/workspace.rs`
- Modify: `crates/codemark-cli/src/cli/handlers.rs:1194-1211` (remove the local fn, re-export/call core)
- Test: `crates/codemark-core/src/storage/workspace.rs` (inline `#[cfg(test)]`)

**Step 1: Write the failing test** in `workspace.rs` tests module:

```rust
#[test]
fn find_bookmark_across_matches_prefix_in_second_db() {
    // build two in-memory dbs; insert a bookmark with a known ULID into db2
    let db1 = Database::open_in_memory().unwrap();
    let db2 = Database::open_in_memory().unwrap();
    let bm = /* sample bookmark with id "01HZZ...FULL" */;
    db2.insert_bookmark(&bm).unwrap();
    let dbs = vec![("a".to_string(), db1), ("b".to_string(), db2)];
    let (found, _db) = Workspace::find_bookmark_across(&dbs, &bm.id[..6]).unwrap();
    assert_eq!(found.id, bm.id);
}
```

**Step 2: Run to verify it fails**

Run: `cargo test -p codemark-core find_bookmark_across_matches_prefix_in_second_db`
Expected: FAIL — `Workspace::find_bookmark_across` not found.

**Step 3: Add the function** to `impl Workspace` (port the CLI body verbatim, using core's `extract_id` equivalent — inline the `#` strip if `extract_id` is CLI-only):

```rust
pub fn find_bookmark_across<'a>(
    dbs: &'a [(String, Database)],
    id: &str,
) -> Result<(crate::engine::bookmark::Bookmark, &'a Database)> {
    let id = id.strip_prefix('#').unwrap_or(id);
    for (_label, db) in dbs {
        if let Some(bm) = db.get_bookmark(id)? {
            return Ok((bm, db));
        }
        if id.len() >= 4 {
            if let Ok(Some(bm)) = db.get_bookmark_by_prefix(id) {
                return Ok((bm, db));
            }
        }
    }
    Err(Error::Input(format!("bookmark not found: {id}")))
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p codemark-core find_bookmark_across`
Expected: PASS.

**Step 5: Point the CLI at the core version.** In `handlers.rs`, delete the local `find_bookmark_across` and replace call sites with `Workspace::find_bookmark_across`. Keep `extract_id` handling by passing the extracted id (or rely on the core `#` strip).

Run: `cargo test -p codemark-cli` → Expected: PASS (existing CLI tests green).

**Step 6: Commit**

```bash
git add crates/codemark-core/src/storage/workspace.rs crates/codemark-cli/src/cli/handlers.rs
git commit -m "refactor(core): move find_bookmark_across into Workspace"
```

---

### Task 0.2: Split `SemanticRepo` into embed-once + prepared search

**Files:**
- Modify: `crates/codemark-core/src/storage/semantic_repo.rs`
- Test: same file, `#[cfg(test)]`

**Step 1: Write the failing test** (needs a real model; gate with the semantic feature and mark `#[ignore]` if model download is unavailable in CI — prefer a small deterministic dimension check instead):

```rust
#[tokio::test]
async fn embed_query_then_prepared_matches_search() {
    // Uses AllMiniLmL6V2; may require the model cached locally.
    let repo = SemanticRepo::new(None, EmbeddingModel::AllMiniLmL6V2);
    let mut conn = Database::open_in_memory().unwrap().into_conn(); // or use a Database
    // store one embedding, then compare search() vs embed_query()+search_prepared()
    let emb = repo.embed_query("auth").await.unwrap();
    let prepared = repo.search_prepared(&conn, &emb, 5, None).unwrap();
    let combined = repo.search(&conn, "auth", 5).await.unwrap();
    assert_eq!(prepared.iter().map(|r| &r.id).collect::<Vec<_>>(),
               combined.iter().map(|r| &r.id).collect::<Vec<_>>());
}
```

**Step 2: Run to verify it fails**

Run: `cargo test -p codemark-core embed_query_then_prepared_matches_search`
Expected: FAIL — `embed_query`/`search_prepared` not found.

**Step 3: Add the two methods and refactor `search`/`search_with_threshold`:**

```rust
/// Embed a query string once (loads the model). Reuse the result across DBs.
pub async fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
    crate::embeddings::VecStore::ensure_extension_loaded();
    let provider = self.provider()?;
    provider.embed(query).await.map_err(|e| {
        crate::error::Error::Operation(format!("Failed to generate query embedding: {}", e))
    })
}

/// Search a single connection with a pre-computed query embedding (no model load).
pub fn search_prepared(
    &self,
    conn: &Connection,
    query_embedding: &[f32],
    limit: usize,
    threshold: Option<f32>,
) -> Result<Vec<SearchResult>> {
    crate::embeddings::VecStore::ensure_extension_loaded();
    let store = VecStore::with_metric(query_embedding.len(), self.distance_metric);
    store.search_with_threshold(conn, query_embedding, limit, threshold).map_err(|e| {
        crate::error::Error::Operation(format!("Semantic search failed: {}", e))
    })
}

/// Collection-target variant.
pub fn search_collections_prepared(
    &self,
    conn: &Connection,
    query_embedding: &[f32],
    limit: usize,
    threshold: Option<f32>,
) -> Result<Vec<SearchResult>> {
    crate::embeddings::VecStore::ensure_extension_loaded();
    let store = self.collection_store(query_embedding.len());
    store.search_with_threshold(conn, query_embedding, limit, threshold).map_err(|e| {
        crate::error::Error::Operation(format!("Semantic search failed: {}", e))
    })
}
```

Then rewrite `search_with_threshold` (and the collection equivalent) as thin
wrappers: `let emb = self.embed_query(query).await?; self.search_prepared(conn, &emb, limit, threshold)`.

**Step 4: Run test to verify it passes**

Run: `cargo test -p codemark-core embed_query_then_prepared_matches_search`
Expected: PASS.

**Step 5: Commit**

```bash
git add crates/codemark-core/src/storage/semantic_repo.rs
git commit -m "refactor(core): split SemanticRepo into embed_query + search_prepared"
```

---

## Phase 1 — RepoWorkspace

### Task 1.1: Introduce `RepoWorkspace`

**Files:**
- Create: `crates/codemark-tui/src/browser/workspace.rs`
- Modify: `crates/codemark-tui/src/browser/mod.rs` (add `mod workspace;`)
- Test: `crates/codemark-tui/src/browser/workspace.rs` (`#[cfg(test)]`)

**Design:** owns checked DBs keyed by repo root (a `Vec<(PathBuf, Database)>` is fine; add an `IndexMap` only if a helper needs it) plus a `focus: PathBuf`.

**Step 1: Write failing tests:**

```rust
#[test]
fn set_scope_opens_and_drops_without_reopening_unchanged() {
    // create two temp repos each with .codemark/codemark.db
    let mut ws = RepoWorkspace::new(root_a.clone()).unwrap(); // seeds A as checked+focus
    assert_eq!(ws.is_multi(), false);
    ws.set_scope(&[root_a.clone(), root_b.clone()]).unwrap();
    assert!(ws.is_multi());
    assert!(ws.get(&root_a).is_some() && ws.get(&root_b).is_some());
    // unchanged A connection is reused (assert by pointer/path identity)
    ws.set_scope(&[root_a.clone()]).unwrap();
    assert!(ws.get(&root_b).is_none());
    assert_eq!(ws.is_multi(), false);
}

#[test]
fn uncheck_last_is_noop() {
    let mut ws = RepoWorkspace::new(root_a.clone()).unwrap();
    ws.set_scope(&[]).unwrap();           // attempt to empty
    assert!(ws.get(&root_a).is_some());   // scope preserved
}
```

**Step 2: Run to verify fail**

Run: `cargo test -p codemark-tui set_scope_opens_and_drops`
Expected: FAIL — type missing.

**Step 3: Implement `RepoWorkspace`:**

```rust
pub struct RepoWorkspace {
    dbs: Vec<(PathBuf, Database)>, // repo_root -> open db, checked set, insertion-ordered
    focus: PathBuf,
}

impl RepoWorkspace {
    fn db_path(root: &Path) -> PathBuf { root.join(".codemark").join("codemark.db") }

    pub fn new(root: PathBuf) -> Result<Self> {
        let db = Database::open(&Self::db_path(&root))?;
        Ok(Self { dbs: vec![(root.clone(), db)], focus: root })
    }

    pub fn set_scope(&mut self, checked: &[PathBuf]) -> Result<()> {
        if checked.is_empty() { return Ok(()); } // uncheck-last is a no-op
        // drop unchecked
        self.dbs.retain(|(root, _)| checked.contains(root));
        // open newly-checked (skip already-open)
        for root in checked {
            if !self.dbs.iter().any(|(r, _)| r == root) {
                if let Ok(db) = Database::open(&Self::db_path(root)) {
                    self.dbs.push((root.clone(), db));
                }
            }
        }
        // keep focus valid
        if !self.dbs.iter().any(|(r, _)| r == &self.focus) {
            self.focus = self.dbs[0].0.clone();
        }
        Ok(())
    }

    pub fn is_multi(&self) -> bool { self.dbs.len() > 1 }
    pub fn dbs(&self) -> impl Iterator<Item = (&Path, &Database)> {
        self.dbs.iter().map(|(r, d)| (r.as_path(), d))
    }
    pub fn get(&self, root: &Path) -> Option<&Database> {
        self.dbs.iter().find(|(r, _)| r == root).map(|(_, d)| d)
    }
    pub fn focus_db(&self) -> &Database { self.get(&self.focus).unwrap() }
    pub fn set_focus(&mut self, root: PathBuf) { if self.get(&root).is_some() { self.focus = root; } }
}
```

**Step 4: Run tests → PASS.**

**Step 5: Commit**

```bash
git add crates/codemark-tui/src/browser/workspace.rs crates/codemark-tui/src/browser/mod.rs
git commit -m "feat(tui): add RepoWorkspace owning one Database per checked repo"
```

---

### Task 1.2: Swap `Browser.db` for `RepoWorkspace` (compile-only, single-repo parity)

**Files:**
- Modify: `crates/codemark-tui/src/browser/mod.rs` (struct field `db: Database` → `workspace: RepoWorkspace`; `Browser::new`)
- Modify: every `self.db` reader — introduce `fn db(&self) -> &Database { self.workspace.focus_db() }` and replace `self.db` with `self.db()` mechanically first, so the diff compiles with today's behavior.

**Step 1:** Add the field and a `db(&self)` accessor returning `focus_db()`. Replace `self.db` usages with `self.db()` (and `self.workspace` where a path is needed). Keep `switch_database` temporarily delegating to `workspace.set_focus` + `set_scope(&[root])`.

**Step 2:** `cargo build -p codemark-tui` → fix until it compiles.

**Step 3:** `cargo test -p codemark-tui` → Expected: PASS (behavior unchanged; still one repo).

**Step 4: Commit**

```bash
git add crates/codemark-tui/src/browser/mod.rs
git commit -m "refactor(tui): route browser through RepoWorkspace focus_db (no behavior change)"
```

---

## Phase 2 — Row tagging & merged building

### Task 2.1: Add a `repo` tag to `PanelItem`

**Files:**
- Modify: `crates/codemark-tui/src/component/panel/item.rs`
- Test: same file.

**Step 1: Failing test:**

```rust
#[test]
fn panel_item_carries_repo() {
    let item = PanelItem::new("x").repo("codemark", "/home/u/codemark");
    assert_eq!(item.repo_name(), Some("codemark"));
    assert_eq!(item.repo_root(), Some("/home/u/codemark"));
}
```

**Step 2:** Run `cargo test -p codemark-tui panel_item_carries_repo` → FAIL.

**Step 3:** Add fields `repo_name: Option<String>`, `repo_root: Option<String>`, builder `repo(name, root)`, getters `repo_name()`, `repo_root()`.

**Step 4:** Run → PASS. **Step 5:** Commit `feat(tui): add repo tag to PanelItem`.

---

### Task 2.2: Merged content builder across the workspace

**Files:**
- Modify: `crates/codemark-tui/src/browser/tabbed_panel.rs` (`build_content_items`, `bookmark_to_panel_item_cached`)
- Modify: `crates/codemark-tui/src/browser/mod.rs:1584+` (`refresh_all_panels_inner`)
- Test: `crates/codemark-tui/src/browser/tabbed_panel.rs`

**Step 1: Failing test** for a new `build_content_items_multi(workspace, live, bm_live)`:

```rust
#[test]
fn multi_repo_bookmarks_tagged_and_author_replaced() {
    // two in-memory-ish repos, one bookmark each with created_by="alice"/"bob"
    let items = build_bookmarks_merged(&workspace, &live); // helper under test
    // multi mode: metadata shows repo name, not author
    assert!(items.iter().any(|i| i.repo_name() == Some("repo_a")));
    assert!(items.iter().all(|i| i.metadata() != Some("alice")));
    // sorted by created_at desc across repos
    assert!(is_sorted_desc_by_created_at(&items));
}

#[test]
fn single_repo_bookmarks_keep_author() {
    let items = build_bookmarks_merged(&single_repo_ws, &live);
    assert_eq!(items[0].metadata(), Some("alice")); // identical to today
}
```

**Step 2:** Run → FAIL.

**Step 3:** Implement a merge layer that, for each `(root, db)` in `workspace.dbs()`, calls the existing per-DB builders, then:
- tags each item `.repo(repo_name_from_root(root), root)`;
- if `workspace.is_multi()`, sets metadata to the repo name (author swap); else leaves it;
- concatenates and sorts by `created_at` desc.
Repo display name = `source_label_from_path`-style leaf (last path component of root).
Wire `refresh_all_panels_inner` to call the merged builder for bookmarks and collections.

**Step 4:** Run → PASS. Also `cargo test -p codemark-tui` full → PASS. **Step 5:** Commit `feat(tui): merge bookmarks/collections across checked repos with repo tags`.

---

## Phase 3 — Per-item DB resolution & live-health keying

### Task 3.1: `db_for_item` helper

**Files:** Modify `crates/codemark-tui/src/browser/mod.rs`. Test in same file.

**Step 1: Failing test:** given a merged item tagged with repo B's root, `db_for_item(item)` returns repo B's DB (assert by `db.path()`).

**Step 2:** Run → FAIL.

**Step 3:** `fn db_for_item(&self, item: &PanelItem) -> &Database { item.repo_root().and_then(|r| self.workspace.get(Path::new(r))).unwrap_or_else(|| self.db()) }`.

**Step 4:** Run → PASS. **Step 5:** Commit.

### Task 3.2: Route preview / heal / delete through `db_for_item`

**Files:** Modify `crates/codemark-tui/src/browser/events.rs` (heal target ~1348+, delete ~1574+, activate ~1381+), `right_pane.rs` render calls, `data.rs` heal task (`db_path` derivation).

**Step 1:** For each selected-item operation, replace `self.db()` / `self.db.path()` with `self.db_for_item(selected)` / its `.path()`. The heal background task takes a `db_path` — derive it from the item's repo root.

**Step 2:** `cargo build -p codemark-tui` → fix.

**Step 3:** Add an integration-style test (Phase 6) later; here just `cargo test -p codemark-tui` → PASS.

**Step 4: Commit** `feat(tui): resolve preview/heal/delete against the selected item's repo DB`.

### Task 3.3: Key live-health caches by `(repo_root, id)`

**Files:** Modify `mod.rs` cache types (`collection_live_health`, `bookmark_live_health`), `spawn_live_health_task`, apply/lookup sites (`bookmark_to_panel_item_cached` health lookup, `collection_health_status` lookup).

**Step 1: Failing test:** two repos with a colliding bookmark id but different health → after applying live health, each row shows its own repo's status (no cross-contamination).

**Step 2:** Run → FAIL.

**Step 3:** Change key type to `String` of form `{repo_root}\x1f{id}` (or a `(String,String)` tuple). Update the fan-out to spawn per checked repo, and every `.get(&id)` to `.get(&key(root, id))`. Preserve the `health_generation` guard.

**Step 4:** Run → PASS, plus full `cargo test -p codemark-tui`. **Step 5:** Commit `fix(tui): key live-health by (repo_root, id) to avoid cross-repo collisions`.

---

## Phase 4 — Selection flow & Tours visibility

### Task 4.1: Repos panel becomes multi-select driving `set_scope`

**Files:** Modify `crates/codemark-tui/src/browser/tabbed_panel.rs` (`new_repos_accounts`: make repos panel `multi_select(true)`), `crates/codemark-tui/src/browser/events.rs:1344` (`activate_context_selection` Repos arm), `bindings.rs:42` (label "Select repo" → "Toggle repo").

**Step 1:** In the Repos arm, replace `switch_database` with: `panel.activate_selected()`, collect `active_items()` roots, `self.workspace.set_scope(&roots)`, `self.workspace.set_focus(selected_root)`, `self.refresh_all_panels()`. Enforce uncheck-last no-op (already handled in `set_scope`, but also prevent un-highlighting the last active item in the panel).

**Step 2:** `cargo build` → fix. Manual: toggling repos updates the merged list.

**Step 3:** `cargo test -p codemark-tui` → PASS.

**Step 4: Commit** `feat(tui): multi-select repos to define the query scope`.

### Task 4.2: Hide Tours when multi

**Files:** Modify `update_tours_tab_visibility` in `mod.rs`.

**Step 1: Failing test:** with 2 checked repos, Tours tab is hidden; with 1, visible (given logged-in precondition).

**Step 2:** Run → FAIL.

**Step 3:** Add `|| self.workspace.is_multi()` to the hide condition.

**Step 4:** Run → PASS. **Step 5:** Commit `feat(tui): hide Tours tab in multi-repo mode`.

---

## Phase 5 — Search fan-out

### Task 5.1: FTS across the workspace

**Files:** Modify `execute_bookmark_search` (mod.rs:642) and `execute_collection_search` (mod.rs:734).

**Step 1: Failing test:** two repos, each with a bookmark matching "auth" → FTS returns both, tagged by repo.

**Step 2:** Run → FAIL.

**Step 3:** In the `SearchMode::Fts` arm, loop `self.workspace.dbs()`, run `db.search_bookmarks(...)`, tag results with repo, concatenate; emit merged `SearchResults`. Keep it synchronous. Do the same for collections.

**Step 4:** Run → PASS. **Step 5:** Commit `feat(tui): FTS search across all checked repos`.

### Task 5.2: Semantic across the workspace (embed once)

**Files:** Modify the `SearchMode::Semantic` arm of `execute_bookmark_search` (mod.rs:669) and the collection equivalent.

**Step 1: Failing test** (`semantic` feature; may need model cache — mark `#[ignore]` if unavailable): index two repos, one semantic query → hits from both, ordered by ascending distance, capped at `SEARCH_RESULT_LIMIT`.

**Step 2:** Run → FAIL.

**Step 3:** In the blocking task:
1. Resolve model/metric/threshold once from `self.workspace.focus_db()`'s config dir.
2. `let emb = semantic_repo.embed_query(&query).await?` — once.
3. Collect the checked repos' `db_path`s before spawning (workspace DBs aren't `Send`); open a fresh `Database` per path inside the task, call `semantic_repo.search_prepared(db.conn(), &emb, limit, threshold)`, tag hits `(distance, repo_root)`.
4. Merge all, sort by distance ascending, `truncate(SEARCH_RESULT_LIMIT)`, resolve each to its full bookmark from its own DB, tag row with repo.

**Step 4:** Run → PASS. **Step 5:** Commit `feat(tui): cross-repo semantic search (embed once, merge by distance)`.

---

## Phase 6 — CLI parity & integration

### Task 6.1: CLI semantic search fans out

**Files:** Modify `crates/codemark-cli/src/cli/handlers/search.rs:169` (`handle_semantic_search`) and the collection semantic path (~306).

**Step 1: Failing test** (CLI integration; note the worktree path caveat — see `docs`/memory on `cli_integration.rs`): two `--repo` refs, `--semantic` → merged, distance-ordered JSON hits from both.

**Step 2:** Run → FAIL.

**Step 3:** Replace `open_db(cli)?` with `open_all_dbs_with_extra_and_repos(cli, &[], &args.repo)?` (+ existing email/owner filters). Resolve model once from the primary/first DB config; `embed_query` once; loop DBs with `search_prepared`; merge by distance; truncate to `args.limit`; annotate each hit with its source label for output.

**Step 4:** Run → PASS; `cargo test -p codemark-cli` full → PASS.

**Step 5: Commit** `feat(cli): multi-repo semantic search via embed-once primitive`.

### Task 6.2: End-to-end TUI integration test

**Files:** Create/extend `crates/codemark-tui/tests/` (or an existing integration harness).

**Step 1: Failing test:** build two temp repos with bookmarks; construct a `Browser`; `set_scope` both; assert: merged list shows both with repo names; selecting a repo-B bookmark makes `db_for_item` resolve B; delete targets B; Tours hidden.

**Step 2:** Run → FAIL.

**Step 3:** Implement any small seams needed for testability (e.g. a constructor that seeds the workspace with an explicit repo set).

**Step 4:** Run → PASS.

**Step 5: Commit** `test(tui): end-to-end multi-repo browse/select/delete`.

### Task 6.3: Cleanup pass

- Remove the now-dead `switch_database` if fully superseded (or keep as `set_scope(&[root])` seed).
- `cargo fmt --all` and `cargo clippy --workspace` → fix warnings introduced.
- Verify `--no-default-features` build of `codemark-tui`/`codemark-cli` still compiles (semantic off).
- Commit `chore: fmt/clippy and remove dead switch_database path`.

---

## Risk notes for the implementer

- **`Send`/`!Send` across the spawn boundary:** `Database`/rusqlite connections aren't `Send`. Every background task must capture `db_path`s (not `Database`) and reopen inside the task — this is the existing pattern (`execute_bookmark_search`, `perform_heal`). The workspace itself lives on the UI thread only.
- **Distance dimension mismatch:** per the spec's accepted limitation, a repo indexed with a different-dimension model makes `VecStore::search_with_threshold` **error** (it validates dimension). v1 has no guard; if a test hits this, that's expected — do not add a guard unless the spec's "Known limitations" decision is revisited.
- **Nav-path performance:** `set_scope` must not reopen unchanged connections (Task 1.1 test enforces this) — toggling one repo should not reopen the rest.
- **CLI integration tests in worktrees** have a hardcoded-root caveat (see memory `cli-integration-tests-worktree-path`); run 6.1's test from the main checkout if it fails only in the worktree.
