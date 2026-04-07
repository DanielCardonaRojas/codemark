# Codemark: CLI Specification

## Global Options

```
codemark [--db <path>]... [--format <fmt>] [--verbose] <subcommand>

--db <path>       Database location (default: .codemark/codemark.db); repeatable for cross-repo queries
--format <fmt>    Output format: json (default), table, line, or tv
--verbose         Enable debug-level logging to stderr
--help            Show help
--version         Show version
```

### Multi-Database Queries

The `--db` flag can be specified multiple times to query across databases from different repositories:

```bash
# Search across two repos
codemark --db ~/repo-a/.codemark/codemark.db --db ~/repo-b/.codemark/codemark.db list --tag auth

# Search for patterns across all known repos
codemark --db ~/projects/*/.codemark/codemark.db search "auth"
```

**Read commands** support multiple databases: `list`, `search`, `resolve`, `show`, `status`, `export`, `collection list`, `collection show`.

**Write commands** use only the first `--db` path (or the auto-detected repo): `add`, `add-from-snippet`, `validate`, `gc`, `import`, `collection create/delete/add/remove`.

When querying multiple databases, results include a `source` field identifying which database (repo) each bookmark came from. In line output this is prepended as the first tab-separated field. In JSON output it appears as `"source": "<repo-name>"` on each item.

## Subcommands

---

### `codemark add`

Create a bookmark from a file and byte range.

```
codemark add --file <path> --range <start:end> --lang <language>
             [--tag <tag>]... [--note "annotation"] [--context "what agent was doing"]
```

| Flag        | Required | Description                                |
|-------------|----------|--------------------------------------------|
| `--file`    | Yes      | Path to the file (relative or absolute)    |
| `--range`   | Yes      | Byte range `start:end` identifying the code region |
| `--lang`    | Yes      | Language identifier (swift, typescript, rust, python) |
| `--tag`     | No       | Tag label; repeatable for multiple tags    |
| `--note`    | No       | Semantic annotation                        |
| `--context` | No       | Agent context at time of bookmarking       |

**Exit codes**: 0 = success, 1 = parse/match failure, 2 = file not found.

**Example**:
```bash
codemark add --file src/auth/middleware.swift --range 1024:1280 --lang swift \
  --tag auth --tag middleware --note "JWT validation entry point"
```

**JSON output**:
```json
{
  "success": true,
  "data": {
    "id": "a1b2c3d4-...",
    "query": "(function_declaration name: (identifier) @name (#eq? @name \"validateToken\")) @target",
    "node_type": "function_declaration",
    "range": { "start": { "line": 42, "col": 4 }, "end": { "line": 67, "col": 5 } },
    "content_hash": "sha256:abcdef..."
  }
}
```

---

### `codemark add-from-snippet`

Create a bookmark by matching a code snippet against a file. Reads the snippet from stdin.

```
codemark add-from-snippet --lang <language> --file <path>
             [--tag <tag>]... [--note "annotation"] [--context "..."]
```

| Flag        | Required | Description                                |
|-------------|----------|--------------------------------------------|
| `--lang`    | Yes      | Language identifier                        |
| `--file`    | Yes      | File to search for the snippet in          |
| `--tag`     | No       | Tag label; repeatable                      |
| `--note`    | No       | Semantic annotation                        |
| `--context` | No       | Agent context                              |

**Example**:
```bash
echo 'func validateToken(_ token: String) -> Bool {' | \
  codemark add-from-snippet --lang swift --file src/auth/middleware.swift \
  --tag auth --note "JWT validation entry point"
```

---

### `codemark resolve`

Resolve a bookmark to its current location.

```
codemark resolve <bookmark-id>
codemark resolve --tag <tag> [--status <status>] [--file <path>]
```

**Single bookmark**:
```bash
codemark resolve a1b2c3d4
```

**Batch by filter**:
```bash
codemark resolve --tag auth --status active
```

| Flag        | Required | Description                                |
|-------------|----------|--------------------------------------------|
| `--tag`     | No       | Filter by tag                              |
| `--status`  | No       | Filter by status (default: active,drifted) |
| `--file`    | No       | Filter by file path                        |

**JSON output (single)**:
```json
{
  "success": true,
  "data": {
    "id": "a1b2c3d4-...",
    "file": "src/auth/middleware.swift",
    "line": 42,
    "column": 4,
    "byte_range": "1024:1280",
    "method": "exact",
    "status": "active",
    "preview": "func validateToken(_ token: String) -> Bool {",
    "note": "JWT validation entry point",
    "tags": ["auth", "middleware"]
  }
}
```

---

### `codemark show`

Display full details of a bookmark.

```
codemark show <bookmark-id>
```

Accepts full UUID or unambiguous prefix. Displays all fields including resolution history.

---

### `codemark validate`

Run resolution on all (or filtered) bookmarks and update statuses.

```
codemark validate [--file <path>] [--auto-archive] [--archive-after <days>]
```

| Flag               | Required | Default | Description                           |
|--------------------|----------|---------|---------------------------------------|
| `--file`           | No       | all     | Validate only bookmarks for this file |
| `--auto-archive`   | No       | false   | Archive bookmarks stale beyond grace  |
| `--archive-after`  | No       | 7       | Days before stale → archived          |

---

### `codemark status`

Print a summary of bookmark health.

```
codemark status
```

**Output**:
```
42 active  |  3 drifted  |  1 stale  |  12 archived
Last validated: 2026-03-23T14:30:00Z
```

---

### `codemark list`

List bookmarks with optional filters.

```
codemark list [--tag <tag>] [--status <status>] [--file <path>] [--limit <n>]
              [--format <fmt>]
```

| Flag        | Required | Description                                |
|-------------|----------|--------------------------------------------|
| `--tag`     | No       | Filter by tag                              |
| `--status`  | No       | Filter by status (default: active,drifted) |
| `--file`    | No       | Filter by file path                        |
| `--limit`   | No       | Maximum results to return                  |
| `--format`  | No       | Output format (see Global Options)         |

**JSON output** (default): Array of bookmark objects with all fields.

**Table output**: Human-readable table with columns: ID (8-char prefix), File, Status, Tags, Note (truncated), Last Resolved.

**Line output**: Tab-separated format, one bookmark per line:
```
<id>\t<file>:<line>\t<status>\t<tags>\t<note>
```

**Integration examples**:
```bash
# fzf with live preview (use --format line for compatibility)
codemark list --format line | fzf --preview 'codemark preview {1}'

# television ad-hoc
tv --source-command 'codemark list --format line' --preview-command 'codemark preview {1}'

# jq processing
codemark list | jq '.[] | select(.tags[]? == "auth")'

# custom format for editor integration
codemark list --format '{file}:{line}:{col}' | head -5
```

---

### `codemark preview`

Show a syntax-highlighted preview of a bookmark's resolved code. Designed to be used as a preview command in fzf, television, or standalone.

```
codemark preview <bookmark-id-or-line>
                 [--context <lines>] [--no-color] [--no-metadata] [--show-query]
```

| Flag            | Required | Default | Description                                         |
|-----------------|----------|---------|-----------------------------------------------------|
| `--context`     | No       | 10      | Lines of context above/below the bookmarked node    |
| `--no-color`    | No       | false   | Disable syntax highlighting                         |
| `--no-metadata` | No       | false   | Omit the metadata header                            |
| `--show-query`  | No       | false   | Show the stored tree-sitter query alongside the code |

The positional argument accepts either a bookmark ID or an entire line-format string (from `codemark list` piped output). When given a line-format string, it extracts the ID from the first tab-separated field.

**Example**:
```bash
# Standalone preview
codemark preview a1b2c3d4

# As fzf preview command (piped from list)
codemark list | fzf --preview 'codemark preview {1}'

# Show the tree-sitter query that identifies this code
codemark preview --show-query a1b2c3d4
```

**Output** (TTY):
```
─── a1b2c3d4 ─── src/auth/middleware.swift:42 ─── active ───
Tags: auth, middleware
Note: JWT validation entry point
Resolution: exact match
────────────────────────────────────────────────────────────
  39 │
  40 │     // MARK: - Token Validation
  41 │
▶ 42 │     func validateToken(_ token: String) -> Bool {
▶ 43 │         guard let claims = decode(token) else {
▶ 44 │             return false
▶ 45 │         }
▶ 46 │         return claims.expiry > Date()
▶ 47 │     }
  48 │
  49 │     func decode(_ token: String) -> Claims? {
  50 │
```

---

### `codemark search`

Full-text search across notes and context.

```
codemark search --note "auth"
codemark search --context "refactoring"
codemark search "auth"    # searches both note and context
codemark search "how are tokens validated" --semantic    # natural language search
codemark search "network error handling" --semantic --limit 5
codemark search "database" --semantic --provider openai    # requires OPENAI_API_KEY
```

| Flag        | Required | Description                                |
|-------------|----------|--------------------------------------------|
| `--semantic`| No       | Enable semantic search using embeddings    |
| `--provider`| No       | Embedding provider: `local` (default) or `openai` |
| `--limit`   | No       | Maximum results to return (default: 10)    |

**Semantic search** (Phase 2a) requires embeddings to be generated via `codemark reindex`. See [Semantic Search](./10-semantic-search.md) for details.

---

### `codemark collection`

Manage named groups of bookmarks.

---

#### `codemark collection create`

```
codemark collection create <name> [--description "..."]
```

| Arg/Flag        | Required | Description                                     |
|-----------------|----------|-------------------------------------------------|
| `<name>`        | Yes      | Slug name (lowercase, alphanumeric, hyphens)    |
| `--description` | No       | Human-readable purpose                          |

**Example**:
```bash
codemark collection create bugfix-auth --description "Token validation regression fix"
```

---

#### `codemark collection delete`

```
codemark collection delete <name>
```

Deletes the collection and all membership records. **Bookmarks are never deleted.**

---

#### `codemark collection add`

```
codemark collection add <name> <bookmark-id>...
```

Add one or more bookmarks to a collection. Creates the collection if it doesn't exist.

**Example**:
```bash
codemark collection add bugfix-auth a1b2c3d4 f5e6d7c8
```

---

#### `codemark collection remove`

```
codemark collection remove <name> <bookmark-id>...
```

Remove bookmarks from a collection. Bookmarks themselves are unchanged.

---

#### `codemark collection list`

```
codemark collection list [--bookmark <id>]
```

List all collections (or collections containing a specific bookmark).

**JSON output** (default): Array of collection objects.

**Table output**: Columns: Name, Description, Bookmark Count, Created.

**Line output**: Tab-separated format `<name>\t<count>\t<description>`

---

#### `codemark collection show`

```
codemark collection show <name> [--format <fmt>]
```

List bookmarks in a collection. Supports all output formats from `codemark list`.

---

#### `codemark collection resolve`

```
codemark collection resolve <name>
```

Batch-resolve all bookmarks in a collection.

---

### `--collection` filter

The following commands accept `--collection <name>` to scope to a collection's bookmarks:

```bash
codemark list --collection bugfix-auth
codemark resolve --collection bugfix-auth
codemark validate --collection bugfix-auth
codemark search "auth" --collection bugfix-auth
```

---

### `codemark diff`

Show bookmarks affected by recent changes.

```
codemark diff [--since <commit>]
```

Defaults to changes since the last recorded validation commit.

---

### `codemark reindex`

Generate embeddings for semantic search.

```
codemark reindex [--force] [--batch-size <n>]
```

| Flag         | Required | Default | Description                                   |
|--------------|----------|---------|-----------------------------------------------|
| `--force`    | No       | false   | Regenerate all embeddings (not just missing)  |
| `--batch-size`| No      | 32      | Number of embeddings to generate per batch    |

**Example**:
```bash
# Generate embeddings for bookmarks that don't have them
codemark reindex

# Regenerate all embeddings (e.g., after changing model)
codemark reindex --force
```

**Exit codes**: 0 = success, 1 = embedding model not found, 2 = database error.

---

### `codemark gc`

Remove old archived bookmarks.

```
codemark gc [--older-than <duration>] [--dry-run]
```

Duration format: `30d`, `2w`, `6m`. Default: `30d`.

---

### `codemark export`

```
codemark export [--format json|csv] [--tag <tag>] [--status <status>]
```

Writes to stdout.

---

### `codemark import`

```
codemark import <file>
```

Reads a JSON export file. Skips duplicate IDs. Validates queries before import.

---

## Exit Codes

| Code | Meaning                                        |
|------|------------------------------------------------|
| 0    | Success                                        |
| 1    | Operation failed (parse error, no match, etc.) |
| 2    | Input error (file not found, invalid args)     |
| 3    | Database error                                 |

## Tool Integration

### fzf

Codemark's pipe-friendly output is designed for direct use with fzf:

```bash
# Browse all bookmarks with live preview
codemark list | fzf --preview 'codemark preview {1}' --preview-window=right:60%

# Search by note text, pick interactively, open in editor
codemark search "auth" | fzf --preview 'codemark preview {1}' | cut -f2 | xargs $EDITOR

# Multi-select bookmarks to resolve
codemark list --tag api | fzf -m --preview 'codemark preview {1}' | cut -f1 | xargs codemark resolve

# Semantic search via fzf prompt (Phase 2a)
QUERY=$(fzf --prompt="Search by meaning> " --print-query | head -1)
codemark search --semantic "$QUERY" --format line | fzf --preview 'codemark preview {1}'
```

### Television

Codemark ships a television channel file at `extras/tv-channel-bookmarks.toml`. The channel supports two search modes:

**Default mode**: Fuzzy filter over full bookmark list (instant, in-memory)
**Semantic mode** (Phase 2a): Press `Ctrl-/` to open natural language search prompt

```toml
[metadata]
name = "bookmarks"
description = "Codemark structural bookmarks"
requirements = ["codemark"]

[source]
command = "codemark list --format line"
display = "{split:\\t:1}  {split:\\t:3}  {split:\\t:4}"
output = "{split:\\t:0}"

[preview]
command = "codemark preview {split:\\t:0}"

[ui.preview_panel]
size = 55

[keybindings]
shortcut = "F5"
ctrl-e = "actions:edit"
ctrl-r = "actions:resolve"
ctrl-d = "actions:details"

[actions.edit]
description = "Open in $EDITOR"
command = "codemark resolve {split:\\t:0} --format '{file}:{line}' | xargs $EDITOR"
mode = "fork"

[actions.resolve]
description = "Re-resolve bookmark"
command = "codemark resolve {split:\\t:0}"
mode = "execute"

[actions.details]
description = "Show full details"
command = "codemark show {split:\\t:0}"
mode = "execute"
```

Install via: `cp extras/tv-channel-bookmarks.toml ~/.config/television/cable/bookmarks.toml`

Then: `tv bookmarks`

### Shell aliases (suggested)

```bash
# Quick bookmark browser
alias cb='codemark list --format line | fzf --preview "codemark preview {1}" --preview-window=right:60%'

# Open bookmarked code in editor
alias cbo='codemark list --format line | fzf --preview "codemark preview {1}" | cut -f1 | xargs -I{} codemark resolve {} --format "{file}:{line}" | xargs $EDITOR'
```

## Environment Variables

| Variable            | Description                                |
|---------------------|--------------------------------------------|
| `CODEMARK_DB`       | Override database path                     |
| `CODEMARK_LOG`      | Log level (error, warn, info, debug, trace)|
| `NO_COLOR`          | Disable colored output                     |
