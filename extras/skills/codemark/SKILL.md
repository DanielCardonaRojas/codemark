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

When starting a session, inject a summary of active bookmarks:

```
codemark list --health active
```

## Agent Directives: Handling Invocations
If the user invoked you with arguments (e.g. `/codemark something`), use `$ARGUMENTS` to search the active bookmarks or execute the desired behavior:
- Search text: `codemark search "$ARGUMENTS"`
- If it looks like a bookmark ID or UUID prefix: `codemark show "$ARGUMENTS"`
- If user asks to "open", "edit", or view a bookmark: `codemark open "$ARGUMENTS"`
- If no arguments were provided, interpret the user's intent based on the conversation context.

## Author Identity (REQUIRED)

Whenever **you** create or annotate a bookmark, identify yourself by name with
`--created-by`. Do **not** use the generic `agent` value — be specific about which
agent you are. Use the slug that matches your own runtime:

| Agent | `--created-by` value |
|-------|----------------------|
| Claude / Claude Code | `claude` |
| OpenAI Codex | `codex` |
| GitHub Copilot | `copilot` |
| Pi | `pi` |

Reserve the default `user` for bookmarks the human creates themselves. The
examples below use `--created-by claude`; **substitute your own agent slug** from
the table above. This lets `--author claude`, `--author codex`, etc. filter
bookmarks by the specific agent that authored them.

## Intelligent Bookmark Discovery

When the user asks about something, try these strategies in order:

### 1. Semantic Search (Most Flexible)
Best for natural language queries and conceptual searches:
```bash
codemark search "user query" --semantic
```

### 2. Tag-Based Filtering
When the user mentions specific concepts or categories. **`--tag` on `list` is single-valued** — pass one tag and combine it with other filters (`--health`, `--author`, `--lang`, `--file`). To narrow by multiple tags at once, use semantic search instead.
```bash
codemark list --tag feature:auth --health active
codemark list --tag layer:api --author claude
codemark list --tag type:config --lang rust
```

### 3. File-Aware Search
When discussing specific files or modules:
```bash
codemark list --file src/auth.rs --health active
codemark list --file src/auth.rs --lang rust
```

### 4. Collection Browsing (best for gathering context)
When exploring a feature or workflow, render the whole collection as markdown — this is the fastest way to load context: it includes the description, tags, links (PRs/docs), and every step's bookmark ID + note.
```bash
codemark tour list
codemark tour show <collection-name> --format markdown
```
Then drill into any step by its short ID: `codemark show <id> --format markdown`.

### 5. Hybrid Search Pattern
For best results, combine semantic search with filters. **Note: `search` has no `--tag` flag** — filter searches with `--lang`, `--author`, `--health`, `--collection`:
```bash
codemark search "authentication" --lang rust --author claude
codemark search "middleware" --health active --collection auth-flow
```

### Discovery Strategy Summary
| User Query Type | Primary Strategy | Fallback |
|-----------------|------------------|----------|
| "Show my auth bookmarks" | `--tag feature:auth` | semantic search |
| "Where's the config?" | `--tag type:config` | `--tag role:config` |
| "Bookmarks in auth.rs" | `--file src/auth.rs` | semantic search |
| "Bookmarks a specific agent authored" | `--author <agent-slug>` (e.g. `--author claude`) | semantic search |
| "What did I mark for X?" | semantic search | tour browse |

## When to use codemark

- **Starting a session**: Load bookmarks from a previous session to restore context.
- **Exploring code**: When you discover something critical, bookmark it with context about *why* it matters.
- **During work**: Bookmark code you'll need to reference later — especially cross-file relationships.
- **Ending a session**: Validate bookmarks so the next session has accurate references.
- **Checking impact**: Use `codemark tour diff` to see which bookmarks are affected by recent changes.

**Proactive bookmarking**: When you recognize code that is critical to the current task — entry points, auth boundaries, error handling paths, configuration — bookmark it immediately with a note explaining its significance and relationship to the work. Don't bookmark everything you read; bookmark what you'd want to know if starting over tomorrow.

## Key Concepts

- **Bookmarks** store a tree-sitter query that identifies code by AST structure, not line numbers.
- **Resolution** re-finds bookmarked code even after edits (exact → relaxed → hash fallback).
- **Tours/Collections** group related bookmarks (one per feature, bugfix, or investigation).
- **Health**: active (healthy), drifted (found but moved), stale (lost), archived (cleaned up).
- **Author**: bookmarks track who created them. Set `--created-by` to your specific agent slug (`claude`, `codex`, `copilot`, `pi`) — not the generic `agent`; the default `user` is reserved for the human. See **Author Identity** above.
- **Notes vs. Comments** (two distinct, durable-vs-ephemeral channels):
  - **Notes** (`--note`) — durable explanations of *what the code is and why it matters*. Context-independent, reusable across any session. Use multiple `--note` flags to capture different aspects. Rendered as plain prose (keep them clean and self-contained).
  - **Comments** (`--comment`) — **markdown** discussion tied to a *task, ticket, PR, or debugging session*. Ephemeral and session-scoped. Rendered as markdown, so they may contain links, lists, and code blocks. Use these for "investigating X for TICKET-42" rather than polluting notes.
- **Annotations**: each `--note`/`--context` becomes an annotation with provenance (author, timestamp, source), so multiple agents can layer context over time.

## Tag Taxonomy

Use structured, colon-prefixed tags for powerful filtering. Combine multiple tags to create precise filters.

### Feature/Domain Tags
Identify which feature or domain area the bookmark belongs to.
- `feature:<name>` — e.g., `feature:auth`, `feature:logging`, `feature:payments`
- `domain:<name>` — e.g., `domain:user-management`, `domain:analytics`

### Architectural Layer Tags
Identify the architectural layer or component boundary.
- `layer:api` — HTTP handlers, API endpoints, controllers
- `layer:business` — Business logic, domain services, use cases
- `layer:data` — Database queries, repositories, data access
- `layer:infra` — Infrastructure, external services, I/O
- `layer:ui` — UI components, views, presentation logic
- `layer:config` — Configuration, constants, environment setup

### Role/Responsibility Tags
Identify the primary role or responsibility of the code.
- `role:entrypoint` — Application entry points, main functions
- `role:handler` — Request handlers, event handlers
- `role:service` — Service layer, business logic
- `role:repository` — Data access, database operations
- `role:middleware` — Middleware, interceptors, filters
- `role:validator` — Validation, verification logic
- `role:transformer` — Data transformation, mapping, conversion
- `role:config` — Configuration loading/parsing
- `role:constant` — Constants, enums, static values
- `role:error` — Error handling, error types
- `role:test` — Test utilities, fixtures, test helpers
- `role:utility` — Helper functions, utilities

### Type Tags
Identify the kind of code element.
- `type:function` — Functions, methods
- `type:class` — Classes, structs
- `type:interface` — Interfaces, protocols, traits
- `type:enum` — Enums, unions
- `type:constant` — Constants, literals
- `type:module` — Modules, packages

### Task/Work Tags
Link bookmarks to specific tasks or work items.
- `task:<id>` — e.g., `task:fix-123`, `task:refactor-auth`
- `pr:<number>` — Associated pull request
- `issue:<number>` — Associated issue

### Security Tags
Identify security-sensitive code.
- `security:auth` — Authentication, authorization
- `security:crypto` — Encryption, hashing, cryptography
- `security:validation` — Input validation, sanitization
- `security:sensitive` — Accesses sensitive data

### Recommended Tag Combinations
Apply **multiple tags when creating** a bookmark (`add`/`edit`/`annotate` accept repeated `--tag`). Note that *filtering* (`list --tag`) only matches a single tag at a time.
```bash
# Auth entry point
--tag feature:auth --tag layer:api --tag role:entrypoint --tag security:auth

# Database query
--tag feature:users --tag layer:data --tag role:repository

# Business logic
--tag feature:payments --tag layer:business --tag role:service

# Configuration
--tag layer:config --tag role:constant
```

### Module/Package Tags (Language-Specific)
Always include the module or package context from the file path. Each language ecosystem has its own conventions—use the appropriate tag format.

#### Swift
- **Tag**: `module:<name>` — Swift Package Manager module name
- **Inference**: From `Sources/<ModuleName>/` or target name in Package.swift
| File path | Module tag |
|-----------|------------|
| `Sources/AuthService/Validator.swift` | `--tag module:AuthService` |
| `Sources/App/Models/User.swift` | `--tag module:App` |

#### Rust
- **Tag**: `crate:<name>` — For workspace crates (e.g., `crates/`)
- **Tag**: `module:<name>` — For modules within `src/`
| File path | Module tag |
|-----------|------------|
| `crates/auth/src/lib.rs` | `--tag crate:auth` |
| `crates/auth/src/service.rs` | `--tag crate:auth --tag module:service` |
| `src/auth/mod.rs` | `--tag module:auth` |
| `src/auth/service.rs` | `--tag module:auth` |

#### Go
- **Tag**: `package:<path>` — Full package path relative to module root
| File path | Module tag |
|-----------|------------|
| `internal/auth/handler.go` | `--tag package:internal.auth` |
| `pkg/api/middleware.go` | `--tag package:pkg.api` |
| `cmd/server/main.go` | `--tag package:cmd.server` |
| `handler.go` (root) | `--tag package:root` |

#### Python
- **Tag**: `package:<path>` — Python package path (dot notation)
| File path | Module tag |
|-----------|------------|
| `app/auth/service.py` | `--tag package:app.auth` |
| `src/backend/db/models.py` | `--tag package:src.backend.db` |
| `tests/test_auth.py` | `--tag package:tests` |

#### TypeScript / JavaScript
- **Tag**: `module:<name>` — Directory or workspace package name
| File path | Module tag |
|-----------|------------|
| `src/auth/service.ts` | `--tag module:auth` |
| `packages/backend/src/db.ts` | `--tag module:backend` |
| `components/auth/Login.tsx` | `--tag module:components.auth` |
| `lib/api/client.ts` | `--tag module:lib.api` |

#### Java
- **Tag**: `package:<name>` — Java package name (dot notation)
| File path | Module tag |
|-----------|------------|
| `com/app/auth/AuthService.java` | `--tag package:com.app.auth` |
| `org/mycompany/api/handler.java` | `--tag package:org.mycompany.api` |

#### C#
- **Tag**: `namespace:<name>` — C# namespace
| File path | Module tag |
|-----------|------------|
| `App.Auth/Services/AuthService.cs` | `--tag namespace:App.Auth.Services` |
| `MyCompany.Data/Repositories/UserRepo.cs` | `--tag namespace:MyCompany.Data.Repositories` |

#### Dart
- **Tag**: `package:<name>` — Dart package name
| File path | Module tag |
|-----------|------------|
| `lib/auth/service.dart` | `--tag package:auth` |
| `lib/models/user.dart` | `--tag package:models` |

**Example (Rust crate):**
```bash
codemark add --file src/auth.rs --range 10 --note "Core JWT validation" --tag crate:auth --tag feature:auth --tag layer:business --tag role:validator
```

**Example (Go package):**
```bash
codemark add --file internal/auth/handler.go --range 25 --note "HTTP handler for authentication" --tag package:internal.auth --tag feature:auth --tag layer:api --tag role:handler
```

## Quick Start

> **IMPORTANT**: The `codemark add` command requires `--file <path>` for all operations. The file path is NOT a positional argument.

### Creating a tour/collection of bookmarks (recommended for agents)
Use `--collection` when creating bookmarks to add them directly to a tour. **Always prefer range-based targeting** as the first resource. Add **multiple targeted notes** with repeated `--note` (one per distinct aspect), and use `--comment` for task/ticket-specific discussion:

```bash
# 1. Preferred: Range-based targeting (Line or Point) — several focused notes
codemark add --file src/auth.rs --range 42 \
  --note "Core auth entry point — all signed requests pass through here" \
  --note "Verifies JWT signature and expiry" \
  --collection login-flow --created-by claude

# 2. Alternative: Snippet-based (if range is unknown)
echo "func validateToken" | codemark add --file src/auth.swift --snippet --note "Validates JWT tokens" --note "Critical for security" --collection login-flow

# 3. Last Resort: Raw tree-sitter query (for extreme precision/disambiguation)
codemark add --file src/auth.swift --query '(function_declaration) @target' --note "Token validation function" --collection login-flow

# Notes = durable code explanation; comments = markdown discussion for this task/ticket
codemark add --file src/auth.rs --range 42 \
  --note "Auth entry point; verifies signature" \
  --comment "Touched while debugging **TICKET-42** — see [PR #116](https://github.com/org/repo/pull/116). Suspect race on token refresh." \
  --collection login-flow

# Read the whole collection back as markdown (description, tags, links, steps + notes)
codemark tour show login-flow --format markdown
```

### Method 1: Range-based bookmarking (Primary Resource)
When you know the file and line numbers. This leverages the **Anchored Precision** engine to automatically generate stable, architecturally-anchored queries.

```bash
# Point targeting: targets the smallest node at line 42, column 10
codemark add --file src/auth.rs --range 42:10 --note "Specific authorization gate" --created-by claude

# Line targeting: targets the tightest node containing line 42
codemark add --file src/auth.rs --range 42 --note "Core auth entry point" --created-by claude

# Span targeting: targets a specific range of lines
codemark add --file src/auth.rs --range 42-67 --note "Full login method" --created-by claude

# Precise Span targeting: targets a specific range of characters
codemark add --file src/auth.rs --range 10:5-10:20 --note "Specific expression" --created-by claude
```

### Method 2: Snippet-based bookmarking
When you have the code snippet but need to find it in a file:

```bash
echo "func validateToken(_ token: String) -> Claims" | codemark add --file src/auth.swift --snippet --note "Validates JWT tokens" --tag role:validator --created-by claude
```

### Method 3: Raw tree-sitter query (Last Resort)
Use this **only** if range-based targeting is ambiguous or you need a highly specific non-positional pattern.

```bash
codemark add --file src/auth.swift --query '(function_declaration name: (simple_identifier) @name (#eq? @name "validateToken")) @target' --note "Validates JWT tokens" --created-by claude
```

For common query patterns across languages, see:
- `queries/swift.md`
- `queries/rust.md`
- `queries/typescript.md`
- `queries/python.md`
- `queries/go.md`
- `queries/java.md`
- `queries/csharp.md`
- `queries/dart.md`
- `queries/common.md` — Cross-language patterns and strategies

## Best Practices

- **Target small, specific code**: Prefer single functions over entire classes, specific methods over whole modules. Smaller bookmarks are more resilient to refactoring, easier to understand, and more precise for navigation.
- **Range-First Policy**: **Always prefer range-based targeting** (`--range`) as your first choice. It leverages the unified "Anchored Precision" engine to generate stable, architecturally-anchored queries automatically. Only use raw tree-sitter queries (`--query`) as a last resort if range-based targeting is ambiguous.
- **Fine-grained execution targeting**: Don't just bookmark containers (classes/functions). Bookmark **execution boundaries** like critical method calls (`call_expression`), authorization gates (`if_statement`), or complex state transitions (`switch`/`match`).
- **Granularity Strategy**: We prefer **more, highly specific bookmarks** over fewer, general ones. For example, if a function performs three critical steps (auth, validation, DB write), it is better to bookmark those three specific lines/nodes than to bookmark the entire function.
- Use `--created-by <your-agent-slug>` (`claude`, `codex`, `copilot`, `pi`) to distinguish your bookmarks from the user's — never the generic `agent`. See **Author Identity**.
- Use `--collection <name>` when creating bookmarks to add them directly to a tour (tours are auto-created if they don't exist).
- **Notes are durable, comments are ephemeral**: explain the code with `--note`; record task/ticket/debugging chatter with `--comment`. This keeps notes reusable in any future context instead of coupled to one agentic session.
- **Link out generously**: attach related PRs, issues, design docs, dashboards, and external references to the *collection* with `codemark tour link add --kind ...`, and embed inline links inside `--comment` markdown. This turns a collection into a self-contained briefing.
- **Tag collections, not just bookmarks**: `codemark tour tag add` (or `--tag` at create time) makes collections discoverable and gives the markdown overview useful metadata.
- **Gather context fast**: `codemark tour show <name> --format markdown` renders the description, tags, links, and every step's ID + note in one shot — use it when restoring context from a previous session.
- **Iterative enhancement**: When working with an existing bookmark and discover new context, use `edit` to add notes without re-parsing the file. Multiple agents can edit the same bookmark over time.
- For detailed note guidelines, see `templates.md`.
- For usage examples, see `examples.md`.
- For tree-sitter query patterns, see `queries/` directory for language-specific guides.

## Commands Reference

### Creating bookmarks
```bash
# By range (line or byte) — optionally add to tour
codemark add --file src/auth.rs --range 42-67 --collection my-work

# By snippet (reads from stdin) — optionally add to tour
echo "snippet here" | codemark add --file src/auth.rs --snippet --collection my-work

# By raw tree-sitter query (most precise) — optionally add to tour
codemark add --file src/auth.rs --query '(function_declaration) @target' --collection my-work

# With multiple notes (each creates a separate annotation entry)
codemark add --file src/auth.rs --range 42-67 --note "Primary observation" --note "Secondary note" --note "Action item" --collection my-work

# With a markdown comment for the current task/ticket (repeatable, kept separate from notes)
codemark add --file src/auth.rs --range 42 --note "Validates the session cookie" --comment "Part of **ENG-42** session-fixation fix." --collection my-work

# Preview what would be bookmarked (dry-run)
codemark add --file src/auth.rs --range 42 --dry-run
```

> **Notes vs. comments**: `--note` is durable, reusable code explanation (rendered as plain prose). `--comment` is markdown discussion tied to a task/ticket/PR/debugging session (rendered as markdown, so links/lists/code blocks work). Keep task chatter in comments so notes stay clean and reusable across sessions.

### Viewing bookmarks
```bash
# Show bookmark with preview (default output is JSON)
codemark show <bookmark-id>

# Human-readable markdown (notes, comments, metadata, resolution history)
codemark show <bookmark-id> --format markdown

# Show only file location (was 'resolve')
codemark show <bookmark-id> --location
```
The global `--format` flag accepts `json` (default), `table`, `line`, `markdown`, or a custom template, and can also be set via `CODEMARK_FORMAT`.

### Load context
```bash
codemark list --health active
codemark list --author claude   # substitute your own agent slug to find your bookmarks
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

### Organize with tours (collections)

Tag and link your collections — not just individual bookmarks. A well-tagged collection with links to the relevant PR, issue, and docs is far easier to rediscover and gives the next agent everything it needs.

```bash
# Tours are auto-created when using --collection, or create explicitly.
# Use --description for the collection's prose summary (collections have no --note).
codemark tour create login-flow --description "Step-by-step login execution path"

# Create with description, tags, and links in one command
codemark tour create auth-flow \
  --description "End-to-end authentication flow" \
  --tag feature:auth --tag area:security \
  --link "https://github.com/org/repo/pull/116,Auth refactor PR"

# Tag an existing collection (repeatable)
codemark tour tag add login-flow feature:auth area:security

# Add links with a typed kind: pr | issue | doc | discussion | dashboard | repo | tour | other
codemark tour link add login-flow --url "https://github.com/org/repo/pull/116" --label "Auth PR"   --kind pr
codemark tour link add login-flow --url "https://docs.example.com/auth"        --label "Auth docs" --kind doc
codemark tour link add login-flow --url "https://linear.app/org/issue/ENG-42"  --label "ENG-42"    --kind issue

# Read a collection as markdown — description, tags, links, and each step's ID + note
codemark tour show login-flow --format markdown

# List all tours
codemark tour list

# Add existing bookmarks to a tour
codemark tour add login-flow <bookmark-id-1> <bookmark-id-2>

# Annotate a tour with tags/links (note: --note is NOT persisted on collections)
codemark tour annotate login-flow --tag phase:2 --link "https://example.com/runbook,Runbook"

# Publish / pull from Codetours
codemark tour push login-flow
codemark tour pull <tour-url>
```

### Editing bookmarks
`edit` (and `annotate`) accept repeatable `--note`, `--comment`, and `--tag`, plus `--context`.
```bash
# Add durable notes, context, or tags to an existing bookmark
codemark edit <bookmark-id> --note "Additional context discovered during implementation"
codemark edit <bookmark-id> --context "Related to auth-refactor feature"
codemark edit <bookmark-id> --tag bug-fix --tag priority:high

# Add multiple notes at once (each --note creates a separate annotation)
codemark edit <bookmark-id> --note "First observation" --note "Second observation" --note "Third insight"

# Add a markdown comment scoped to the current task/ticket (kept separate from durable notes)
codemark edit <bookmark-id> --comment "Debugging **ENG-42**: this lock is held too long; see [trace](https://example.com/t/9)."
```

### Health and maintenance
```bash
# Check bookmark health
codemark health status

# Run resolution (was 'heal')
codemark health check --auto-archive

# Remove old archived bookmarks
codemark health gc --older-than 30d
```

### Data operations
```bash
# Export bookmarks
codemark data export --export-format json

# Import bookmarks
codemark data import backup.json

# Rebuild semantic search embeddings
codemark data reindex
```

### Check impact
```bash
# See which bookmarks are affected by recent changes
codemark tour diff --since HEAD~3
```

## Enriching Bookmark Context

High-quality context makes bookmarks discoverable and useful across sessions. Always enrich bookmarks with information that would help you or another agent find and understand the code later.

### Module/Package Context (Required)

Always include module or package information inferred from the file path. This is critical for finding bookmarks by their location in the codebase.

```bash
# Infer from file path and add as context
codemark add --file src/auth/service.rs --range 42 --context "Module: auth | Package: service" --note "Core JWT validation" --tag module:auth
```

| Language | Path pattern | Module context |
|----------|--------------|----------------|
| Rust | `crates/auth/src/lib.rs` | `crate:auth` |
| Rust | `src/auth/service.rs` | `module:auth` |
| Go | `internal/auth/handler.go` | `package:internal.auth` |
| Python | `app/auth/service.py` | `package:app.auth` |
| TypeScript | `src/auth/service.ts` | `module:auth` |
| Java | `com/app/auth/AuthService.java` | `package:com.app.auth` |
| Swift | `Sources/AuthService/` | `module:AuthService` |

### Domain Context

Explain which business domain or bounded context this code belongs to.

```bash
codemark edit <id> --context "Domain: User authentication | Bounded context: Identity and Access Management"
```

### Usage Context

Document where and how this code is used.

```bash
codemark edit <id> --context "Used by: API middleware, websocket handler, cron jobs"
```

### Evolution Context

Track the lifecycle and evolution of the code.

```bash
codemark edit <id> --context "Added in: v2.3.0 | Deprecated in: v3.0.0 | Replacement: src/auth/v2/"
```

### Dependency Context

Link to related bookmarks and dependencies.

```bash
codemark edit <id> --context "Depends on: [[bookmark-id-claims]] | Called by: [[bookmark-id-middleware]]"
```

### Performance Context

Note performance characteristics when relevant.

```bash
codemark edit <id> --context "Performance: O(n) where n = user roles | Cache: 30s TTL"
```

### Security Context

For security-sensitive code, document security considerations.

```bash
codemark edit <id> --context "Security: Validates JWT signature | Checks expiry | Handles token rotation"
```

### Context Template

When creating or editing bookmarks, use this template:

```
Context: <domain> | <usage> | <dependencies> | <security/evolution notes>
```

Examples:
- `Context: Auth domain | Validates all API requests | Depends on Claims struct`
- `Context: Payment processing | Called by checkout flow | External: Stripe API`
- `Context: User preferences | Cached 30s | DB: users_table`

## Supported languages
Swift, Rust, TypeScript, Python, Go, Java, C#, Dart.
