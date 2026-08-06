# What is Codemark?

**Codemark** bookmarks code by structure, not by line number. A `file:line` reference breaks the moment you add a newline above it. Codemark uses [tree-sitter](https://tree-sitter.github.io/tree-sitter/) to remember *what* you marked (a function, a block, a type) and anchors the bookmark to that named structure, so it still points at the right place after you refactor or reformat around it.

That durability is what makes it useful for long AI agent sessions, code audits, and keeping track of things you want to find again.

## Features

- 🧠 **Smart Resolution**: Queries are anchored to named structures and survive refactoring and reformatting via tiered matching (Exact → Relaxed → Hash Fallback).
- 🖥️ **Interactive Dashboard (TUI)**: Lazygit-style TUI for efficient, keyboard-first interaction.
- 📑 **Rich Metadata**: Captures AST structure, git context, content hashes, and append-only notes/tags.
- 🔍 **Semantic Search**: Find code by intent (e.g., *"where is authentication handled?"*) with local embeddings, no API key required.
- 🗃️ **Collections**: Group bookmarks into logical sets for specific tasks.
- 📦 **Git Integrated**: Track bookmarks across commits and branches.
- 🧩 **Agent Skills**: An installable skill that teaches AI coding agents to bookmark for you. Works with Claude Code, GitHub Copilot, Gemini CLI, and any agent that loads `.agents/skills`.

## Why semantic bookmarks?

While AI is transforming how we write code, spending time in the editor is still necessary. However, relying purely on raw text search or brittle line-number bookmarks doesn't scale. Codemark provides a way to encode semantic meaning into your bookmarks so queries make sense to both humans and agents.
