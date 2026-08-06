# What is Codemark?

Codemark is a CLI and Terminal User Interface (TUI) tool designed to help developers and AI agents navigate and share context within codebases.

## The Vision
While AI is transforming how we write code, spending time in the editor is still necessary. However, relying purely on raw text search or brittle line-number bookmarks doesn't scale. Codemark provides a way to encode semantic meaning into your bookmarks so queries make sense to both humans and agents.

## Why Codemark?
- **Semantic Encoding:** Queries make sense to humans (unlike vectors).
- **Durable:** Powered by tree-sitter, bookmarks attach to AST nodes, not just line numbers.
- **Distill Discoveries:** Save what your AI agents find for future reference.
- **Share Context:** Provide high-level, accurate context back to agents.

## Features
- **Fast Text Search (FTS) & Semantic Search**
- **Bookmark Metadata:** Links, notes, comments, health status.
- **Keyboard-Driven TUI:** Lazygit-inspired, with Vim motions and mouse support.
- **Multi-Repo Queries** & Automatic repository detection.
- **Agent Skill Integration**
- **Highly Configurable:** Editor integration and multiple themes.
