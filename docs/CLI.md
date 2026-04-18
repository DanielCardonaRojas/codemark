# Command Reference

Full list of subcommands and flags for `codemark`.

## Global Options

```
codemark [--db <path>]... [--format <fmt>] [--verbose] <subcommand>

--db <path>       Database location (repeatable for multi-repo queries)
--format <fmt>    Output format: json (default), table, line, tv, markdown
--verbose         Enable debug-level logging to stderr
```

---

## Core Commands

### `add`
Create a bookmark from a file and range.
```bash
codemark add --file <path> --range <start:end> [--tag <tag>]... [--note <note>]
```
- `--range`: Line range (e.g., `42` or `42:67`) or byte range with `b` prefix (e.g., `b1024:1280`).
- `--hunk`: Derive range from a git diff hunk header.

### `add-from-snippet`
Create a bookmark by matching a snippet from stdin.
```bash
echo "code snippet" | codemark add-from-snippet --file <path>
```

### `resolve`
Find the current location of a bookmark.
```bash
codemark resolve <id>
```

### `show`
Display full details and resolution history.
```bash
codemark show <id>
```

### `list`
List bookmarks with filters.
```bash
codemark list [--tag <tag>] [--status <status>] [--file <path>]
```

### `open`
Open a bookmarked file in your configured editor.
```bash
codemark open <id>
```

---

## Search & Maintenance

### `search`
Search across notes, context, and tags.
```bash
codemark search "query"
codemark search --semantic "natural language query"
```

### `reindex`
Rebuild embeddings for semantic search.
```bash
codemark reindex [--force]
```

### `heal`
Batch-resolve all bookmarks and update their status (active/drifted/stale).
```bash
codemark heal [--auto-archive]
```

### `status`
Print a health summary of all bookmarks.

### `diff`
Show bookmarks affected by recent git changes.

### `gc`
Permanently remove old archived bookmarks.
```bash
codemark gc --older-than 30d
```

---

## Collections

Group bookmarks for specific investigations or tasks.

### `collection create <name>`
### `collection add <name> <id>...`
### `collection list`
### `collection show <name>`
### `collection reorder <name> <id>...`

---

## Advanced

### `annotate`
Add notes, context, or tags to an existing bookmark without re-parsing.
```bash
codemark annotate <id> --note "new note" --tag bug
```

### `preview`
Show a syntax-highlighted preview of bookmarked code.
```bash
codemark preview <id>
```

### `export` / `import`
Transfer bookmarks via JSON or CSV.

### `completions`
Generate shell completions for bash, zsh, fish, or powershell.
