# Codemark

```text
   ______              __                          __ 
  / ____/____   _____ / /_   ____ ___   ____ _ ____/ /__
 / /    / __ \ / ___// __ \ / __ `__ \ / __ `// __  // /_/
/ /___ / /_/ // /__ / / / // / / / / // /_/ // /_/ // ,< 
\____/ \____/ \___//_/ /_//_/ /_/ /_/ \__,_/ \__,_//_/|_|
```

[![crates.io](https://img.shields.io/crates/v/codemark)](https://crates.io/crates/codemark)
[![CI](https://github.com/DanielCardonaRojas/codemark/actions/workflows/test.yml/badge.svg)](https://github.com/DanielCardonaRojas/codemark/actions/workflows/test.yml)
[![license](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75+-orange.svg)](https://www.rust-lang.org)
[![television](https://img.shields.io/badge/television-ready-brightgreen)](https://alexpasmantier.github.io/television/)
[![agent-ready](https://img.shields.io/badge/agent-ready-blueviolet)](#claude-code)

**Codemark** is a structural bookmarking system for code. Unlike fragile `file:line` references that break when you insert a single newline, Codemark uses **[tree-sitter](https://tree-sitter.github.io/tree-sitter/)** to capture the semantic structure of your code.

Bookmarks **self-heal** across renames, refactors, and formatting changes, making them perfect for long-running AI agent sessions, code audits, and developer knowledge management.

---

## 🚀 Why Codemark?

Standard bookmarks are "dumb"—they point to a coordinate. When the code moves, the coordinate points to the wrong thing. Codemark is "smart"—it knows what you bookmarked (e.g., "the `validateToken` function in `auth.rs`").

- **Self-Healing Resolution**: Tiered matching (Exact → Relaxed → Hash Fallback) ensures your bookmarks stay alive even if the code drifts.
- **Agent Ready**: Designed for AI coding agents (like Claude Code) to maintain context across sessions.
- **TUI & CLI Native**: Works beautifully with `fzf`, `television`, `bat`, and `Neovim`.
- **Semantic Search**: Find bookmarks by meaning using local vector embeddings (no API key required).

---

## 🛠️ Features

- 🧠 **Smart Resolution**: Bookmarks survive renames and structural changes.
- 🔍 **Semantic Search**: Find code by intent (e.g., *"where is authentication handled?"*).
- 🗃️ **Collections**: Group bookmarks into logical sets for specific tasks.
- 📦 **Git Integrated**: Track bookmarks across commits and branches.
- 🧩 **First-class Integrations**:
    - **Television**: Interactive TUI for browsing and fuzzy-finding.
    - **Neovim**: Gut signs, visual selection bookmarking, and Telescope support.
    - **Claude Code**: Specialized plugin for AI-driven bookmarking.
- 🚀 **Quick Open**: Jump directly to bookmarked code in your favorite editor.

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

### 1. Create a bookmark
Bookmark a function by line number. Codemark automatically identifies the AST node.
```bash
codemark add --file src/auth.rs --range 42 --tag auth --note "JWT entry point"
```

### 2. Find it later
Even if you've added lines or moved the function, Codemark finds it:
```bash
codemark resolve a1b2
```

### 3. Search by meaning
Reindex once to enable semantic search:
```bash
codemark reindex
codemark search --semantic "how are tokens validated?"
```

### 4. Interactive TUI
The recommended way to browse is via **[television](https://alexpasmantier.github.io/television/)**:
```bash
tv codemark
```

---

## 📖 Documentation

- [**Full Command Reference**](./docs/CLI.md) — Detailed flag and subcommand guide.
- [**Configuration**](./docs/configuration.md) — Editor setup, global/local config, and semantic search.
- [**Markdown Templates**](./docs/templates.md) — Customize `codemark show` output.
- [**Neovim Plugin**](./extras/neovim-plugin/README.md) — Setup for `codemark.nvim`.
- [**Claude Code Plugin**](./extras/claude-code-plugin/) — Power up your AI agent.

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
