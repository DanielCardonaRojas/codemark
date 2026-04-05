# Codemark

A structural bookmarking system for code. Instead of fragile file:line references, Codemark stores **tree-sitter queries** that identify code by its AST shape. Bookmarks survive renames, refactors, and reformatting through layered resolution.

## Why

AI coding agents and developers revisit the same code across sessions. Line-number bookmarks break on any edit. Codemark bookmarks self-heal:

- **Exact match** — the AST query still finds the node
- **Relaxed match** — structure matches but names changed
- **Hash fallback** — content moved, found by normalized hash
- **Failed** — code was deleted; bookmark is marked stale, not silently wrong

## Install

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

# Browse interactively with fzf
codemark list | fzf --preview 'codemark preview {1}'
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
echo 'func validateToken' | codemark add-from-snippet --file src/auth.swift
```

Options: `--tag <tag>` (repeatable), `--note "why this matters"`, `--created-by agent`, `--dry-run`

### Resolving bookmarks

```bash
codemark resolve a1b2                          # single bookmark by ID prefix
codemark resolve --tag auth                    # batch by tag
codemark resolve --lang rust --status active   # batch by language + status
```

### Browsing and searching

```bash
codemark list                                  # table (TTY) or line format (piped)
codemark list --tag auth --lang swift          # filtered
codemark list --author agent                   # only agent-created bookmarks
codemark search "authentication"               # full-text search in notes/context
codemark show a1b2                             # full details + resolution history
codemark preview a1b2                          # syntax-highlighted code with context
codemark preview a1b2 --show-query             # also show the tree-sitter query
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

| Mode | When | Format |
|------|------|--------|
| Table | TTY (default) | Human-readable columns |
| Line | Piped (default) | `id\tfile\tstatus\ttags\tnote` — for fzf/grep/awk |
| JSON | `--json` | `{success, data, error}` envelope |
| Custom | `--format '{file}:{line}'` | Template with `{id}`, `{file}`, `{status}`, `{tags}`, `{note}`, `{query}` |

## Integration

### fzf

```bash
# Browse with live preview
codemark list | fzf --preview 'codemark preview {1}' --preview-window=right:60%

# Open in editor
codemark list | fzf --preview 'codemark preview {1}' | cut -f2 | xargs $EDITOR
```

### Shell completions

```bash
# Zsh — write to any directory in your fpath, then restart shell
# Check your fpath with: echo $fpath
mkdir -p ~/.zsh/completions
codemark completions zsh > ~/.zsh/completions/_codemark
# Ensure this is in your .zshrc (before compinit):
#   fpath=(~/.zsh/completions $fpath)
#   autoload -Uz compinit && compinit

# Bash
mkdir -p ~/.local/share/bash-completion/completions
codemark completions bash > ~/.local/share/bash-completion/completions/codemark

# Fish
codemark completions fish > ~/.config/fish/completions/codemark.fish
```

### Television

```bash
# Install the channel
cp extras/tv-channel-bookmarks.toml ~/.config/television/cable/bookmarks.toml

# Use it
tv bookmarks
```

### Claude Code

```bash
# Install the plugin
ln -s /path/to/codemark/extras/claude-code-plugin ~/.claude/plugins/codemark
```

This gives agents the `/codemark` skill for proactive bookmarking during exploration.

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

1. **Parse** the file with tree-sitter to build an AST
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

## License

MIT
