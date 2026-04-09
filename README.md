# Codemark

[![crates.io](https://img.shields.io/crates/v/codemark)](https://crates.io/crates/codemark)
[![license](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

A structural bookmarking system for code. Instead of fragile file:line references, Codemark stores **[tree-sitter](https://tree-sitter.github.io/tree-sitter/)** queries that identify code by its AST shape. Bookmarks survive renames, refactors, and reformatting through layered resolution.

## Why

AI coding agents and developers revisit the same code across sessions. Line-number bookmarks break on any edit. Codemark bookmarks self-heal:

- **Exact match** — the AST query still finds the node
- **Relaxed match** — structure matches but names changed
- **Hash fallback** — content moved, found by normalized hash
- **Failed** — code was deleted; bookmark is marked stale, not silently wrong

## Install

**Homebrew (recommended):**

```bash
brew tap DanielCardonaRojas/codemark
brew install codemark
```

**Cargo:**

```bash
cargo install --path .
```

Requires Rust 1.75+. The binary is self-contained (SQLite is bundled).

## Quick start

```bash
# Bookmark a function by line number
codemark add --file src/auth.rs --range 42 --tag auth --note "Token validation entry point"

# Preview what would be bookmarked (dry run)
codemark add --file src/auth.rs --range 42:67 --dry-run

# Bookmark from a code snippet
echo 'fn validate_token' | codemark add-from-snippet --file src/auth.rs --tag auth

# Resolve — find where the code is now
codemark resolve a1b2

# List all bookmarks
codemark list

# Preview with bat (syntax highlighted)
codemark preview a1b2 | jq -r '"bat --highlight-line \(.data.line_range) \(.data.file_path)"' | sh
```

## Supported languages

| Language | Extensions |
|----------|-----------|
| Swift | `.swift` |
| Rust | `.rs` |
| TypeScript | `.ts`, `.tsx` |
| Python | `.py` |
| Go | `.go` |
| Java | `.java` |
| C# | `.cs` |
| Dart | `.dart` |

Language is auto-detected from file extension. Use `--lang` to override.

## Commands

### Creating bookmarks

```bash
# By line range (most common)
codemark add --file src/auth.rs --range 42        # single line
codemark add --file src/auth.rs --range 42:67     # line range

# By byte range (for tooling)
codemark add --file src/auth.rs --range b1024:1280

# From a git diff hunk
codemark add --file src/auth.rs --hunk "@@ -42,7 +42,9 @@"

# From a code snippet on stdin
echo 'func validateToken' | codemark add-from-snippet --file src/auth.rs
```

Options: `--tag <tag>` (repeatable), `--note "why this matters"`, `--created-by agent`, `--dry-run`

### Resolving bookmarks

```bash
codemark resolve a1b2                          # single bookmark by ID prefix
codemark resolve --tag auth                    # batch by tag
codemark resolve --lang rust --status active   # batch by language + status
```

### Previewing bookmarks

```bash
codemark preview a1b2                          # JSON: file path, line range, byte range, status
codemark preview a1b2 --at-commit abc123       # preview as it was at a specific commit
codemark preview a1b2 --at-creation            # preview as it was when created
codemark preview a1b2 --resolution-id <id>     # preview at a specific resolution
```

Preview outputs JSON for easy piping to your editor or viewer:

```bash
# View with [bat](https://github.com/sharkdp/bat) (syntax highlighted, with extension detection)
codemark preview a1b2 | jq -r '"\(.data.file_path)\n\(.data.line_range)"' | xargs -n2 sh -c 'bat --highlight-line "$2" -l "${1##*.}" "$1"' _

# Or simpler (no syntax highlighting):
codemark preview a1b2 | jq -r '"--highlight-line \(.data.line_range) \(.data.file_path)"' | xargs bat

# Open in vim/nvim at the bookmarked line
codemark preview a1b2 | jq -r '"\(.data.file_path) +\(.data.line_range | split(":")[0])"' | xargs nvim

# Open in other editors (file:line format)
codemark preview a1b2 | jq -r '"\(.data.file_path):\(.data.line_range | split(":")[0])"' | xargs code
codemark preview a1b2 | jq -r '"\(.data.file_path):\(.data.line_range | split(":")[0])"' | xargs hx
```

### Browsing and searching

```bash
codemark list                                  # table (TTY) or line format (piped)
codemark list --tag auth --lang swift          # filtered
codemark list --author agent                   # only agent-created bookmarks
codemark search "authentication"               # full-text search in notes/context
codemark search --semantic "auth functions"    # semantic search (vector embeddings)
codemark show a1b2                             # full details + resolution history
```

### Health management

```bash
codemark status                                # 42 active | 3 drifted | 1 stale | 0 archived
codemark validate                              # re-resolve all, update statuses
codemark validate --auto-archive               # also archive stale bookmarks
codemark diff --since HEAD~3                   # which bookmarks are affected by recent changes
codemark gc --older-than 30d                   # permanently delete old archived bookmarks
```

### Collections

```bash
codemark collection create bugfix-auth --description "Auth fix investigation"
codemark collection add bugfix-auth a1b2 f5e6
codemark collection resolve bugfix-auth        # batch-resolve the collection
codemark collection list                       # list all collections with counts
codemark collection show bugfix-auth           # list bookmarks in collection
codemark collection delete bugfix-auth         # delete collection (bookmarks kept)
```

### Cross-repo queries

Query across multiple repository databases:

```bash
codemark --db ~/repo-a/.codemark/codemark.db \
         --db ~/repo-b/.codemark/codemark.db \
         list --tag auth
```

Results include a `Source` column identifying which repo each bookmark came from.

### Export/Import

```bash
codemark export > bookmarks.json
codemark export --export-format csv > bookmarks.csv
codemark import bookmarks.json                 # skips duplicates
```

## Output modes

All commands output **JSON by default** (optimized for AI agents and scripting).

```bash
codemark list                    # JSON array of bookmarks
codemark list --format table     # Human-readable table
codemark list --format line      # Tab-separated (for fzf, tv, grep)
codemark list --format tv        # TV format with line numbers
```

| Command | Default | Available formats |
|---------|---------|-------------------|
| `list` | JSON | table, line, tv, custom template |
| `show` | JSON | table |
| `resolve` | JSON | table, line |
| `status` | JSON | table |
| `collection list` | JSON | table, line |
| `collection show` | JSON | table, line, tv |
| `preview` | JSON | JSON only |

## Integration

### [fzf](https://github.com/junegunn/fzf)

```bash
# Browse with live preview
codemark list --format tv | fzf --preview 'bat --highlight-line {3} $(codemark preview {1} | jq -r .data.file_path)'

# Or view the JSON preview data
codemark list --format tv | fzf --preview 'codemark preview {1} | jq'

# Open selected bookmark in nvim
codemark list --format tv | fzf | cut -f1 | while read id; do
  codemark preview "$id" | jq -r '"\(.data.file_path) +\(.data.line_range | split(":")[0])"' | xargs nvim
done
```

### Shell completions

```bash
# Zsh — write to any directory in your fpath, then restart shell
mkdir -p ~/.zsh/completions
codemark completions zsh > ~/.zsh/completions/_codemark
# Add to .zshrc: fpath=(~/.zsh/completions $fpath)

# Bash
mkdir -p ~/.local/share/bash-completion/completions
codemark completions bash > ~/.local/share/bash-completion/completions/codemark

# Fish
codemark completions fish > ~/.config/fish/completions/codemark.fish
```

### [television](https://github.com/alexpasmant/television)

```bash
# Install the channel
cp extras/codemark.toml ~/.config/television/cable/codemark.toml

# Use it
tv codemark

# Features:
# - Preview: shows file with bat at bookmarked line range
# - Ctrl+E: open in $EDITOR
# - Ctrl+R: re-resolve bookmark
# - Ctrl+D: show full details
```

### Claude Code

```bash
# Install the plugin
ln -s /path/to/codemark/extras/claude-code-plugin ~/.claude/plugins/codemark
```

This gives agents the `/codemark` skill for proactive bookmarking during exploration.

## Semantic search

Codemark supports semantic search using vector embeddings (powered by [sqlite-vec](https://github.com/asg017/sqlite-vec)). Bookmarks are automatically embedded when created, and you can search by meaning rather than exact keywords.

```bash
# Enable semantic search (creates embeddings for all bookmarks)
codemark reindex

# Search by meaning
codemark search --semantic "database connection initialization"
codemark search --semantic "error handling" --limit 10
```

Embeddings are stored using the [sqlite-vec](https://github.com/asg017/sqlite-vec) extension. For manual database inspection with the sqlite3 CLI, you need a version with loadable extension support:

```bash
# Install sqlite3 with extension support via Homebrew
brew install sqlite3

# Load the vec0 extension (download from https://github.com/asg017/sqlite-vec/releases)
/opt/homebrew/opt/sqlite/bin/sqlite3 .codemark/codemark.db
sqlite> .load ./vec0
sqlite> SELECT bookmark_id FROM bookmark_embeddings;
```

The first run of `codemark reindex` will download the ML model ([all-MiniLM-L6-v2](https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2), ~100MB) to `~/.cache/codemark/`.

## Configuration

Place `.codemark/config.toml` in your repository root:

```toml
[storage]
max_resolutions_per_bookmark = 20    # prune old resolution history

[health]
auto_archive_after_days = 7          # for validate --auto-archive
```

See `extras/config.example.toml` for all options.

## How it works

1. **Parse** the file with [tree-sitter](https://tree-sitter.github.io/tree-sitter/) to build an AST
2. **Find** the target node (smallest declaration covering the line/byte range)
3. **Generate** an S-expression query that uniquely identifies the node:
   ```scheme
   (class_declaration
     name: (type_identifier) @name0
     (#eq? @name0 "AuthService")
     (class_body
       (function_declaration
         name: (simple_identifier) @fn_name
         (#eq? @fn_name "validateToken")) @target))
   ```
4. **Store** the query + content hash + metadata in SQLite
5. **Resolve** later by running the query against the (potentially changed) file, falling through tiers: exact → relaxed → minimal → hash fallback

## Database

SQLite at `.codemark/codemark.db` relative to the git repo root. Auto-created on first use. Add `.codemark/` to `.gitignore` — each developer/agent maintains their own bookmarks.

See [database-schema.md](docs/database-schema.md) for the complete schema diagram and table documentation.

## Project structure

```
src/
├── cli/           # clap CLI definition, handlers, output formatting
├── config.rs      # .codemark/config.toml parsing
├── engine/        # bookmark models, resolution pipeline, health state machine, hashing
├── git/           # repo detection, HEAD capture, diff analysis
├── parser/        # tree-sitter language registry, parse cache
├── query/         # query generation, relaxation, matching
└── storage/       # SQLite database, migrations, CRUD repos
```

## Related projects

- [tree-sitter](https://tree-sitter.github.io/tree-sitter/) — Parsing library for code
- [sqlite-vec](https://github.com/asg017/sqlite-vec) — Vector search extension for SQLite
- [fzf](https://github.com/junegunn/fzf) — Command-line fuzzy finder
- [television](https://github.com/alexpasmant/television) — Modern, fuzzy selector
- [bat](https://github.com/sharkdp/bat) — Cat clone with syntax highlighting
- [git2](https://github.com/rust-lang/git2-rs) — Rust bindings to libgit2

## License

[MIT](LICENSE)
