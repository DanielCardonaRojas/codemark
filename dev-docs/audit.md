# Codebase Audit: Codemark

**Date:** April 9, 2026  
**Status:** Completed  
**Auditor:** Gemini CLI (Senior Software Engineer)

---

## 🏗️ 1. Architecture Overview

Codemark is a layered, local-first CLI application built in Rust, designed to provide "self-healing" structural bookmarks for developers and AI agents.

### Core Layers:
- **CLI Layer (`src/cli/`)**: Built with `clap`, featuring JSON-first output for seamless AI agent integration.
- **Engine Layer (`src/engine/`, `src/query/`)**:
    - **Generator**: Uses `tree-sitter` to create robust AST-based queries from code ranges.
    - **Resolution**: Implements a 4-tier fallback mechanism (Exact -> Relaxed -> Minimal -> Hash) to track code across refactors.
    - **Health**: Tracks "drift" and "stale" status of bookmarks.
- **Storage Layer (`src/storage/`)**: SQLite (`rusqlite`) with `sqlite-vec` extension for local vector persistence.
- **Semantic Layer (`src/embeddings/`)**: Local embedding generation via the `candle` ML framework (no external API dependencies).
- **Git Integration (`src/git/`)**: Contextual tracking via `git2` to follow bookmarks across branches/commits.

---

## 🗄️ 2. Data Model

- **Bookmarks**: Stores the structural `tree-sitter` query, metadata (tags, notes), and content hashes (SHA-256).
- **Resolutions**: Immutable history of where a bookmark was found at specific git commits.
- **Collections**: Logical groupings for organizing bookmarks by task, feature, or pull request.

---

## 🛡️ 3. Design Critique & Potential Flaws

1.  **Static Language Support**: Tree-sitter grammars are currently hardcoded in `src/parser/languages.rs`. Adding support for a new language requires a recompile of the Rust binary.
2.  **Performance at Scale**: The `hash_fallback_walk` in `src/engine/resolution.rs` scans the file system when structural matching fails. This could be slow in multi-million-line monorepos.
3.  **Local Model Lifecycle**: While `candle` provides privacy, the management of model weights (downloading, versioning, swapping) is not yet fully formalized in the current design.
4.  **SQLite Concurrency**: Simultaneous writes from multiple CLI instances or agents could lead to "database is locked" errors without a robust retry strategy in the storage layer.

---

## 💡 4. Recommendations for Evolution

1.  **Dynamic Grammar Loading (WASM)**: Move toward Tree-sitter WASM or dynamic library loading to allow users to add language support without recompiling.
2.  **Project Indexing**: Implement a lightweight "content-addressable index" to speed up `hash_fallback_walk` during renames and heavy refactoring.
3.  **Team Sync**: Add `export`/`import` or git-based sync (e.g., `git notes`) to allow teams to share architectural bookmarks.
4.  **Editor Integration**: Expand beyond the Neovim plugin to VS Code and JetBrains to provide "CodeLens" visualizations of bookmarks.

---

## 🔍 5. Competitive Landscape

| Tool | Core Focus | Comparison to Codemark |
| :--- | :--- | :--- |
| **Aider (Repo Map)** | Transient AI Context | Codemark provides **persistent** and **structural** tracking. |
| **Sourcegraph** | Global Search | Codemark is a **lightweight, local-first** CLI tool for specific bookmarks. |
| **Cursor** | IDE-Integrated Context | Codemark is **editor-agnostic** and works with any CLI agent. |
| **Universal Ctags** | Symbol Indexing | Codemark tracks **specific logic blocks**, not just named definitions. |

---

## 📝 6. Summary Conclusion
Codemark’s **structural "self-healing"** is its primary competitive advantage. By leveraging Tree-sitter to define the "essence" of a bookmark rather than just a line number or a fuzzy embedding, it provides a level of durability across refactors that standard tagging systems cannot match.
