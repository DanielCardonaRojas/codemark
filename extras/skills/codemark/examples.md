# Codemark Usage Examples

## Creating a High-Quality Bookmark

This example shows the step-by-step process of creating a bookmark with full context.

### 1. Identify the target and dry-run
```bash
codemark add --file src/auth.rs --range 42 --dry-run
```

**Output snippet:**
```json
{
  "node_type": "function_item",
  "name": "validate_token",
  "lines": "42-55"
}
```

### 2. Formulate the note
Focus on *why* the code matters and its relationships. Avoid repeating information like the file name or node type which is already stored.
- **Good Note**: `Core auth validator. entry point for all signed requests. Relationships: depends on Claims struct.`
- **Avoid**: `[Function: validate_token] in auth.rs` (Redundant).

### 3. Add the bookmark with tags and multiple notes
```bash
# Single note
codemark add --file src/auth.rs --range 42 --note "Core auth validator. entry point for all signed requests. Relationships: depends on Claims struct." --tag feature:auth --tag role:entrypoint --tag layer:business --created-by claude

# Multiple notes (each creates a separate annotation entry)
codemark add --file src/auth.rs --range 42 --note "Core auth validator. entry point for all signed requests." --note "Relationships: depends on Claims struct." --note "Performance: O(1) cache hit rate" --tag feature:auth --tag role:entrypoint --tag layer:business --created-by claude
```

## Targeting Fine-Grained Execution Logic

Instead of bookmarking the entire function, target the specific execution point where critical actions occur.

### Bookmark a specific method call inside a function
```bash
# Target the exact line where the database is updated
codemark add --file src/db/repo.rs --query '(call_expression function: (member_expression property: (property_identifier) @method (#eq? @method "update_user_balance"))) @target' --note "Critical: Database update point for user balances. Relationships: triggered after payment verification." --tag layer:data --tag role:repository --created-by claude
```

## Creating Bookmarks with Raw Queries

### 1. Use dry-run to verify query uniqueness
```bash
codemark add --file src/auth.swift --query '(function_declaration name: (simple_identifier) @name (#eq? @name "validateToken")) @target' --dry-run
```

### 2. Create bookmark with context and multiple notes
```bash
# Context attaches to first note when multiple notes provided
codemark add --file src/auth.swift --query '(function_declaration name: (simple_identifier) @name (#eq? @name "validateToken")) @target' --note "Validates JWT tokens. checks expiry and cache." --context "Called by API middleware on all authenticated endpoints" --note "Returns Claims struct on success" --note "Raises AuthenticationError on failure" --tag feature:auth --tag role:validator --created-by claude
```

### 3. Cross-language query examples
For detailed query patterns per language, see `queries/` directory.
```bash
# Swift - see queries/swift.md
codemark add --file AuthService.swift --query '(function_declaration name: (simple_identifier) @name (#eq? @name "validateToken")) @target'

# Rust - see queries/rust.md
codemark add --file auth.rs --query '(function_item name: (identifier) @name (#eq? @name "validate_token")) @target'

# TypeScript - see queries/typescript.md
codemark add --file AuthService.ts --query '(method_definition name: (property_identifier) @name (#eq? @name "validateToken")) @target'

# Python - see queries/python.md
codemark add --file auth.py --query '(function_definition name: (identifier) @name (#eq? @name "validate_token")) @target'
```

## Organizing an Ordered Collection

For complex call chains, use ordered collections. **Recommended: Use `--collection` when creating bookmarks** — collections are auto-created and bookmarks are added in order.

```bash
# Add bookmarks directly to a collection (collection is auto-created)
# Each bookmark can have multiple notes
codemark add --file src/handler.rs --range 10 --note "HTTP request handler" --note "Entry point for login flow" --collection login-flow

codemark add --file src/middleware.rs --query '(function_item name: (identifier) @name (#eq? @name "validate")) @target' --note "JWT validation middleware" --note "Called before all protected endpoints" --collection login-flow

codemark add --file src/db.rs --query '(function_item name: (identifier) @name (#eq? @name "lookup_user")) @target' --note "Database query for user lookup" --note "Uses connection pool" --note "Returns User struct or NotFound error" --collection login-flow

codemark tour show login-flow --format markdown
```

**Alternative: Create collection first, then add existing bookmarks**
```bash
# Create collection
codemark tour create login-flow --description "Step-by-step login execution path"

# Add existing bookmarks by ID (useful when reorganizing)
codemark tour add login-flow <id_handler> <id_middleware> <id_db_query>

# Verify order
codemark tour show login-flow --format markdown
```

## Searching and Filtering

`list --tag` and `search` filter by a **single** tag — combine with `--health`, `--author`, `--lang`, or `--collection` rather than passing multiple `--tag` flags. To narrow by several concepts at once, use semantic search.

```bash
# Find auth-related bookmarks, narrowed by author/health
codemark list --tag feature:auth --author claude
codemark list --tag role:entrypoint --health active

# Search for bookmarks involving "JWT" (no --tag flag on search)
codemark search "JWT" --lang rust

# List bookmarks authored by a specific agent (substitute your own slug)
codemark list --author claude
```

## Working with Multiple Notes

The `--note` flag is repeatable. Each `--note` creates a separate annotation entry, allowing you to document different aspects of the code independently.

### Adding multiple notes when creating a bookmark
```bash
codemark add --file src/auth.rs --range 42 --note "Primary function: validates JWT tokens" --note "Performance: O(1) with cache" --note "Security: verifies signature and expiry" --tag feature:auth
```

### Adding multiple notes via annotate
```bash
codemark edit <bookmark-id> --note "Discovered during debugging: race condition" --note "Fix: added mutex lock" --note "Related to issue #123"
```

### Combining notes with context
When `--context` is provided alongside multiple notes, it attaches only to the first note:
```bash
codemark add --file src/payment.rs --range 100 --note "Processes payment via Stripe" --context "Part of checkout refactor v2.0" --note "Idempotent: safe to retry" --note "Webhook: handles async confirmation"
```

This creates three annotations:
1. "Processes payment via Stripe" + context "Part of checkout refactor v2.0"
2. "Idempotent: safe to retry" (no context)
3. "Webhook: handles async confirmation" (no context)

### Use cases for multiple notes
- **Separate concerns**: Document behavior, performance, and security as separate notes
- **Progressive discovery**: Add new observations over time without editing previous notes
- **Multi-aspect analysis**: Document different perspectives (e.g., user-facing vs. implementation details)
- **Action items**: Combine documentation with TODOs or follow-up tasks

## Notes vs. Comments

Notes explain the code durably; comments capture task/ticket/debugging discussion as markdown. Keep them separate so notes stay reusable across sessions.

```bash
# Durable note (plain prose) + task-scoped markdown comment
codemark add --file src/auth.rs --range 42 \
  --note "Validates the session cookie before any handler runs" \
  --comment "Touched for **ENG-42** (session fixation). See [PR #128](https://github.com/org/repo/pull/128); needs a regression test." \
  --created-by claude

# Add a follow-up comment later without disturbing the notes
codemark edit <bookmark-id> --comment "Confirmed fixed by rotating the session id on login. Closing ENG-42."
```

## Enriching a Collection (tags + links)

Make collections self-contained briefings: tag them and link the relevant PR, issue, and docs.

```bash
codemark tour create auth-investigation --description "Tracing the JWT auth path for ENG-42"
codemark tour tag add auth-investigation feature:auth area:security
codemark tour link add auth-investigation --url "https://github.com/org/repo/pull/128" --label "Auth fix PR"  --kind pr
codemark tour link add auth-investigation --url "https://linear.app/org/issue/ENG-42" --label "ENG-42"       --kind issue
codemark tour link add auth-investigation --url "https://docs.example.com/auth"       --label "Auth design" --kind doc

# Read it all back as a single markdown briefing
codemark tour show auth-investigation --format markdown
```

## Checking Impact After Changes

```bash
# See what's affected by recent commits
codemark tour diff --since HEAD~3

# Validate all bookmarks are still healthy
codemark health check
```

## Session Workflow Example

```bash
# 1. Load existing bookmarks at session start
codemark list --health active

# 2. During exploration, bookmark critical findings directly to a collection
codemark add --file src/api/middleware.go --query '(function_declaration name: (identifier) @name (#eq? @name "AuthMiddleware")) @target' --note "Middleware that validates JWT tokens on all protected routes" --tag feature:auth --tag role:middleware --created-by claude --collection auth-investigation
codemark add --file src/api/handler.go --query '(function_declaration name: (identifier) @name (#eq? @name "LoginHandler")) @target' --note "HTTP handler for login endpoint" --tag feature:auth --tag role:handler --created-by claude --collection auth-investigation

# 3. At session end, validate bookmarks
codemark health check --auto-archive

# 4. Check overall health
codemark health status

# 5. Review what you've bookmarked in this session
codemark tour show auth-investigation --format markdown
```
