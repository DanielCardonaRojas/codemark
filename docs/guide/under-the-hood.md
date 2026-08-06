# Under the Hood

Codemark is a **local-first** Rust workspace. No cloud service, account, or API key is required for any core feature.

## Architecture & Tech Stack

| Layer | Technology |
|-------|------------|
| **Language** | [Rust](https://www.rust-lang.org) (workspace: `codemark-core`, `codemark-cli`, `codemark-tui`, `codetours-server`) |
| **Structural parsing** | [tree-sitter](https://tree-sitter.github.io/tree-sitter/) |
| **Storage** | [SQLite](https://www.sqlite.org) via `rusqlite` (bundled), with [`sqlite-vec`](https://github.com/asg017/sqlite-vec) for vector search and FTS5 for full-text search |
| **Embeddings** | Local models run on [`candle`](https://github.com/huggingface/candle) for semantic search with no API key |
| **TUI** | [`ratatui`](https://ratatui.rs) + `crossterm`, with [`syntect`](https://github.com/trishume/syntect) syntax highlighting |
| **Markdown** | [`pulldown-cmark`](https://github.com/pulldown-cmark/pulldown-cmark) renders the details and collection-overview previews |
| **Templating** | [Handlebars](https://handlebarsjs.com/) |
| **Sync server** | [`axum`](https://github.com/tokio-rs/axum) + `tokio`, JWT auth (the `codetours` server) |
| **Git integration** | [`git2`](https://github.com/rust-lang/git2-rs) (libgit2) |

## Supported Languages

Codemark supports 8 built-in languages out of the box, and **any other language** via dynamic WASM grammar loading.

**Built-in:**
- 🦀 **Rust**
- 🍎 **Swift**
- 🔷 **TypeScript / TSX**
- 🐍 **Python**
- 🐹 **Go**
- ☕ **Java**
- 🎯 **Dart**
- ♯ **C#**

### Dynamic (WASM) loading

You can add support for any language without recompiling Codemark. Simply drop a compiled Tree-sitter `.wasm` grammar and a `manifest.json` into Codemark's grammar cache directory (e.g., `~/Library/Caches/codemark/grammars/<language>/` on macOS). Codemark will automatically discover it on the next run.
