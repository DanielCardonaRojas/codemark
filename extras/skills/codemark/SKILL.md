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
codemark list --status active
```

## Agent Directives: Handling Invocations
If the user invoked you with arguments (e.g. `/codemark something`), use `$ARGUMENTS` to search the active bookmarks or execute the desired behavior:
- Search text: `codemark search "$ARGUMENTS"`
- If it looks like a bookmark ID or UUID prefix: `codemark show "$ARGUMENTS"`
- If user asks to "open", "edit", or view a bookmark: `codemark open "$ARGUMENTS"`
- If no arguments were provided, interpret the user's intent based on the conversation context.

## Intelligent Bookmark Discovery

When the user asks about something, try these strategies in order:

### 1. Semantic Search (Most Flexible)
Best for natural language queries and conceptual searches:
```bash
codemark search "user query" --semantic
```

### 2. Tag-Based Filtering
When the user mentions specific concepts or categories:
```bash
codemark list --tag feature:auth --tag role:entrypoint
codemark list --tag layer:api --status active
codemark list --tag type:config --author agent
```

### 3. File-Aware Search
When discussing specific files or modules:
```bash
codemark list --file src/auth.rs --status active
codemark list --file src/auth.rs --lang rust
```

### 4. Collection Browsing
When exploring a feature or workflow:
```bash
codemark collection list
codemark collection show <collection-name>
```

### 5. Hybrid Search Pattern
For best results, combine semantic search with filters:
```bash
codemark search "authentication" --lang rust --author agent
codemark search "middleware" --tag layer:api --status active
```

### Discovery Strategy Summary
| User Query Type | Primary Strategy | Fallback |
|-----------------|------------------|----------|
| "Show my auth bookmarks" | `--tag feature:auth` | semantic search |
| "Where's the config?" | `--tag type:config` | `--tag role:config` |
| "Bookmarks in auth.rs" | `--file src/auth.rs` | semantic search |
| "Recent agent bookmarks" | `--author agent` | semantic search |
| "What did I mark for X?" | semantic search | collection browse |

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

### Lifecycle Tags
Identify the state or lifecycle of the bookmarked code.
- `status:active` — Currently in use
- `status:deprecated` — Deprecated, planned for removal
- `status:experimental` — Experimental, may change
- `status:stable` — Stable, unlikely to change

### Task/Work Tags
Link bookmarks to specific tasks or work items.
- `task:<id>` — e.g., `task:fix-123`, `task:refactor-auth`
- `pr:<number>` — Associated pull request
- `issue:<number>` — Associated issue

### Risk Tags
Identify risk level or sensitivity.
- `risk:high` — High risk, changes require careful review
- `risk:medium` — Medium risk
- `risk:low` — Low risk, safe to modify

### Security Tags
Identify security-sensitive code.
- `security:auth` — Authentication, authorization
- `security:crypto` — Encryption, hashing, cryptography
- `security:validation` — Input validation, sanitization
- `security:sensitive` — Accesses sensitive data

### Recommended Tag Combinations
```bash
# Auth entry point
--tag feature:auth --tag layer:api --tag role:entrypoint --tag security:auth

# Database query
--tag feature:users --tag layer:data --tag role:repository

# Business logic
--tag feature:payments --tag layer:business --tag role:service --tag risk:high

# Configuration
--tag layer:config --tag role:constant --tag status:stable
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
codemark add --file crates/auth/src/lib.rs --range 10 --note "Core JWT validation" --tag crate:auth --tag feature:auth --tag layer:business --tag role:validator
```

**Example (Go package):**
```bash
codemark add --file internal/auth/handler.go --range 25 --note "HTTP handler for authentication" --tag package:internal.auth --tag feature:auth --tag layer:api --tag role:handler
```

## Quick Start

### Creating a collection of bookmarks (recommended for agents)
Use `--collection` when creating bookmarks to add them directly to a collection:

```bash
codemark add --file src/auth.rs --range 42 --note "Core auth entry point: Handles all JWT verification" --tag feature:auth --tag role:entrypoint --created-by agent --collection login-flow
codemark add-from-query --file src/auth.swift --query '(function_declaration name: (simple_identifier) @name (#eq? @name "validateToken")) @target' --note "Validates JWT tokens" --created-by agent --collection login-flow
codemark collection show login-flow
```

### Method 1: Range-based bookmarking
When you know the file and line numbers:

```bash
codemark add --file src/auth.rs --range 42 --note "Core auth entry point: Handles all JWT verification" --context "Module: auth | Validates all JWT tokens for API requests" --tag module:auth --tag feature:auth --tag role:entrypoint --created-by agent
```

### Method 2: Snippet-based bookmarking
When you have the code snippet but need to find it in a file:

```bash
echo "func validateToken(_ token: String) -> Claims" | codemark add-from-snippet --file src/auth.swift --note "Validates JWT tokens" --context "Module: auth | JWT validation with expiry check" --tag module:auth --tag feature:auth --tag role:validator --created-by agent
```

### Method 3: Raw tree-sitter query (recommended for agents)
When you want precise control over what gets bookmarked:

```bash
codemark add-from-query --file src/auth.swift --query '(function_declaration name: (simple_identifier) @name (#eq? @name "validateToken")) @target' --note "Validates JWT tokens" --context "Module: auth | Core validation logic" --tag module:auth --tag feature:auth --tag role:validator --created-by agent
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

- Use `--created-by agent` to distinguish your bookmarks from the user's.
- Use `--collection <name>` when creating bookmarks to add them directly to a collection (collections are auto-created if they don't exist).
- **Iterative enhancement**: When working with an existing bookmark and discover new context, use `annotate` to add notes without re-parsing the file. Multiple agents can annotate the same bookmark over time.
- For detailed note guidelines, see `templates.md`.
- For usage examples, see `examples.md`.
- For tree-sitter query patterns, see `queries/` directory for language-specific guides.

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
# Add notes, context, or tags to an existing bookmark
codemark annotate <bookmark-id> --note "Additional context discovered during implementation"
codemark annotate <bookmark-id> --context "Related to auth-refactor feature"
codemark annotate <bookmark-id> --tag bug-fix --tag priority:high

# Add annotation as an agent with provenance
codemark annotate <bookmark-id> --note "Found this during debugging the session timeout issue" --added-by agent --source investigation
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
codemark annotate <id> --context "Domain: User authentication | Bounded context: Identity and Access Management"
```

### Usage Context

Document where and how this code is used.

```bash
codemark annotate <id> --context "Used by: API middleware, websocket handler, cron jobs"
```

### Evolution Context

Track the lifecycle and evolution of the code.

```bash
codemark annotate <id> --context "Added in: v2.3.0 | Deprecated in: v3.0.0 | Replacement: src/auth/v2/"
```

### Risk Context

Document risk level and change impact.

```bash
codemark annotate <id> --context "Risk: High | Change impact: Affects all authenticated routes | Requires: Security review"
```

### Dependency Context

Link to related bookmarks and dependencies.

```bash
codemark annotate <id> --context "Depends on: [[bookmark-id-claims]] | Called by: [[bookmark-id-middleware]]"
```

### Performance Context

Note performance characteristics when relevant.

```bash
codemark annotate <id> --context "Performance: O(n) where n = user roles | Cache: 30s TTL"
```

### Security Context

For security-sensitive code, document security considerations.

```bash
codemark annotate <id> --context "Security: Validates JWT signature | Checks expiry | Handles token rotation"
```

### Context Template

When creating or annotating bookmarks, use this template:

```
Context: <domain> | <usage> | <dependencies> | <risk/security/evolution notes>
```

Examples:
- `Context: Auth domain | Validates all API requests | Depends on Claims struct | Risk: high`
- `Context: Payment processing | Called by checkout flow | External: Stripe API | Risk: critical`
- `Context: User preferences | Cached 30s | DB: users_table | Low risk`

### Maintenance
```bash
codemark heal --auto-archive
codemark status
```

## Supported languages
Swift, Rust, TypeScript, Python, Go, Java, C#, Dart.
