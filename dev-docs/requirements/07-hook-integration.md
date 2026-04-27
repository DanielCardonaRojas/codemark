# Codemark: Hook Integration & Agent Workflow

## Overview

Codemark's primary value comes from **automatic** bookmark creation — a note-taking sub-agent observes what the primary agent does and captures structural bookmarks without interrupting the workflow. This document specifies the integration model.

## Claude Code Hook Architecture

Claude Code supports hooks that fire on tool invocations. Codemark uses these hooks to trigger bookmark capture.

### Hook Configuration

In `.claude/hooks.json` (or the project's Claude Code settings):

```json
{
  "hooks": {
    "post_tool_use": [
      {
        "type": "command",
        "command": "codemark-observer",
        "when": "tool_name in ['Read', 'Edit', 'Write', 'Grep']",
        "timeout_ms": 5000
      }
    ],
    "session_end": [
      {
        "type": "command",
        "command": "codemark heal --auto-archive",
        "timeout_ms": 30000
      }
    ]
  }
}
```

### Observer Process

`codemark-observer` is a lightweight wrapper (or a mode of the codemark binary) that:

1. Receives the tool use context from the hook (tool name, arguments, result summary).
2. Decides whether this interaction is bookmark-worthy.
3. If yes, invokes `codemark add` or `codemark add-from-snippet`.

### Bookmark-Worthy Heuristics

Not every file read deserves a bookmark. The observer filters based on:

| Signal                    | Action                                          |
|---------------------------|------------------------------------------------|
| `Edit` tool used          | Bookmark the edited region (high value)         |
| `Write` tool used         | Bookmark key functions in the new file          |
| `Read` + agent took action on it | Bookmark if the agent's next tool references it |
| `Grep` with few results   | Don't bookmark (search, not navigation)        |
| `Read` of config/docs     | Don't bookmark (not structural code)           |

### Context Capture

The observer captures:
- **context**: The agent's current task description (from tool use context).
- **tags**: Inferred from the file path and agent's task (e.g., file in `auth/` → tag `auth`).
- **notes**: Generated from the agent's activity (e.g., "edited during auth refactor").

## Session Lifecycle

### Session Start
```bash
# Agent's first action: load relevant bookmarks
codemark resolve --status active --json | jq '.data[]'

# Or load bookmarks for the area the agent will work on
codemark list --tag auth --json
```

### During Session
- Hooks fire on each tool use.
- Observer creates bookmarks for significant interactions.
- Agent can explicitly bookmark via natural language → tool use:
  ```
  "Remember this function for later" → codemark add-from-snippet ...
  ```

### Session End
- `codemark heal --auto-archive` runs, updating health statuses.
- Stale bookmarks past the grace period are archived.

## Sub-Agent Architecture

The note-taking sub-agent can be implemented as:

### Option A: Hook + CLI (Recommended for v1)
- The hook calls `codemark add-from-snippet` directly.
- Minimal overhead, no agent API needed.
- Heuristics are simple rules in the observer script.

### Option B: Agent API (Future)
- The hook sends an event to a long-running agent process.
- The agent has richer context (conversation history, task graph).
- Can make smarter decisions about what to bookmark and how to annotate.

## Performance Budget

Hooks must not slow down the primary agent. Targets:
- Observer decision (bookmark or skip): < 10ms
- Bookmark creation (when triggered): < 100ms
- Total hook overhead per tool use: < 150ms

If a hook exceeds its timeout, it's killed and the primary agent continues unaffected.

## Configuration

### `.codemark/config.toml`
```toml
[observer]
enabled = true
auto_tag_from_path = true       # infer tags from directory names
min_edit_size = 10              # minimum bytes changed to trigger bookmark
ignore_patterns = [             # files to never bookmark
    "*.lock",
    "*.generated.*",
    "node_modules/**",
    ".build/**",
    "Pods/**"
]

[health]
archive_after_days = 7
gc_after_days = 30

[resolution]
cross_file_search = false       # expensive; opt-in
max_hash_scan_files = 50        # limit for cross-file hash search
```

## Example Workflow

```
Session 1: Agent refactors auth module
├── Agent reads src/auth/middleware.swift
│   └── Observer: skip (read only, no action yet)
├── Agent edits validateToken() function
│   └── Observer: bookmark! tag:auth note:"modified during auth refactor"
├── Agent reads src/auth/token_store.swift
│   └── Observer: skip
├── Agent edits refreshToken() function
│   └── Observer: bookmark! tag:auth note:"modified during auth refactor"
├── Agent creates src/auth/jwt_validator.swift
│   └── Observer: bookmark top-level declarations, tag:auth
└── Session ends
    └── codemark heal: 3 active bookmarks created

Session 2: Agent fixes a bug in auth
├── Agent starts, loads context:
│   codemark resolve --tag auth --json
│   → validateToken @ middleware.swift:42 (active, exact)
│   → refreshToken @ token_store.swift:88 (active, exact)
│   → JWTValidator @ jwt_validator.swift:12 (active, exact)
├── Agent navigates directly to relevant code — no re-discovery needed
├── Agent edits validateToken()
│   └── Observer: updates existing bookmark
└── Session ends
    └── codemark heal: 3 active (1 updated)

Between sessions: another developer renames middleware.swift → auth_handler.swift

Session 3: Agent returns
├── codemark resolve --tag auth --json
│   → validateToken: exact match failed, hash fallback found it in auth_handler.swift:42
│     status: drifted, query regenerated
│   → refreshToken @ token_store.swift:88 (active, exact)
│   → JWTValidator @ jwt_validator.swift:12 (active, exact)
├── Agent knows the file was renamed, proceeds with correct path
```
