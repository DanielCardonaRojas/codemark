# ADR 0001: Moving and copying collections across git branches

- **Status:** Proposed
- **Date:** 2026-11-08
- **Codemark context:** collection `collection-branch-transfer`. Load it with `codemark tour show collection-branch-transfer --format markdown`.

## Context

Collections (tours) are often built on a feature branch that later merges. The
collection keeps the feature branch's label and resolution history. Once the
tour is shared, everyone else sees context for a branch that no longer exists.
Users need a way to move a collection to another branch, or copy it there.

What the codebase does today:

- **Nothing is branch-scoped.** The only link between a collection and a branch
  is `collections.created_branch` (V9). It is a free-text label set at creation,
  and nothing ever updates it. `bookmarks.commit_hash` and
  `resolutions.commit_hash` only record where a bookmark came from.
- **Stored paths never include a worktree, by design.** `detect_context`
  (`git/context.rs`) takes the parent of `git rev-parse --git-common-dir` as
  the repo root. Every linked worktree therefore opens the main checkout's
  `.codemark/codemark.db`, and `file_path` is always relative to the repo
  root. A path only changes between branches when the file is **renamed**.
- **Resolution only reads the working tree.** `FileProvider` (`vfs.rs`) already
  takes a `commit` argument, but `LocalFileProvider` ignores it.
  `git_context::read_file_at_commit` can read a blob at any revision, but
  nothing calls it. Every worktree shares the git object store, so reading
  directly from git works from any worktree.
- **Bookmarks are unique per repo, path and query.** V22 enforces
  `UNIQUE (repo_id, file_path, query)`, and `insert_bookmark_in_txn` returns
  the existing row when that key already exists. A copy that keeps the same
  path and query can't get its own row.

## Decision

1. **Fork bookmarks.** Moving or copying a collection creates *new* bookmark
   rows. Each new row starts with exactly one `Active` resolution at the target
   commit. **Resolution history is discarded.**
2. **A forked row belongs to the collection it was forked into.** Migration V27:
   - Adds `bookmarks.scope_collection_id`, which records the collection that
     owns a forked row. It is NULL for normal adds.
   - Adds `bookmarks.branch`, which records the target branch. It is
     informational only and not part of the key.
   - **Rebuilds the `bookmarks` table** to remove V7's inline
     `UNIQUE(file_path, query)`. Changing the V22 indexes alone isn't enough,
     because that constraint is still on the live table. The rebuild copies all
     rows and recreates the foreign keys, the FTS triggers and the other indexes.
   - Recreates both V22 partial unique indexes with the scope included:
     - `(repo_id, file_path, query, COALESCE(scope_collection_id, ''))` where
       `repo_id IS NOT NULL`
     - `(file_path, query, COALESCE(scope_collection_id, ''))` where
       `repo_id IS NULL` (legacy rows)

   **Why:** keying on `branch` alone would make repeated copies to the same
   branch collide. Examples are copying a tour to `main` twice, or copying two
   tours that share a bookmark. Keying on the owning collection gives every
   copy its own rows.

   **Dedup is unchanged:** a normal `add` leaves the scope NULL.
3. **Resolve against the target branch's committed tree, not a checkout.**
   - A new `GitRevFileProvider` reads files at the target revision.
   - git2 tree-to-tree diffing with `find_similar` maps renamed paths from the
     source commit to the target commit. The source commit is the bookmark's
     current resolution commit.
   - The existing tier ladder (`resolution::resolve`) does the matching.
   - **Only exact matches count by default.** A relaxed, minimized or
     hash-fallback query can land on an unrelated node once the original code is
     gone. Those matches count as **unmatched** unless `--allow-fuzzy` is passed.
   - Every step in the dry-run report includes its match tier and whether the
     content hash changed. Fuzzy matches are flagged for review even when they
     are allowed.
4. **Two operations share one engine:** `plan_transfer` does a dry run and
   `apply_transfer` writes the result in a single transaction.
   - **Copy:**
     - Creates a new collection, named with `--as` or `<name>@<branch>` by default.
     - Carries over the description, prompt, tags and links.
     - Leaves out publish and import metadata.
     - Its forked rows are scoped to the new collection's id.
   - **Move:**
     - Updates the existing collection in place, so its id, name, links and
       publish metadata are kept.
     - Forks the **matched** steps into rows scoped to that collection, swaps
       them in as members, and sets `created_branch`.
     - Collisions with rows the collection forked in an earlier move are
       avoided: the old rows are detached and deleted before the new forks are
       inserted.
     - Deletes the old rows of **matched** steps when no other collection uses
       them.
     - **Never deletes the original row of an unmatched step.** With `drop`, the
       step leaves the collection but its bookmark stays, with notes, comments
       and tags intact. The report gives its id so it can be rebuilt or added
       back.
5. **Unmatched steps:** `--on-unmatched fail|drop`, default `fail`.
   - `fail` writes nothing and exits non-zero.
   - The JSON report lists the unmatched steps (bookmark id, query, path, note,
     reason, best fuzzy candidate if one exists) and the collection's stored
     `prompt`.
6. **The agent path lives in the skill, not the CLI.** The skill tells the
   agent to:
   - run `--dry-run` first;
   - when only some steps are missing, apply with `drop` and rebuild those steps
     on the target branch from the prompt and notes;
   - when most steps are missing, re-run the whole prompt into a new collection.

   The CLI never launches an agent.
7. **Surfaces:**
   - The CLI comes first: `codemark tour move|copy` (mirrored under
     `collection`), plus the skill update.
   - TUI support is a follow-up. It needs a git-branch picker, a dialog with
     more than two options, and a background task.

## Consequences

- **Pro:** Copies are independent. Re-baselining on the target branch never
  changes the source collection's hashes or health.
- **Pro:** No checkout or worktree is needed, and file renames are handled.
- **Pro:** The `prompt` stored with each collection (V24) becomes useful when a
  tour has to be rebuilt.
- **Con:** Bookmark identity now includes the owning collection. A plain `add`
  on the same path and query creates or reuses the unscoped row, never a forked
  one. Queries and the TUI branch filter can use `bookmarks.branch` as an extra
  way to filter.
- **Con:** V27 rebuilds the whole `bookmarks` table, which is the riskiest step
  in the migration. It needs a test that the data and triggers survive, like the
  one added for the V26 `collection_links` rebuild.
- **Con:** Deleting a collection should also delete the forked rows it owns when
  no other collection uses them. Otherwise they become orphans.
- **Con:** Requiring exact matches by default fails more often when code drifts
  between branches. That is deliberate: a false Active is worse than a step
  that is reported as unmatched.
- **Con:** Forked rows duplicate tags, notes and comments, and their embeddings
  have to be regenerated.

## Alternatives considered

- **Share bookmark rows and re-baseline them.** No schema change is needed. But
  the source collection's hashes and history change too, and history can't be
  discarded for just one of the two collections.
- **Key forks on `branch`.** This was the first draft. Two copies of a tour on
  the same branch collide, so copies aren't independent.
- **Drop the UNIQUE index.** Simpler, but it gives up the database-level
  deduplication that V22 added.
- **Check out the target branch, or resolve inside its worktree.** This is
  fragile: the branch may not be checked out anywhere, and the shared database
  always resolves paths against the main checkout.
- **Have the CLI spawn an agent.** That couples the CLI to particular agent
  runtimes. A JSON report that the skill acts on is easier to compose.

## Out of scope / known issue

Running `add` from a worktree outside the main checkout probably fails with
`relative_to_root` ("not under repo root"). Resolution from a linked worktree
reads files from the main checkout. This is unrelated to transfers and is
tracked separately if needed.
