# Codemark: Functional Requirements

## FR-1: Bookmark Creation

### FR-1.1: Create from byte range
**Input**: File path, byte range (start:end), language identifier, optional tags/note/context.
**Behavior**:
1. Parse the file using the appropriate tree-sitter grammar.
2. Find the smallest named AST node that spans the given byte range.
3. Generate a tree-sitter query that uniquely identifies this node within the file.
4. Capture the current HEAD commit hash for git context.
5. Compute a content hash of the matched node's text.
6. Store all data in SQLite and return the bookmark ID + generated query.

**Output**: Bookmark ID (UUID), generated query string, matched node type and range.

### FR-1.2: Create from snippet (stdin)
**Input**: Code snippet via stdin, language identifier, file path to match against, optional tags/note/context.
**Behavior**:
1. Parse the target file using the appropriate tree-sitter grammar.
2. Parse the snippet to determine its AST structure.
3. Find the node in the target file's AST that best matches the snippet's structure and content.
4. Generate a tree-sitter query for the matched node.
5. Capture HEAD commit hash and content hash as in FR-1.1.
6. Store and return as in FR-1.1.

**Matching strategy**: Exact text match first, then structural match with content similarity scoring. If multiple candidates match, prefer the one with highest text overlap.

**Output**: Same as FR-1.1.

### FR-1.3: Query generation
The generated tree-sitter query must:
- Use named node types (not anonymous nodes) for stability.
- Include enough structural context to be unique within the file (e.g., parent function name, class name).
- Capture the target node with `@target` for extraction.
- Be deterministic: same input produces same query.

**Query complexity tiers** (used for relaxed resolution):
1. **Exact**: Full structural path with field names and text predicates.
2. **Relaxed**: Node type + parent context, text predicates removed.
3. **Minimal**: Node type only, used as last resort before hash fallback.

### FR-1.4: Metadata capture
Every bookmark stores:
- **tags**: JSON array of user-supplied labels (e.g., `["auth-flow", "api-boundary"]`).
- **notes**: Free-text semantic annotation from the agent.
- **context**: What the agent was doing when it created the bookmark.
- **created_by**: Agent session identifier.
- **created_at**: ISO 8601 timestamp.

---

## FR-2: Bookmark Resolution

### FR-2.1: Single bookmark resolution
**Input**: Bookmark ID.
**Behavior**:
1. **Exact match**: Run the stored query against the current file. If exactly one node matches and its content hash matches, return it.
2. **Relaxed match**: If exact fails, generate a relaxed query (remove text predicates, loosen structure). If one match, return it.
3. **Hash fallback**: If query-based resolution fails, scan the file for a node whose content hash matches. If found, regenerate the query from the new location.
4. **Cross-file search** (optional, expensive): If the file itself has moved, search files with the same extension for hash matches.
5. **Failure**: If all methods fail, mark the bookmark as `stale` and record the failure in resolution history.

**Output**: File path, line number, column, byte range, matched text preview, resolution method used.

**Side effects**: Updates the bookmark's `status`, `resolution_method`, `last_resolved_at`. If the bookmark moved, updates `file_path`. Records an entry in the `resolutions` table.

### FR-2.2: Batch resolution
**Input**: Filter criteria (tag, status, file path).
**Behavior**: Resolve all matching bookmarks. Return results as a list.
**Optimization**: Group bookmarks by file to avoid re-parsing the same file multiple times.

### FR-2.3: Resolution performance
- Single bookmark resolution: < 50ms on a file under 10K lines.
- Batch resolution for 100 bookmarks across 20 files: < 2 seconds.
- Tree-sitter parsing results should be cached per file within a single CLI invocation.

---

## FR-3: Health Tracking

### FR-3.1: Bookmark statuses
| Status     | Meaning                                                        |
|------------|----------------------------------------------------------------|
| `active`   | Last resolution succeeded via exact or relaxed match.          |
| `drifted`  | Resolved via hash fallback or relaxed match; query may be stale. |
| `stale`    | Failed to resolve entirely. Code may have been deleted.         |
| `archived` | Manually or automatically archived. Excluded from default queries. |

### FR-3.2: Validation
**Input**: Optional file path filter, optional `--auto-archive` flag.
**Behavior**:
1. Resolve all matching bookmarks (default: all non-archived).
2. Update each bookmark's status based on resolution outcome.
3. If `--auto-archive` is set, archive bookmarks that have been `stale` for longer than the configurable grace period (default: 7 days).

**Output**: Summary of status changes (e.g., "3 active → drifted, 1 drifted → stale, 2 stale → archived").

### FR-3.3: Status summary
**Input**: None.
**Output**: Count of bookmarks by status (e.g., "42 active, 3 drifted, 1 stale, 12 archived").

---

## FR-4: Search and Query

### FR-4.1: List bookmarks
**Input**: Optional filters — tag, status, file path.
**Output**: Table of matching bookmarks with: ID (short), file path, status, tags, note (truncated), last resolved timestamp.

### FR-4.2: Show bookmark details
**Input**: Bookmark ID (full or prefix).
**Output**: All bookmark fields, plus resolution history (last N entries).

### FR-4.3: Full-text search
**Input**: Search string.
**Behavior**: Search across `notes` and `context` fields using SQLite FTS or LIKE matching.
**Output**: Matching bookmarks with relevance-ranked results.

### FR-4.4: Cross-repository search
**Input**: Multiple `--db` paths pointing to databases from different repositories.
**Behavior**: Open each database, run the query against all of them, merge results. Each result is annotated with a `source` label derived from the database path (typically the repo directory name).
**Output**: Unified result set with a `source` column, sorted by relevance or creation date.

**Use case**: A developer working across `service-auth` and `service-api` repos can search for all bookmarks tagged `auth` to understand the auth boundary across both services:
```bash
codemark --db ~/service-auth/.codemark/codemark.db \
         --db ~/service-api/.codemark/codemark.db \
         search --tag auth
```

**Constraints**:
- Read-only across secondary databases — no status updates, no resolution history writes.
- Resolution against secondary databases requires the source files to be accessible from the current working directory (paths are relative to each repo root).
- Bookmark ID prefixes must be unambiguous across all queried databases; if ambiguous, the `source` qualifier is required.

---

## FR-5: Git Integration

### FR-5.1: Git context capture
On bookmark creation, capture:
- The current HEAD commit (`commit_hash`). This records *when* the bookmark was created relative to repo history.

Blame data (per-line author, commit, message) is **not stored** in the bookmark. A bookmarked node can span lines from many commits and authors, making any single blame entry misleading. Instead, blame is queried on demand via `git blame` when needed during resolution or display.

### FR-5.2: Diff impact analysis
**Input**: Optional `--since <commit>` (defaults to last validation run).
**Behavior**:
1. Get the list of files changed since the given commit.
2. Filter bookmarks to those referencing changed files.
3. Resolve those bookmarks and report status changes.

**Output**: List of affected bookmarks with before/after status.

---

## FR-6: Maintenance

### FR-6.1: Garbage collection
**Input**: Optional `--older-than <duration>` (default: 30 days).
**Behavior**: Permanently delete `archived` bookmarks (and their resolution history) older than the threshold.

### FR-6.2: Export
**Input**: Format flag (`json` or `csv`).
**Output**: All bookmarks (or filtered set) in the specified format, written to stdout.

### FR-6.3: Import
**Input**: File path to a JSON export.
**Behavior**: Import bookmarks, skipping duplicates (by ID). Validate query syntax before import.

---

## FR-7: Output Modes

All listing commands support multiple output modes to serve both human and machine consumers:

### FR-7.1: Table output (default for TTY)
Human-readable table with columns. Automatically selected when stdout is a terminal.

### FR-7.2: Line output (`--format line`)
One bookmark per line, tab-separated fields. Designed for piping into fzf, television, grep, awk, and shell scripts. This is the **primary integration surface** for interactive tooling.

**Format**: `<id>\t<file>:<line>\t<status>\t<tags>\t<note_truncated>`

Example:
```
a1b2c3d4	src/auth/middleware.swift:42	active	auth,middleware	JWT validation entry point
f5e6d7c8	src/api/router.swift:118	drifted	api,routing	Main request dispatcher
```

### FR-7.3: JSON output (`--json`)
Machine-readable JSON. Primary interface for agent consumption.

JSON output includes:
- `success`: boolean
- `data`: command-specific payload
- `errors`: array of error objects (if any)
- `metadata`: timing, bookmark counts, resolution statistics

### FR-7.4: Custom format (`--format <template>`)
User-defined output template using field placeholders: `{id}`, `{file}`, `{line}`, `{col}`, `{status}`, `{tags}`, `{note}`, `{query}`, `{node_type}`.

Example:
```bash
codemark list --format '{file}:{line}  # {note}'
```

### FR-7.5: Auto-detection
When stdout is a TTY, use table output. When piped, use line output. `--json` and `--format` override auto-detection.

---

## FR-8: Preview and Interactive Browsing

### FR-8.1: Preview command
**Input**: Bookmark ID (or line-format string from `--format line` output).
**Behavior**:
1. Resolve the bookmark to its current location.
2. Read the source file around the resolved location.
3. Display the code with syntax highlighting (via tree-sitter), with the bookmarked node visually highlighted.
4. Show bookmark metadata (tags, note, status, resolution method) as a header.

**Output**: Syntax-highlighted code context suitable for terminal display or use as a preview pane in fzf/television.

**Flags**:
| Flag        | Default | Description                                         |
|-------------|---------|-----------------------------------------------------|
| `--context` | 10      | Lines of context above and below the bookmarked node |
| `--no-color`| false   | Disable syntax highlighting (for non-terminal use)  |
| `--metadata`| true    | Show bookmark metadata header                       |

### FR-8.2: fzf integration
Codemark output pipes directly into fzf for interactive selection:

```bash
# Browse and select a bookmark, then open in editor
codemark list --format line | fzf --preview 'codemark preview {1}' | cut -f2 | xargs $EDITOR

# Search by tag, pick interactively
codemark list --tag auth --format line | fzf --preview 'codemark preview {1}'
```

### FR-8.3: Television integration
Codemark ships a television channel file (`tv-channel-bookmarks.toml`) for drop-in integration:

```bash
# After installing the channel:
tv bookmarks

# Ad-hoc (no channel file needed):
tv --source-command 'codemark list --format line' \
   --preview-command 'codemark preview {1}' \
   --no-remote
```

The shipped channel file supports:
- Fuzzy search across bookmark notes, tags, and file paths.
- Live preview of resolved code with syntax highlighting.
- Custom keybindings: Enter to open in editor, Ctrl-R to re-resolve, Ctrl-D to show full details.

### FR-8.4: Collection browsing
Users can browse collections interactively:

```bash
# Two-step: pick a collection, then browse its bookmarks
codemark collection list | fzf | xargs codemark collection show | fzf --preview 'codemark preview {1}'

# Direct: browse a known collection
codemark list --collection bugfix-auth | fzf --preview 'codemark preview {1}'
```

### FR-8.5: Query preview
**Input**: Bookmark ID.
**Behavior**: Display the stored tree-sitter query alongside the code it matches, showing the structural relationship. Useful for debugging bookmark drift and understanding how Codemark identifies code.

```bash
codemark preview --show-query a1b2c3d4
```

Output shows the query, the matched AST node, and the resolved code side by side.

---

## FR-9: Collections

Collections are named groups of bookmarks. A bookmark can belong to zero or more collections. Collections organize bookmarks by purpose (bugfix, feature, investigation) rather than by code attribute (tags, file path).

### FR-9.1: Create collection
**Input**: Collection name (slug), optional description.
**Behavior**: Create a new empty collection. Name must be unique, lowercase alphanumeric plus hyphens.
**Output**: Collection ID and name.

### FR-9.2: Delete collection
**Input**: Collection name.
**Behavior**: Delete the collection and all membership records. **Bookmarks are never deleted.**
**Output**: Confirmation with count of bookmarks that were in the collection.

### FR-9.3: Add bookmarks to collection
**Input**: Collection name, one or more bookmark IDs.
**Behavior**: Add the bookmarks to the collection. Silently skip bookmarks already in the collection. Create the collection if it doesn't exist (convenience shorthand).
**Output**: Count of bookmarks added.

### FR-9.4: Remove bookmarks from collection
**Input**: Collection name, one or more bookmark IDs.
**Behavior**: Remove the membership records. Bookmarks themselves are unchanged.
**Output**: Count of bookmarks removed.

### FR-9.5: List collections
**Input**: None (or optional `--bookmark <id>` to list collections a bookmark belongs to).
**Output**: Table of collections with: name, description, bookmark count, created date.

Pipe-friendly line format: `<name>\t<count>\t<description>`

### FR-9.6: Show collection contents
**Input**: Collection name.
**Output**: List of bookmarks in the collection (same format as `codemark list`), supporting all the same output modes and filters.

### FR-9.7: Resolve collection
**Input**: Collection name.
**Behavior**: Batch-resolve all bookmarks in the collection. Equivalent to `codemark resolve --collection <name>`.
**Output**: Resolution results for each bookmark, grouped by file.

### FR-9.8: Collection filter on existing commands
The `--collection <name>` flag is supported on:
- `codemark list` — list only bookmarks in the collection
- `codemark resolve` — resolve only bookmarks in the collection
- `codemark validate` — validate only bookmarks in the collection
- `codemark search` — search only within bookmarks in the collection
