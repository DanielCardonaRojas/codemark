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
- If it looks like a bookmark ID or UUID prefix: `codemark show "$ARGUMENTS"`
- If user asks to "open", "edit", or view a bookmark: `codemark open "$ARGUMENTS"`
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
- **Annotations**: bookmarks can have multiple annotations (notes, context) added by different agents over time, with provenance tracking.

## Quick Start

### Creating a collection of bookmarks (recommended for agents)
Use `--collection` when creating bookmarks to add them directly to a collection:
```bash
# Create multiple bookmarks in a single collection
codemark add \
  --file src/auth.rs --range 42 \
  --note "Core auth entry point: Handles all JWT verification" \
  --tag feature:auth --tag role:entrypoint \
  --created-by agent \
  --collection login-flow

codemark add-from-query \
  --file src/auth.swift \
  --query '(function_declaration name: (simple_identifier) @name (#eq? @name "validateToken")) @target' \
  --note "Validates JWT tokens" \
  --created-by agent \
  --collection login-flow

# View the collection
codemark collection show login-flow
```

### Method 1: Range-based bookmarking
When you know the file and line numbers:
```bash
codemark add \
  --file src/auth.rs --range 42 \
  --note "Core auth entry point: Handles all JWT verification" \
  --tag feature:auth --tag role:entrypoint \
  --created-by agent
```

### Method 2: Snippet-based bookmarking
When you have the code snippet but need to find it in a file:
```bash
echo "func validateToken(_ token: String) -> Claims" | \
  codemark add-from-snippet --file src/auth.swift \
  --note "Validates JWT tokens" \
  --created-by agent
```

### Method 3: Raw tree-sitter query (recommended for agents)
When you want precise control over what gets bookmarked:
```bash
codemark add-from-query \
  --file src/auth.swift \
  --query '(function_declaration name: (simple_identifier) @name (#eq? @name "validateToken")) @target' \
  --note "Validates JWT tokens" \
  --created-by agent
```

For common query patterns across languages, see `queries.md`.

## Best Practices

- Use `--created-by agent` to distinguish your bookmarks from the user's.
- Use `--collection <name>` when creating bookmarks to add them directly to a collection (collections are auto-created if they don't exist).
- **Iterative enhancement**: When working with an existing bookmark and discover new context, use `annotate` to add notes without re-parsing the file. Multiple agents can annotate the same bookmark over time.
- For detailed note guidelines, see `templates.md`.
- For usage examples, see `examples.md`.
- For tree-sitter query patterns, see `queries.md`.

## Commands Reference

### Creating bookmarks
```bash
# By range (line or byte) — optionally add to collection
codemark add --file src/auth.rs --range 42:67 --collection my-work

# By code snippet (searches for snippet in file) — optionally add to collection
codemark add-from-snippet --file src/auth.rs --collection my-work

# By raw tree-sitter query (most precise) — optionally add to collection
codemark add-from-query --file src/auth.rs --query '(function_declaration) @target' --collection my-work

# Preview what would be bookmarked (dry-run)
codemark add --file src/auth.rs --range 42 --dry-run
```

### Load context
```bash
codemark resolve --status active
codemark list --author agent
codemark search "authentication"
```

### Open bookmarked files
```bash
# Open a bookmark in your configured editor
codemark open <bookmark-id>

# The editor is configured via [open] section in .codemark/config.toml
# Supports placeholder substitution: {FILE}, {LINE_START}, {LINE_END}, {ID}
# Example: rs = "nvim +{LINE_START} {FILE}"
```

### Organize with collections
```bash
# Collections are auto-created when using --collection, or create explicitly:
codemark collection create login-flow --description "Step-by-step login execution path"

# Show bookmarks in a collection
codemark collection show login-flow

# List all collections
codemark collection list
```

### Check impact
```bash
codemark diff --since HEAD~3
```

### Enhancing existing bookmarks
```bash
# Add notes, context, or tags to an existing bookmark (no re-parsing needed)
codemark annotate <bookmark-id> --note "Additional context discovered during implementation"
codemark annotate <bookmark-id> --context "Related to auth-refactor feature"
codemark annotate <bookmark-id> --tag bug-fix --tag priority:high

# Add annotation as an agent with provenance
codemark annotate <bookmark-id> \
  --note "Found this during debugging the session timeout issue" \
  --added-by agent \
  --source investigation
```

### Maintenance
```bash
codemark heal --auto-archive
codemark status
```

## Supported languages
Swift, Rust, TypeScript, Python, Go, Java, C#, Dart.
