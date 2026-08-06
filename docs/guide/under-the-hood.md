# Under the Hood

Codemark is a **local-first** Rust workspace built on top of Tree-sitter, SQLite, and local embeddings. It requires no cloud service or API keys for its core features.

This document dives deep into the internal mechanisms that make Codemark durable and intelligent.

---

## Architecture & Tech Stack

| Layer | Technology |
|-------|------------|
| **Language** | [Rust](https://www.rust-lang.org) |
| **Structural parsing** | [tree-sitter](https://tree-sitter.github.io/tree-sitter/) |
| **Storage** | [SQLite](https://www.sqlite.org) via `rusqlite` with `sqlite-vec` & FTS5 |
| **Embeddings** | Local models run on [`candle`](https://github.com/huggingface/candle) |
| **TUI** | [`ratatui`](https://ratatui.rs) + `crossterm` + `syntect` |
| **Templating** | [Handlebars](https://handlebarsjs.com/) & `pulldown-cmark` |
| **Git integration** | [`git2`](https://github.com/rust-lang/git2-rs) (libgit2) |

---

## Tree-sitter & Syntax Trees

Instead of saving line numbers (`file:42`), Codemark uses Tree-sitter to parse your code into an Abstract Syntax Tree (AST). 

When you bookmark a range, Codemark doesn't just record the text; it records the **structural path** to that node (e.g., "The function named `sendRequest` inside the class `APIClient`"). Because Tree-sitter is incredibly fast and incremental, Codemark can rebuild these syntax trees in milliseconds when you resolve a bookmark later.

### Supported Languages
Codemark supports 8 built-in languages (Rust, Swift, TypeScript/TSX, Python, Go, Java, Dart, C#).
It also supports **Dynamic (WASM) loading**. You can drop a compiled Tree-sitter `.wasm` grammar into the cache directory, and Codemark will instantly understand the new language—no recompilation needed.

---

## Query Generation Logic

When a bookmark is created, the core engine must generate a Tree-sitter query that uniquely identifies that code for future resolution. This involves two critical steps:

### 1. Sticky Nodes (Landmarks)
Users rarely highlight perfect AST boundaries. If you select lines 42-45, you might have selected the middle of a `while` loop. 
Codemark uses **Sticky Nodes** (or Landmarks) defined in language profiles to "snap" your arbitrary selection to the nearest meaningful semantic boundary (like a function, a class declaration, or a named struct). This ensures the bookmark represents a logical unit of code, not just a random text chunk.

### 2. Query Compaction
The raw AST path from the root of a file to your target node can be incredibly verbose (e.g., `program -> declaration_list -> class_declaration -> block -> function_declaration -> ...`).
If Codemark saved this exact path, adding a simple wrapper (like wrapping a function in a new namespace) would break the bookmark.

Instead, Codemark applies **Query Compaction**. It strips out intermediate nodes that aren't necessary for uniqueness, preserving only the critical identifying markers (like names and specific structural parents). The result is a highly resilient Tree-sitter query that survives intermediate refactoring.

---

## Health Status Calculation

Code moves. When you run `codemark heal` or resolve a collection, Codemark calculates the health of every bookmark using a tiered matching strategy:

- 🟢 **Active**: The code was found in its original file. 
  - *Exact Match*: The generated query found the node perfectly.
  - *Relaxed Match*: The strict query failed, but Codemark loosened the constraints (e.g., ignoring parents) and found it anyway.
- 🟡 **Drifted**: The code could not be found in the original file via AST queries.
  - *Hash Fallback*: Codemark generated a whitespace-normalized hash of the original code snippet and searched the entire repository. It found the code in a *new file* (e.g., it was extracted or renamed). The bookmark is updated to point to the new location and its query is regenerated.
- 🔴 **Stale**: The code could not be found via AST queries or Hash Fallback. It was likely deleted entirely.

---

## Repository Registry & Worktrees

Codemark tracks repositories by their **Identity**, not just their path on disk.

### The Global Registry
When you run Codemark in a repository, it registers that repo in the global SQLite database (`~/.config/codemark/registry.db`). It records the Git remote (e.g., `github.com/DanielCardonaRojas/codemark`) as the canonical identity.

This allows AI agents to execute **Multi-Repo Queries** (`codemark search --repo facebook/react`) even if they aren't currently inside that repository's directory.

### Worktree Awareness
If you use Git worktrees, you might have multiple physical directories representing the same repository. Because Codemark maps identity via the global registry and Git remotes, it understands that these physical paths are the same logical project. Your bookmarks remain accessible regardless of which worktree you are working in.
