# Overview

**Codemark** bookmarks code by structure, not by line number. A `file:line`
reference breaks the moment you add a newline above it; Codemark uses
[tree-sitter](https://tree-sitter.github.io/tree-sitter/) to remember *what* you
marked — a function, a type, a block — and anchors the bookmark to that named
structure so it still resolves after you refactor or reformat around it.

That durability is what makes it useful for long AI agent sessions, code audits,
and keeping track of things you want to find again.

## Why Codemark?

The way we navigate code hasn't caught up with how code is produced now.

- **Spending time in the editor matters less, but navigation still does.** In the
  AI era, you read and direct more than you type by hand. What slows you down is
  *finding* the right spot — the entry point, the boundary, the place a previous
  session already understood. Quick, durable navigation is the bottleneck.
- **Easy navigation.** A keyboard-driven dashboard (lazygit-style, vim motions)
  lets you fly through bookmarks, collections, and search without leaving the
  terminal.
- **Distill agent discoveries.** When an agent explores a codebase and finds the
  load-bearing code, that knowledge evaporates at session end unless it's
  captured. Codemark turns those discoveries into durable, queryable bookmarks.
- **Give agents high-level context.** Instead of re-exploring from scratch each
  session, an agent loads a collection and instantly knows where the important
  code lives and why.
- **Durable enough to survive refactors.** Bookmarks are anchored to AST
  structure and a normalized content hash, so renaming a file, extracting a
  method, or reformatting doesn't lose your place.
- **The encoding is semantic.** A bookmark encodes "the function named
  `validate_token`" — a query that makes sense to a human. Vectors do not. You
  can read a bookmark's query and immediately understand what it points at.
- **Sharable context.** Collections are guided tours you can publish and pull
  across a team, so onboarding and institutional knowledge travel with the code.

## Vision

Code navigation should be **structural, durable, and shareable** — equally
useful to a human reading a codebase and to an agent that needs to act on it.
Codemark's goal is to be the durable memory layer over a codebase: the place
where "this is where the important thing lives, and this is why" persists across
sessions, refactors, and handoffs.

## Features

### Search

- **Semantic search.** Find bookmarks by intent ("where is authentication
  handled?") using local vector embeddings generated on-device with
  [candle](https://github.com/huggingface/candle) — no API key, no network call
  for inference.
- **Full-text search.** Substring matching across notes, context, tags, and file
  paths via SQL, so exact lookups stay fast and predictable.

### Bookmark metadata

Every bookmark carries rich, append-only metadata:

- **Notes & comments** — two distinct channels. *Notes* are durable, plain-prose
  explanations of what the code is; *comments* are markdown discussion tied to a
  task, ticket, or PR.
- **Tags** — structured, colon-prefixed taxonomy (`feature:auth`, `layer:api`,
  `role:entrypoint`) for precise filtering.
- **Links** — attach PRs, issues, and docs to a collection.
- **Health status** — `active`, `drifted`, `stale`, or `archived`, computed by
  re-resolving the structural query against the current tree.
- **Git context** — commit hash, branch, and breadcrumbs up the enclosing
  structure.

### The dashboard (TUI)

- **Keyboard-driven, lazygit-inspired.** `1`–`5` jump to panes, `+`/`-` resize,
  `Ctrl+C` quits from anywhere.
- **Vim motions.** `j`/`k` move, `J`/`K` scroll, `h`/`l` step through a
  collection, `n`/`N` follow markdown links.
- **Mouse support.** Click to focus and select, scroll to navigate panes.
- **Many themes.** Bundled schemes including Catppuccin Mocha, Everforest,
  Dracula, Nord, Gruvbox, Solarized, and OneHalfDark. Drop in `.tmTheme` or
  base16 `.yaml` files for fully cohesive preview + chrome theming.

### Repositories

- **Current repository detection.** Walks up from your working directory to find
  the Git root and stores bookmarks alongside it — zero setup.
- **Multi-repo queries.** Query several repositories at once by identity
  (`--repo owner/name`) or by explicit database paths (`--db path`).

### Integration

- **Agent skill.** An installable skill (`codemark install-skill`) teaches AI
  coding agents to create and recall bookmarks for you. Works with Claude Code,
  GitHub Copilot, Gemini CLI, and any agent that loads `.agents/skills`.
- **Configurable editor.** Press `o` to open a bookmark in your editor —
  per-extension commands, terminal vs GUI detection, and `$EDITOR` fallback.
