# 🔖 Codemark

[![crates.io](https://img.shields.io/crates/v/codemark)](https://crates.io/crates/codemark)
[![CI](https://github.com/DanielCardonaRojas/codemark/actions/workflows/test.yml/badge.svg)](https://github.com/DanielCardonaRojas/codemark/actions/workflows/test.yml)
[![license](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75+-orange.svg)](https://www.rust-lang.org)
[![agent-ready](https://img.shields.io/badge/agent-ready-blueviolet)](#claude-code)

**Codemark** is a structural bookmarking system for code. Unlike fragile `file:line` references that break when you insert a single newline, Codemark uses **[tree-sitter](https://tree-sitter.github.io/tree-sitter/)** to capture the semantic structure of your code.

Bookmarks **self-heal** across renames, refactors, and formatting changes, making them perfect for long-running AI agent sessions, code audits, and developer knowledge management.

---

## 🚀 Why Codemark?

Standard bookmarks are "dumb"—they point to a coordinate. When the code moves, the coordinate points to the wrong thing. Codemark is "smart"—it knows what you bookmarked (e.g., "the `validateToken` function in `auth.rs`").

- **Self-Healing Resolution**: Tiered matching (Exact → Relaxed → Hash Fallback) ensures your bookmarks stay alive even if the code drifts.
- **Native TUI Dashboard**: A powerful, keyboard-centric command center for managing your code knowledge.
- **Agent Ready**: Designed for AI coding agents (like Claude Code) to maintain context across sessions.
- **Semantic Search**: Find bookmarks by meaning using local vector embeddings (no API key required).

---

## 🖥️ Native Dashboard (TUI)

Codemark features a built-in, keyboard-driven dashboard inspired by `lazygit`. It's the primary interface for managing structural bookmarks and tours.

![Screenshot](./codemark_tui_screenshot.png)
[Query Preview](./codemark_tui_query_screenshot.png) |
[Collections](./codemark_tui_collections_screenshot.png)

---

## 🛠️ Features

- 🧠 **Smart Resolution**: Bookmarks survive renames and structural changes.
- 🖥️ **Interactive Dashboard**: Lazygit-style TUI for efficient, keyboard-first interaction.
- 📑 **Rich Metadata**: Captures AST structure, git context, content hashes, and append-only notes/tags.
- 🔍 **Semantic Search**: Find code by intent (e.g., *"where is authentication handled?"*).
- 🗃️ **Collections**: Group bookmarks into logical sets for specific tasks.
- 📦 **Git Integrated**: Track bookmarks across commits and branches.
- 🧩 **First-class Integrations**:
    - **Neovim**: Gut signs, visual selection bookmarking, and Telescope support.
    - **Claude Code**: Specialized plugin for AI-driven bookmarking.

---

## 💻 Installation

### Homebrew (macOS/Linux)
```bash
brew tap DanielCardonaRojas/codemark
brew install codemark
```

### Cargo
```bash
cargo install codemark
```

*Requires Rust 1.75+. SQLite is bundled.*

---

## 🚦 Quick Start

### 1. Install the agent skill
Teach your AI agent how to use Codemark. This installs the Codemark skill into your agent's skills directory:
```bash
codemark install-skill --agent claude --scope user
```

### 2. Let your agent bookmark for you
Ask your agent to capture the structure of your codebase as it explores. For example:

> *"Create a collection called `auth-audit` and bookmark the key functions involved in authentication."*

Your agent will create a collection and add structural bookmarks that survive refactors.

### 3. Browse with the Dashboard
Launch the keyboard-driven TUI to review and manage what's been captured:
```bash
codemark dashboard
```

---

## 📖 Documentation

- [**Full Command Reference**](./dev-docs/CLI.md) — Detailed flag and subcommand guide.
- [**Configuration**](./dev-docs/configuration.md) — Editor setup, global/local config, and semantic search.
- [**Neovim Plugin**](./extras/neovim-plugin/README.md) — Setup for `codemark.nvim`.
- [**Claude Code Plugin**](./extras/claude-code-plugin/) — Power up your AI agent.

---

## 🎨 Customizing Markdown Output

Codemark uses [Handlebars](https://handlebarsjs.com/) templates to format bookmark output. You can customize the `codemark show` command output by providing your own template.

### Template Locations

1. **User config directory** (highest priority): `~/.config/codemark/templates/codemark_show.md`
   - Create this file to override the default template
   - The directory is created automatically if it doesn't exist

2. **Project template** (for reference): `./templates/codemark_show.md`
   - Contains the default template that you can copy and modify

### Copy and Customize

To create your custom template:

```bash
# Copy the default template to your config directory
mkdir -p ~/.config/codemark/templates
cp ./templates/codemark_show.md ~/.config/codemark/templates/

# Edit it to your liking
nano ~/.config/codemark/templates/codemark_show.md
```

### Available Template Variables

| Variable | Description |
|----------|-------------|
| `{{short_id}}` | First 8 chars of bookmark ID |
| `{{id}}` | Full bookmark ID |
| `{{file_path}}` | Path to the file |
| `{{file_name}}` | Just the filename |
| `{{language}}` | Programming language |
| `{{health}}` | `active`, `drifted`, `stale`, or `archived` |
| `{{ui_status}}` | Projected UI status |
| `{{current_resolution_id}}` | ID of the current resolution pointer |
| `{{status}}` | Alias for `{{health}}` (deprecated) |
| `{{query}}` | Tree-sitter query |
| `{{created_at}}` | Creation timestamp |
| `{{created_by}}` | Creator (optional) |
| `{{commit_hash}}` | Git commit hash (optional) |
| `{{short_commit}}` | First 8 chars of commit (optional) |
| `{{last_resolved_at}}` | Last resolution time (optional) |
| `{{resolution_method}}` | Resolution method (optional) |
| `{{stale_since}}` | When it became stale (optional) |

### Loops and Conditionals

- `{{#each annotations}}` — Loop through annotations
- `{{#each resolutions}}` — Loop through resolution history (each has `id`, `is_current`, `is_anchored`, `ui_status`)
- `{{#each tags}}` — Loop through tags
- `{{#if created_by}}` — Conditional content
- `{{escape_markdown value}}` — Escape special markdown characters
- `{{truncate value}}` — Truncate string to 8 characters

For the full template specification, see the [templates reference](./dev-docs/templates.md).

---

## 🎨 Supported Languages

Codemark speaks AST for:
- 🦀 **Rust**
- 🍎 **Swift**
- 🔷 **TypeScript / TSX**
- 🐍 **Python**
- 🐹 **Go**
- ☕ **Java**
- 🎯 **Dart**
- ♯ **C#**

---

## 🛡️ License

Released under the [MIT License](LICENSE).
