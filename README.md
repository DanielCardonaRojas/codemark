# Codemark

[![crates.io](https://img.shields.io/crates/v/codemark)](https://crates.io/crates/codemark)
[![license](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

**In the age of AI, being in your editor is less important.** Codemark lets you bookmark the critical parts of your codebase, providing a structured map that helps you jump into the editor only when needed.

A structural bookmarking system for code. Instead of fragile `file:line` references, Codemark stores **[tree-sitter](https://tree-sitter.github.io/tree-sitter/)** queries that identify code by its AST shape. Bookmarks survive renames, refactors, and reformatting through layered resolution.

## Why

Traditional bookmarks are brittle. Codemark uses abstract syntax trees to understand your code the way a compiler does.

In the era of AI-driven development, we spend more time orchestrating agents and reviewing code than writing it line-by-line. Constant presence in a full IDE is less important than having a reliable map of the system's critical paths.

Line-number bookmarks break on any edit. Codemark bookmarks **self-heal**:

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

## Customizing Markdown Templates

The `codemark show --format markdown` output uses a Handlebars template that you can customize.

### Template Location

Templates are stored in your configuration directory:

| Platform | Template Path |
|----------|---------------|
| macOS | `~/Library/Application Support/codemark/templates/codemark_show.md` |
| Linux | `~/.config/codemark/templates/codemark_show.md` |
| Windows | `%APPDATA%\codemark\templates\codemark_show.md` |

The template file is created automatically on first run with sensible defaults. You can edit this file to customize the markdown output.

### Template Variables

The following variables are available in the template:

| Variable | Type | Description |
|----------|------|-------------|
| `{{short_id}}` | String | First 8 chars of bookmark ID |
| `{{id}}` | String | Full bookmark ID |
| `{{file_path}}` | String | Path to the file |
| `{{file_name}}` | String | Just the filename |
| `{{language}}` | String | Programming language |
| `{{status}}` | String | `active`, `drifted`, `stale`, or `archived` |
| `{{query}}` | String | Tree-sitter query |
| `{{created_at}}` | String | Creation timestamp |
| `{{created_by}}` | String? | Creator (optional) |
| `{{commit_hash}}` | String? | Git commit hash (optional) |
| `{{short_commit}}` | String? | First 8 chars of commit (optional) |
| `{{last_resolved_at}}` | String? | Last resolution time (optional) |
| `{{resolution_method}}` | String? | Resolution method (optional) |
| `{{stale_since}}` | String? | When it became stale (optional) |
| `{{tags}}` | Array | List of tags (use `{{#each tags}}` loop) |
| `{{annotations}}` | Array | List of annotations (use `{{#each annotations}}` loop) |
| `{{resolutions}}` | Array | Resolution history (use `{{#each resolutions}}` loop) |

### Custom Helpers

- `{{escape_markdown value}}` — Escapes special markdown characters
- `{{truncate value}}` — Truncates a string to 8 characters

### Default Template

```handlebars
# Bookmark: {{short_id}}

## Metadata
| Property | Value |
|----------|-------|
| **File** | {{file_path}} |
| **Language** | {{language}} |
| **Status** | {{status}} |
| **Created** | {{created_at}} |
{{#if created_by}}| **Author** | {{escape_markdown created_by}} |{{/if}}
{{#if last_resolved_at}}| **Last Resolved** | {{last_resolved_at}} |{{/if}}
{{#if resolution_method}}| **Resolution Method** | {{resolution_method}} |{{/if}}
{{#if commit_hash}}| **Commit** | `{{short_commit}}` |{{/if}}
{{#if stale_since}}| **Stale Since** | {{stale_since}} |{{/if}}

## Tree-sitter Query
```scheme
{{query}}
```

{{#if tags}}
## Tags
{{#each tags}}
- `{{escape_markdown this}}`
{{/each}}
{{/if}}

{{#if annotations}}
## Annotations
{{#each annotations}}
### {{added_by}}
*{{source}}* added: {{added_at}}

{{#if notes}}{{escape_markdown notes}}{{/if}}

{{#if context}}
```
{{escape_markdown context}}
```
{{/if}}
{{/each}}
{{/if}}

{{#if resolutions}}
## Resolution History
| Time | Method | File | Lines | Matches | Commit |
|------|--------|------|-------|---------|--------|
{{#each resolutions}}
| {{resolved_at}} | {{method}} | {{file_path}} | {{line_range}} | {{match_count}} | {{#if commit_hash}}`{{short_commit}}`{{else}}-{{/if}} |
{{/each}}
{{/if}}
```

See [TEMPLATE_DESIGN.md](TEMPLATE_DESIGN.md) for full template documentation.

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

Codemark reads configuration from two locations (layered, with local override):

1. **Global config:** `$XDG_CONFIG_HOME/codemark/config.toml` (or platform-specific default)
2. **Local override (optional):** `.codemark/config.toml` in your repository

When a local config exists, its values override the global config. This allows you to set global defaults while customizing per-repository settings.

### Config File Locations

| Platform | Global Config Path |
|----------|-------------------|
| All (if `$XDG_CONFIG_HOME` is set) | `$XDG_CONFIG_HOME/codemark/config.toml` |
| macOS | `~/Library/Application Support/codemark/config.toml` |
| Linux | `~/.config/codemark/config.toml` |
| Windows | `%APPDATA%\codemark\config.toml` |

The global config file is automatically created on first run with sensible defaults.

### Editor Configuration

The `[open]` section configures the `codemark open` command:

```toml
[open]
# Default editor command (supports placeholders)
default = "nvim +{LINE_START} {FILE}"

# Extension-specific overrides
[open.extensions]
rs = "nvim +{LINE_START} {FILE}"
swift = "xed --line {LINE_START} {FILE}"
# md = "typora {FILE}"  # uncomment to use typora for markdown
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
