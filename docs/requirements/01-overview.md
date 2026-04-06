# Codemark: Project Overview

## Problem Statement

AI coding agents suffer from **session amnesia**. Each conversation starts from zero — the agent re-discovers the same functions, re-navigates the same files, and burns time and tokens rebuilding context that existed minutes ago. Existing persistence mechanisms (file-path bookmarks, line-number references, plain-text notes) are fragile: they break on refactors, renames, insertions, and branch switches.

## Vision

Codemark is a **structural bookmarking system for code**. Instead of storing brittle file paths and line numbers, it stores **tree-sitter queries** that identify code by its AST shape, paired with **semantic metadata** about *why* that code matters. Bookmarks self-heal through layered resolution, track their own health, and gracefully degrade when code evolves — giving developers and agents a persistent, queryable, code-aware memory that survives refactors, renames, and session boundaries.

Codemark is a **Unix-philosophy CLI tool**: it produces line-oriented, pipe-friendly output that integrates naturally with the developer's existing toolkit — fzf, television, grep, awk, and shell scripts. Interactive browsing, fuzzy search, and live preview of resolved bookmarks are first-class workflows, not afterthoughts.

## Core Value Proposition

1. **Durable references**: Bookmarks identify code structurally (AST queries), not positionally (file:line). A renamed function is still found.
2. **Semantic context**: Each bookmark carries *why* it was saved — the agent's intent, the task context, user-assigned tags — not just *where*.
3. **Self-healing**: Layered resolution (exact match → relaxed query → content hash fallback) means bookmarks degrade gracefully rather than breaking silently.
4. **Health awareness**: Bookmarks know when they're drifting. The system surfaces stale references instead of returning wrong answers.
5. **Git-native**: Bookmarks capture HEAD context at creation, track across commits, and can report which bookmarks a diff affected. Blame is queried on demand, not cached.
6. **Collections**: Group related bookmarks into named sets — one per bugfix, feature, or investigation — that can be loaded, resolved, and browsed as a unit.
7. **Cross-repository queries**: Each repo maintains its own database, but read commands accept multiple `--db` paths to search across repos — enabling developers and agents to draw connections across service boundaries.
8. **Semantic search** (Phase 2a): Natural language queries like "where's the login logic?" return relevant bookmarks even without exact keyword matches, using vector embeddings stored locally in SQLite.

## Target Users

- **Developers** (primary): Engineers who want structural bookmarks for navigating, annotating, and revisiting code — with interactive search and preview through tools like fzf and television.
- **AI coding agents** (secondary): Claude Code, Cursor, Copilot Workspace, and similar tools that operate across sessions and need persistent code references.
- **Agent framework developers**: Teams building custom agent pipelines that need persistent code references.

## Integration Model

### Human Workflow (Primary)

Codemark is designed as a standalone CLI tool that integrates with the Unix ecosystem:

```
┌─────────────────────────────────────────────────────────────────┐
│  Developer Workflow                                             │
│                                                                 │
│  codemark list --format line | fzf --preview 'codemark preview {}' │
│  codemark list --format line | tv --preview-command 'codemark preview {}' │
│  tv bookmarks                    # via shipped television channel │
│                                                                 │
│  ┌──────────────┐   pipe    ┌──────────────┐   action          │
│  │ codemark CLI  │─────────>│ fzf / tv     │──────────> open   │
│  │ (list/search) │          │ (fuzzy pick)  │           editor  │
│  └──────┬───────┘          └──────┬───────┘                    │
│         │                         │                             │
│  codemark preview <id>     codemark resolve <id>                │
│  → syntax-highlighted      → file:line:col for $EDITOR         │
│    code with query match                                        │
│    highlighted                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Agent Workflow (Secondary)

Codemark also integrates with AI agent workflows through **hooks** — specifically Claude Code hooks that trigger a lightweight note-taking sub-agent. The sub-agent observes what the primary agent touches and automatically captures structural bookmarks.

```
┌─────────────────────────────────────────────┐
│  AI Agent Session                           │
│                                             │
│  ┌─────────┐    hook     ┌──────────────┐  │
│  │ Primary  │───────────>│ Note-taking  │  │
│  │ Agent    │            │ Sub-agent    │  │
│  └─────────┘            └──────┬───────┘  │
│                                 │          │
│                          codemark CLI      │
│                                 │          │
│                          ┌──────▼───────┐  │
│                          │   SQLite DB  │  │
│                          └──────────────┘  │
└─────────────────────────────────────────────┘

Next session:
┌─────────────────────────────────────────────┐
│  AI Agent Session (new)                     │
│                                             │
│  codemark resolve --tag "auth-flow"         │
│  → Returns current file:line:col for all    │
│    auth-related bookmarks, even if code     │
│    was refactored between sessions          │
└─────────────────────────────────────────────┘
```

## Technology Choice

**Rust**, for the following reasons:

- The tree-sitter ecosystem is Rust-native — bindings are first-class, maintained by the tree-sitter org.
- `rusqlite` is battle-tested for embedded SQLite.
- Static binary with zero runtime dependencies — critical for CLI tooling that agents invoke.
- `clap` provides excellent CLI ergonomics with subcommands.
- `serde` handles JSON serialization for tags and structured output.
- Performance matters: resolution runs in hooks on every tool call; it must be fast.

## Success Criteria

1. A bookmark created in session N resolves correctly in session N+1, even after moderate refactoring.
2. Resolution completes in < 50ms for a single bookmark on a medium-sized codebase.
3. The CLI is a single static binary with no runtime dependencies beyond the SQLite file.
4. A developer can fuzzy-search bookmarks and preview resolved code in < 1 second via fzf or television.
5. An agent can query "what do I know about auth?" and get back current file locations with semantic context.
6. Stale bookmarks are surfaced, not silently wrong.
7. All list/search output is pipe-friendly by default — one entry per line, parseable by standard Unix tools.
