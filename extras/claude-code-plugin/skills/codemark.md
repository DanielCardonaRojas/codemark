---
name: codemark
description: >
  Manage structural code bookmarks that survive refactoring. Use when the user says
  "remember this", "bookmark this", "save this location", "load my bookmarks",
  "what do I have bookmarked", "mark this code", "codemark", or when starting a new
  session and needing to reload context from a previous session. Also use proactively
  when you discover critical code during exploration — entry points, boundaries,
  configuration, error handling — that you'd want to remember if starting over.
---

# Codemark — Structural Code Bookmarking

You have access to `codemark`, a CLI tool that creates **structural bookmarks** for code using tree-sitter AST queries. Unlike file:line references, these bookmarks survive renames, refactors, and reformatting.

## When to use codemark

- **Starting a session**: Load bookmarks from a previous session to restore context
- **Exploring code**: When you discover something critical, bookmark it with context about *why* it matters
- **During work**: Bookmark code you'll need to reference later — especially cross-file relationships
- **Ending a session**: Validate bookmarks so the next session has accurate references
- **Checking impact**: Use `diff` to see which bookmarks are affected by recent changes

**Proactive bookmarking**: When you recognize code that is critical to the current task — entry points, auth boundaries, error handling paths, configuration — bookmark it immediately with a note explaining its significance and relationship to the work. Don't bookmark everything you read; bookmark what you'd want to know if starting over tomorrow.

## Key concepts

- **Bookmarks** store a tree-sitter query that identifies code by AST structure, not line numbers
- **Resolution** re-finds bookmarked code even after edits (exact → relaxed → hash fallback)
- **Collections** group related bookmarks (one per feature, bugfix, or investigation)
- **Status**: active (healthy), drifted (found but moved), stale (lost), archived (cleaned up)
- **Author**: bookmarks track who created them (`--created-by agent` vs default `user`)

## Quick reference

### Load context (start of session)

```bash
# Load all active bookmarks as JSON
codemark resolve --status active --json

# Load a specific collection
codemark collection resolve <name> --json

# Load bookmarks for a specific tag or language
codemark resolve --tag auth --json
codemark list --lang rust --json

# Load only agent-created bookmarks
codemark list --author agent --json

# Search by note text
codemark search "authentication" --json
```

### Bookmark code (during session)

Always use `--dry-run` first to verify the target is correct:

```bash
# Preview what line 42 would bookmark
codemark add --file <path> --range 42 --dry-run

# Bookmark a line range (language auto-detected from extension)
codemark add --file src/auth.rs --range 42:67 \
  --tag auth --note "Token validation entry point — all auth flows start here" \
  --created-by agent --json

# Bookmark from a code snippet (reads from stdin)
echo 'fn validate_token' | codemark add-from-snippet \
  --file src/auth.rs --tag auth --note "Token validator" \
  --created-by agent --json

# Bookmark from a git diff hunk
codemark add --file src/auth.rs --hunk "@@ -42,7 +42,9 @@" \
  --tag auth --note "Changed in this PR" --created-by agent --json
```

**Range formats:**
- `42` — single line
- `42:67` — line range (inclusive)
- `b1024:1280` — byte range (for precise targeting)

**Always pass `--created-by agent`** so your bookmarks can be distinguished from user-created ones.

### Check what changed

```bash
# Which bookmarks are affected by recent commits?
codemark diff --since HEAD~3 --json

# Which bookmarks are affected since a specific commit?
codemark diff --since abc123 --json
```

### Organize with collections

Collections are **ordered** — bookmarks maintain their position, which is useful for representing call paths, execution sequences, or reading order.

```bash
# Create a collection for your current task
codemark collection create feature-rate-limiting \
  --description "Rate limiting feature for API client"

# Add bookmarks in order (order is preserved)
codemark collection add feature-rate-limiting <id1> <id2> <id3>

# Insert at a specific position (0-indexed, shifts existing items)
codemark collection add feature-rate-limiting --at 1 <new_id>

# Reorder bookmarks (sets order from argument order)
codemark collection reorder feature-rate-limiting <id3> <id1> <id2>

# List all collections
codemark collection list --json

# View bookmarks in collection order
codemark collection show feature-rate-limiting --json

# Batch-resolve a collection (returns results in order)
codemark collection resolve feature-rate-limiting --json
```

**Use ordered collections for code paths**: When tracing a call chain like `HTTP handler → auth middleware → token validation → database query`, add bookmarks in execution order. This gives future sessions a sequential walkthrough of how data flows through the system.

### Remove bookmarks

```bash
codemark remove <id>               # remove one
codemark remove <id1> <id2> <id3>  # remove multiple
```

### Check health (end of session)

```bash
# Validate all bookmarks — updates statuses
codemark validate

# Quick health summary
codemark status

# Auto-archive bookmarks stale for >7 days
codemark validate --auto-archive
```

### Inspect a bookmark

```bash
# Full details including query and resolution history
codemark show <id>

# Syntax-highlighted code preview
codemark preview <id>

# Preview with the tree-sitter query shown
codemark preview <id> --show-query
```

## Filtering

Most list/search commands support these filters:

| Flag | Purpose |
|------|---------|
| `--tag <tag>` | Filter by tag |
| `--lang <language>` | Filter by language (swift, rust, typescript, python) |
| `--author <who>` | Filter by creator (user, agent) |
| `--status <status>` | Filter by status (active, drifted, stale, archived) |
| `--file <path>` | Filter by file path |
| `--collection <name>` | Filter by collection |

## Output modes

- Default (TTY): human-readable tables
- Piped: tab-separated lines for fzf/grep
- `--json`: structured JSON envelope `{success, data, error}` — **always use this for programmatic access**
- `--format '{file}:{line} # {note}'`: custom templates

## Best practices for agents

1. **Always use `--json`** when reading bookmark data programmatically
2. **Always use `--created-by agent`** when creating bookmarks
3. **Use `--dry-run`** before `add` to verify you're targeting the right node
4. **Write notes that explain *why*, not *what*** — "all auth flows start here" beats "auth function"
5. **Tag consistently** — use lowercase hyphenated tags like `auth-flow`, `api-boundary`
6. **Group related bookmarks** into collections named after the task
7. **Search before bookmarking** — avoid duplicating existing bookmarks:
   ```bash
   codemark search "auth" --json
   ```
8. **Check diff after changes** — `codemark diff --since HEAD~1` shows impact on bookmarks
9. **Validate at session end** to keep the database healthy

## Supported languages

Swift (`.swift`), Rust (`.rs`), TypeScript (`.ts`, `.tsx`), Python (`.py`).

Language is auto-detected from file extension — `--lang` is optional.

## ID prefixes

Bookmark IDs are UUIDs but all commands accept **unambiguous prefixes** (minimum 4 chars):
```bash
codemark show a1b2
codemark resolve a1b2
codemark preview a1b2
```
