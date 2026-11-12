#!/usr/bin/env bash
#
# Build the curated fixture knowledge base the README demo tapes record against.
#
# The tapes used to run against the repo's live working database
# (.codemark/codemark.db). We dogfood codemark for issue context, so that DB
# keeps gaining WIP/issue collections, which leak into the recordings and shift
# the position-based keystrokes in the tapes. This script seeds a small,
# deterministic database instead: a known set of bookmarks (tags, notes,
# comments) and collections (descriptions, tags, links) that show off the
# features the gallery demonstrates.
#
# It seeds from scratch with the CLI (no hand-edited binary), so the fixture
# always matches the current schema. capture.sh calls it on every capture, and
# CI runs it on every PR, so a renamed function or changed CLI flag fails there
# instead of at the next capture.
#
# Usage: screenshots/seed-fixture.sh <dest-dir>
#
#   <dest-dir>  where to create the fixture: a clone of this repo at
#               FIXTURE_REV with the seeded .codemark/codemark.db. Must not
#               exist yet. Browse it with `codemark-tui` from that directory.
#
# Environment:
#   CODEMARK     codemark CLI to seed with (default: `codemark` on PATH)
#   FIXTURE_REV  commit to check out (default: the pin below)
#
# The clone is pinned so the gallery's code previews and health states don't
# change with every commit to main. Bookmarks are located by *function name*
# (not line numbers), and codemark generates the structural query exactly as it
# would for a user. When you bump the pin, re-run this script: if a bookmarked
# function was renamed or removed, it fails and names the function.
#
# After seeding, the script commits two small edits in the clone so the gallery
# shows more than "healthy": one bookmarked function is modified (Drifted) and
# one is removed (Broken).

set -euo pipefail

DEST="${1:?usage: seed-fixture.sh <dest-dir>}"
CODEMARK="${CODEMARK:-codemark}"
FIXTURE_REV="${FIXTURE_REV:-559038abfa951c914e8a29f4b71eb4e68c2707f9}"
# An inherited CODEMARK_DB would point every CLI call below at that database
# (e.g. your working one) instead of the fixture's.
unset CODEMARK_DB
REPO_ROOT="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

[ ! -e "$DEST" ] || {
  echo "error: $DEST already exists; pass a fresh path" >&2
  exit 1
}
git -C "$REPO_ROOT" cat-file -e "${FIXTURE_REV}^{commit}" 2>/dev/null || {
  echo "error: fixture commit $FIXTURE_REV not found in $REPO_ROOT" >&2
  echo "       (shallow clone? fetch full history: git fetch --unshallow)" >&2
  exit 1
}

# `--shared` borrows this repo's object store, so the clone is instant and
# needs no network. A named branch (not a detached HEAD) and a GitHub origin
# make the TUI's branch and repo fields look like a normal checkout.
git clone -q --shared --no-checkout "$REPO_ROOT" "$DEST"
git -C "$DEST" checkout -q -B main "$FIXTURE_REV"
git -C "$DEST" remote set-url origin https://github.com/DanielCardonaRojas/codemark.git
# The clone inherits every local branch of this repo as `origin/*`. Drop them so
# the TUI's branch picker lists only the fixture's own branches (main and the
# feature branch seeded below), keeping branches.tape deterministic.
git -C "$DEST" for-each-ref --format='%(refname)' refs/remotes |
  while read -r ref; do git -C "$DEST" update-ref -d "$ref"; done
cd "$DEST"

# The checkout carries the repo's own tracked .codemark/codemark.db. Drop it so
# nothing from the live knowledge base reaches the fixture.
rm -rf .codemark
"$CODEMARK" init >/dev/null 2>&1

# json_field <key>: read one string field from the CLI's JSON on stdin.
json_field() {
  sed -n "s/^[[:space:]]*\"$1\": \"\\(.*\\)\",\\{0,1\\}\$/\\1/p" | head -1
}

# mark <file> <fn> [codemark add args...]: bookmark function <fn> in <file>
# and print the new bookmark's id. The line range comes from a dry-run match on
# the function's name, then `add --range` lets codemark generate its own query.
mark() {
  local file="$1" fn="$2"
  shift 2
  local q="(function_item name: (identifier) @n (#eq? @n \"$fn\")) @target"
  local lines
  lines="$("$CODEMARK" add --dry-run --file "$file" --query "$q" 2>/dev/null | json_field lines)"
  if [ -z "$lines" ]; then
    echo "error: function '$fn' not found in $file (renamed upstream? update seed-fixture.sh)" >&2
    exit 1
  fi
  "$CODEMARK" add --file "$file" --range "$lines" --created-by user "$@" 2>/dev/null | json_field id
}

# collection <name> <description> [codemark tour create args...]
collection() {
  local name="$1" desc="$2"
  shift 2
  "$CODEMARK" tour create "$name" --description "$desc" "$@" >/dev/null 2>&1
}

DOCS="https://github.com/DanielCardonaRojas/codemark"

# ---------------------------------------------------------------------------
# A feature branch with its own collection, for branches.tape: filter the
# Collections list by branch, then clone a `main` collection onto this branch.
# Created first so it sits below the main collections in the newest-first list.
# The branch stays at FIXTURE_REV (the drift/break edits below land on main
# only), so its steps resolve cleanly.
# ---------------------------------------------------------------------------
FEATURE_BRANCH=sync-retry
git checkout -q -b "$FEATURE_BRANCH"
collection tour-sync \
  "Pushing and pulling tours to the Codetours server, with retries on flaky networks" \
  --tag area:sync --tag concept:sync
C=tour-sync
mark crates/codemark-core/src/sync.rs sync --collection $C \
  --tag feature:sync --tag role:entry-point \
  --note "Entry point for \`codemark sync\`: pull remote tours, then push local ones" >/dev/null
mark crates/codemark-core/src/sync.rs sync_pull --collection $C \
  --tag feature:sync \
  --note "Downloads and imports every remote tour newer than the local copy" >/dev/null
mark crates/codemark-core/src/sync.rs upload_pack --collection $C \
  --tag feature:sync \
  --note "Uploads a packed tour; the retry loop for transient failures goes here" >/dev/null
git checkout -q main

# ---------------------------------------------------------------------------
# Collections. The TUI lists them newest first, and demo.tape opens the *top*
# one and steps through it, so `bookmark-resolution` is created last and must
# keep at least four steps. (Each CLI call is a separate process, so creation
# timestamps never tie.)
# ---------------------------------------------------------------------------

collection tui-dashboard \
  "The terminal dashboard: startup, database discovery, and key handling" \
  --tag area:tui

collection server-auth \
  "GitHub device-flow login and JWT authentication for the Codetours server" \
  --tag area:server --tag concept:authentication

collection semantic-search \
  "Local vector embeddings that let you search bookmarks by meaning, no API key required" \
  --tag area:search --tag concept:embeddings

collection bookmark-resolution \
  "How a bookmark survives a refactor: generate a structural query, run it, relax it when the code moves, and heal" \
  --tag area:engine --tag concept:resolution \
  --link "$DOCS#how-it-works,How it works"

# --- bookmark-resolution (the guided tour in demo.tape) ---------------------
C=bookmark-resolution
mark crates/codemark-core/src/query/generator.rs generate_query --collection $C \
  --tag feature:query-generation --tag concept:resolution --tag role:entry-point \
  --note "Turns a selected byte range into a tree-sitter query that anchors on structure (node kinds, names), not line numbers" \
  --comment $'**Why structure, not lines?** Line numbers break on the first edit above the code. A query keeps matching after the code moves.' \
  >/dev/null
mark crates/codemark-core/src/query/matcher.rs run_query --collection $C \
  --tag feature:resolution --tag concept:resolution \
  --note "Runs a stored query against the current file and returns every match" >/dev/null
mark crates/codemark-core/src/engine/resolution.rs resolve --collection $C \
  --tag feature:resolution --tag concept:resolution --tag role:entry-point \
  --note "Resolve a bookmark after the code moves: exact query, then relaxed query, then content-hash fallback" \
  --comment $'Resolution ladder:\n1. exact query match\n2. relaxed query (fewer predicates)\n3. hash fallback\n\nAnything past step 1 marks the bookmark as drifted.' \
  >/dev/null
mark crates/codemark-core/src/query/relaxer.rs relax_query --collection $C \
  --tag feature:resolution --tag concept:resolution \
  --note "Drops the most brittle predicates so a renamed or reshaped node can still be found" >/dev/null
mark crates/codemark-core/src/engine/heal.rs heal_bookmark --collection $C \
  --tag feature:healing --tag concept:resolution \
  --note "Re-resolves a bookmark and records the new location, so it keeps tracking the code" >/dev/null
mark crates/codemark-core/src/engine/hash.rs content_hash --collection $C \
  --tag feature:resolution --tag concept:hashing \
  --note "Whitespace-normalized content hash; tells us if the matched code actually changed" >/dev/null

# --- semantic-search ---------------------------------------------------------
C=semantic-search
mark crates/codemark-core/src/embeddings/provider.rs prepare_embedding_text --collection $C \
  --tag feature:semantic-search --tag concept:embeddings \
  --note "Builds the text we embed for a bookmark: notes, tags, and the code it points at" >/dev/null
mark crates/codemark-core/src/embeddings/local.rs shared_local_provider --collection $C \
  --tag feature:semantic-search --tag concept:embeddings \
  --note "Process-wide local embedding model, loaded once and reused across searches" >/dev/null
mark crates/codemark-core/src/embeddings/vec_store.rs search_with_threshold --collection $C \
  --tag feature:semantic-search --tag concept:embeddings --tag layer:data \
  --note "Nearest-neighbour search over sqlite-vec, filtered by a similarity threshold" >/dev/null

# --- server-auth -------------------------------------------------------------
C=server-auth
mark crates/codetours-server/src/handlers/auth.rs github_device_login --collection $C \
  --tag feature:auth --tag concept:authentication --tag role:entry-point \
  --note "Starts the GitHub device-flow login for the CLI" >/dev/null
mark crates/codetours-server/src/handlers/auth.rs github_device_poll --collection $C \
  --tag feature:auth --tag concept:authentication \
  --note "Polls GitHub until the user approves the device code, then issues a session token" >/dev/null
mark crates/codetours-server/src/handlers/auth.rs generate_jwt --collection $C \
  --tag feature:auth --tag concept:authentication --tag security:auth \
  --note "Signs the session JWT returned to authenticated clients" >/dev/null
mark crates/codetours-server/src/auth/extractor.rs decode_jwt --collection $C \
  --tag feature:auth --tag concept:authentication --tag security:auth \
  --note "Validates the bearer token on every authenticated request" \
  --comment $'TODO: rotate the signing secret without logging every user out.' >/dev/null

# --- tui-dashboard -----------------------------------------------------------
C=tui-dashboard
mark crates/codemark-tui/src/entry.rs run_app --collection $C \
  --tag feature:tui --tag layer:ui --tag role:entry-point \
  --note "TUI startup: terminal setup, database discovery, theme, then the main event loop" >/dev/null
mark crates/codemark-core/src/storage/workspace.rs open_primary --collection $C \
  --tag feature:storage --tag layer:data \
  --note "Finds the repo's .codemark/codemark.db from the working directory" >/dev/null
mark crates/codemark-tui/src/browser/events.rs handle_resize --collection $C \
  --tag feature:tui --tag layer:ui \
  --note "Grows or shrinks the focused pane with + and -" >/dev/null

# --- loose bookmarks (not in any collection) ---------------------------------
mark crates/codemark-core/src/git/context.rs detect_context \
  --tag feature:git --tag concept:resolution \
  --note "Finds the repo root, branch, and HEAD commit for the current directory" >/dev/null
mark crates/codemark-core/src/sync.rs build_auth_headers \
  --tag feature:sync --tag concept:authentication \
  --note "Adds the bearer token to requests sent to the Codetours server" >/dev/null

# ---------------------------------------------------------------------------
# Health states: simulate later edits so the gallery isn't all-green.
#   relax_query -> Drifted (body changed; the query still finds it)
#   build_auth_headers -> Broken (function removed; nothing left to match)
# ---------------------------------------------------------------------------

# edit_or_die <file> <perl-expr> <description>: apply an in-place edit and fail
# if it changed nothing (the upstream code moved on; update the expression).
edit_or_die() {
  local file="$1" expr="$2" what="$3" before
  before="$(git hash-object "$file")"
  perl -0pi -e "$expr" "$file"
  if [ "$(git hash-object "$file")" = "$before" ]; then
    echo "error: could not simulate '$what' in $file (code changed upstream? update seed-fixture.sh)" >&2
    exit 1
  fi
}

edit_or_die crates/codemark-core/src/query/relaxer.rs \
  's/(pub fn relax_query\([^)]*\)[^{]*\{\n)/$1    \/\/ Relaxation also tolerates renamed captures.\n    let query = query.trim();\n/' \
  "drift relax_query"
edit_or_die crates/codemark-core/src/sync.rs \
  's/\n(\/\/\/[^\n]*\n)*pub fn build_auth_headers\(.*?\n\}\n//s' \
  "remove build_auth_headers"

# No signing: a global commit.gpgsign would otherwise fail on the fixture identity.
git -c user.name="codemark fixture" -c user.email="fixture@codemark.invalid" \
  -c commit.gpgsign=false commit -q -m "fixture: simulate later edits (drifted + broken bookmarks)" -- \
  crates/codemark-core/src/query/relaxer.rs crates/codemark-core/src/sync.rs

echo "Seeded fixture at $DEST (${FIXTURE_REV:0:12})"
