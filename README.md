# Codemark

[![crates.io](https://img.shields.io/crates/v/codemark)](https://crates.io/crates/codemark)
[![license](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

A structural bookmarking system for code. Instead of fragile `file:line` references, Codemark stores **[tree-sitter](https://tree-sitter.github.io/tree-sitter/)** queries that identify code by its AST shape. Bookmarks survive renames, refactors, and reformatting through layered resolution.

## Why

AI coding agents and developers revisit the same code across sessions. Line-number bookmarks break on any edit. Codemark bookmarks **self-heal**:

- **Exact match** — the AST query still finds the node (even if moved)
- **Relaxed match** — structure matches but names changed
- **Hash fallback** — content moved, found by normalized hash
- **Stale** — code was deleted; bookmark is marked stale, not silently wrong

## Features

- 🛠️ **8 Languages** — Swift, Rust, TS, Python, Go, Java, C#, Dart
- 🧠 **Semantic Search** — Find bookmarks by meaning, not just keywords
- 🧩 **Integrations** — First-class support for `fzf`, `television`, `bat`, `glow`, and `Neovim`
- 🚀 **Quick Open** — Open bookmarked files directly in your configured editor
- 🤖 **Agent Ready** — JSON-first output optimized for Claude Code and other AI agents
- 📦 **Git Integrated** — Track bookmarks across commits and diffs
- 🗃️ **Collections** — Group and reorder bookmarks for specific investigations

## Install

**Homebrew (recommended):**

```bash
brew tap DanielCardonaRojas/codemark
brew install codemark
```

**Cargo:**

```bash
cargo install codemark
```

Requires Rust 1.75+. The binary is self-contained (SQLite is bundled).

## Quick Start

```bash
# Bookmark a function by line number
codemark add --file src/auth.rs --range 42 --tag auth --note "Token validation entry point"

# Resolve — find where the code is now
codemark resolve a1b2

# List all bookmarks (in a pretty table)
codemark list --format table

# Search by meaning (semantic search)
codemark search --semantic "how we handle authentication"
```

## Supported Languages

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

## Commands

### Creating Bookmarks

```bash
# By line range (most common)
codemark add --file src/auth.rs --range 42        # single line
codemark add --file src/auth.rs --range 42:67     # line range

# With rich metadata (perfect for agents and audit trails)
codemark add --file src/auth.rs --range 42 \
  --tag auth --tag security \
  --note "Critical: validate_token is the main entry point for all API requests" \
  --context "Investigating PR #123; this function seems to be missing a boundary check" \
  --created-by "claude-agent"

# From a git diff hunk
codemark add --file src/auth.rs --hunk "@@ -42,7 +42,9 @@"
```
# From a code snippet on stdin
echo 'func validateToken' | codemark add-from-snippet --file src/auth.rs

# Using a raw tree-sitter query
codemark add-from-query --file src/auth.rs --query "(function_declaration name: (identifier) @n (#eq? @n \"login\"))"
```

### Browsing and Searching

```bash
codemark list --format table                   # human-readable table
codemark list --tag auth --lang swift          # filtered
codemark search "authentication"               # full-text search in notes/context
codemark search --semantic "auth functions"    # semantic search (vector embeddings)
codemark show a1b2                             # full details + resolution history
codemark show a1b2 --format markdown           # rich markdown (great with glow)
```

### Opening Bookmarks

```bash
# Open a bookmark in your configured editor
codemark open a1b2

# Resolves to current location, then opens in editor
# Supports extension-specific editors (e.g., .md files in Typora)
# See [Configuration](#configuration) for editor setup
```

### Health Management

```bash
codemark status                                # summary: active | drifted | stale | archived
codemark heal                                  # re-resolve all bookmarks & update status
codemark heal --auto-archive                   # also archive bookmarks stale for >7 days
codemark diff --since HEAD~3                   # bookmarks affected by recent changes
codemark gc --older-than 30d                   # permanently delete old archived bookmarks
```

### Collections

```bash
codemark collection create bugfix-auth --description "Auth fix investigation"
codemark collection add bugfix-auth a1b2 f5e6
codemark collection list                       # list all collections
codemark collection show bugfix-auth           # list bookmarks in collection
```

## Integration

### Neovim (`codemark.nvim`)

Located in `extras/neovim-plugin`. It provides visual selection bookmarking, gut signs, and Telescope integration.

```lua
-- Using lazy.nvim
{
  "DanielCardonaRojas/codemark",
  dir = "extras/neovim-plugin", -- or path to your local clone
  dependencies = { "nvim-telescope/telescope.nvim" },
  config = function()
    require("codemark").setup()
  end,
}
```

### [television](https://github.com/alexpasmant/television)

A modern, fuzzy selector with built-in support for codemark.

```bash
# Install the channel
cp extras/tv-channel-bookmarks.toml ~/.config/television/channels/codemark.toml

# Use it
tv codemark
```

### [fzf](https://github.com/junegunn/fzf)

```bash
# Browse with live preview
codemark list --format tv | fzf --preview 'bat --highlight-line {3} $(codemark preview {1} | jq -r .data.file_path)'
```

### [glow](https://github.com/charmbracelet/glow)

Render markdown bookmarks in the terminal with syntax highlighting:

```bash
codemark show a1b2 --format markdown | glow -
```

### Claude Code

Install the plugin to give Claude proactive bookmarking skills:

```bash
ln -s /path/to/codemark/extras/claude-code-plugin ~/.claude/plugins/codemark
```

## Output Modes

All commands output **JSON by default** (optimized for AI agents and scripting). Use `--format` to override.

| Mode | Format | Use Case |
|------|--------|----------|
| `json` | JSON | Default, scripting, AI agents |
| `table` | UTF-8 Table | Human inspection |
| `line` | TSV | `grep`, `awk`, simple piping |
| `tv` | Custom TSV | `television` and `fzf` integration |
| `markdown` | Markdown | `glow`, documentation |

## Configuration

Codemark reads configuration from `.codemark/config.toml` in your repository. Run `codemark init` to create the default config, or copy [docs/config.default.toml](docs/config.default.toml).

### Editor Configuration

The `[open]` section configures the `codemark open` command:

```toml
[open]
# Default editor command (supports placeholders)
default = "nvim +{LINE_START} {FILE}"

# Extension-specific overrides
[open.extensions]
rs = "nvim +{LINE_START} {FILE}"
md = "typora {FILE}"
py = "code --goto {FILE}:{LINE_START}:{LINE_END}"

# Terminal editors (codemark waits for them to exit)
[open.editor_types]
terminal = ["vim", "vi", "nvim", "neovim", "emacs", "nano", "micro", "helix", "hx"]
# GUI editors (codemark spawns and returns immediately)
gui = ["xed", "code", "idea", "subl", "typora"]
```

**Placeholders:** `{FILE}`, `{LINE_START}`, `{LINE_END}`, `{ID}`

If no config is set, codemark uses `$EDITOR` or falls back to `vim`.

See [docs/open-command-spec.md](docs/open-command-spec.md) for full documentation.

## How it works

1. **Parse** the file with [tree-sitter](https://tree-sitter.github.io/tree-sitter/) to build an AST
2. **Find** the target node (smallest declaration covering the line/byte range)
3. **Generate** an S-expression query that uniquely identifies the node
4. **Store** the query + content hash + metadata in SQLite (at `.codemark/codemark.db`)
5. **Resolve** later by running the query against the file, falling through tiers: exact → relaxed → minimal → hash fallback

## License

[MIT](LICENSE)
