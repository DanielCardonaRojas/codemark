---
name: codemark
description: >
  Manage structural code bookmarks that survive refactoring. Use when the user says
  "remember this", "bookmark this", "save this location", "load my bookmarks",
  "what do I have bookmarked", "mark this code", "codemark", or when starting a new
  session and needing to reload context from a previous session. Also use proactively
  when you discover critical code during exploration — entry points, boundaries,
  configuration, error handling — that you'd want to remember if starting over.
allowed-tools: Bash(codemark *)
---

# Codemark — Structural Code Bookmarking

You have access to `codemark`, a CLI tool that creates **structural bookmarks** for code using tree-sitter AST queries. Unlike file:line references, these bookmarks survive renames, refactors, and reformatting.

## Active Context
```!
# Inject a summary of active bookmarks into the session
codemark list --status active
```

## Agent Directives: Handling Invocations
If the user invoked you with arguments (e.g. `/codemark something`), use `$ARGUMENTS` to search the active bookmarks or execute the desired behavior:
- Search text: `codemark search "$ARGUMENTS"`
- If no arguments were provided, interpret the user's intent based on the conversation context.

## When to use codemark

- **Starting a session**: Load bookmarks from a previous session to restore context.
- **Exploring code**: When you discover something critical, bookmark it with context about *why* it matters.
- **During work**: Bookmark code you'll need to reference later — especially cross-file relationships.
- **Ending a session**: Validate bookmarks so the next session has accurate references.
- **Checking impact**: Use `diff` to see which bookmarks are affected by recent changes.

**Proactive bookmarking**: When you recognize code that is critical to the current task — entry points, auth boundaries, error handling paths, configuration — bookmark it immediately with a note explaining its significance and relationship to the work. Don't bookmark everything you read; bookmark what you'd want to know if starting over tomorrow.

## Key Concepts

- **Bookmarks** store a tree-sitter query that identifies code by AST structure, not line numbers.
- **Resolution** re-finds bookmarked code even after edits (exact → relaxed → hash fallback).
- **Collections** group related bookmarks (one per feature, bugfix, or investigation).
- **Status**: active (healthy), drifted (found but moved), stale (lost), archived (cleaned up).
- **Author**: bookmarks track who created them (`--created-by agent` vs default `user`).

## Quick Start

1. **Load bookmarks**: `codemark resolve --status active`
2. **Bookmark with metadata**:
   Use the `codemark add` command directly. Do NOT include redundant structural information like `[Source_file: ...]` in the notes, as this is already part of the bookmark metadata.
   ```bash
   codemark add \
     --file src/auth.rs --range 42 \
     --note "Core auth entry point: Handles all JWT verification" \
     --tag feature:auth --tag role:entrypoint \
     --created-by agent
   ```

## Best Practices

- Use `--created-by agent` to distinguish your bookmarks from the user's.
- Use **ordered collections** for execution paths: `codemark collection create <name>`.
- For detailed note guidelines, see `templates.md`.
- For usage examples, see `examples.md`.

## Commands Reference

### Load context
```bash
codemark resolve --status active
codemark list --author agent
codemark search "authentication"
```

### Organize with collections
```bash
codemark collection create login-flow --description "Step-by-step login execution path"
codemark collection add login-flow <id1> <id2>
codemark collection show login-flow
```

### Check impact
```bash
codemark diff --since HEAD~3
```

### Maintenance
```bash
codemark validate
codemark status
```

## Supported languages
Swift, Rust, TypeScript, Python, Go, Java, C#, Dart.
