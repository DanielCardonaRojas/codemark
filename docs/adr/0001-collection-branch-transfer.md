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
- **Stored paths never include a worktree.** `detect_context`
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
   rows, stamped with the target branch. Each new row starts with exactly one
   `Active` resolution at the target commit. **Resolution history is discarded.**
2. **Add a branch to the unique key.** Migration V27 adds the nullable column
   `bookmarks.branch` and rebuilds the unique index as
   `(repo_id, file_path, query, COALESCE(branch, ''))`.
   - A normal `add` leaves `branch` NULL, so deduplication works as it does today.
3. **Resolve against the target branch's committed tree, not a checkout.**
   - A new `GitRevFileProvider` reads files at the target revision.
   - git2 tree-to-tree diffing with `find_similar` maps renamed paths from the
     source commit to the target commit. The source commit is the bookmark's
     current resolution commit.
   - The existing tier ladder (`resolution::resolve`) does the matching.
   - `--strict` accepts only exact matches. By default, any tier except
     `Failed` counts as a match.
4. **Two operations share one engine:** `plan_transfer` does a dry run and
   `apply_transfer` writes the result in a single transaction.
   - **Copy:**
     - Creates a new collection, named with `--as` or `<name>@<branch>` by default.
     - Carries over the description, prompt, tags and links.
     - Leaves out publish and import metadata.
   - **Move:**
     - Updates the existing collection in place, so its id, name, links and
       publish metadata are kept.
     - Swaps its members for the forked rows and sets `created_branch`.
     - Deletes old bookmark rows that no other collection still references.
5. **Unmatched steps:** `--on-unmatched fail|drop`, default `fail`.
   - `fail` writes nothing and exits non-zero.
   - The JSON report lists the unmatched steps (query, path, note, reason) and
     the collection's stored `prompt`.
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
- **Con:** Bookmark identity now includes `branch`. A plain `add` on the same
  path and query does not reuse a forked row. Queries and the TUI branch filter
  should start treating `bookmarks.branch` as a filter dimension.
- **Con:** Forked rows duplicate tags, notes and comments, and their embeddings
  have to be regenerated.

## Alternatives considered

- **Share bookmark rows and re-baseline them.** No schema change is needed. But
  the source collection's hashes and history change too, and history can't be
  discarded for just one of the two collections.
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
