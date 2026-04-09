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

### 3. Add the bookmark with tags
```bash
codemark add --file src/auth.rs --range 42 \
  --note "Core auth validator. entry point for all signed requests. Relationships: depends on Claims struct." \
  --tag feature:auth --tag role:entrypoint --tag layer:logic \
  --created-by agent
```

## Creating Bookmarks with Raw Queries

### 1. Use dry-run to verify query uniqueness
```bash
codemark add-from-query \
  --file src/auth.swift \
  --query '(function_declaration name: (simple_identifier) @name (#eq? @name "validateToken")) @target' \
  --dry-run
```

### 2. Create bookmark with context
```bash
codemark add-from-query \
  --file src/auth.swift \
  --query '(function_declaration name: (simple_identifier) @name (#eq? @name "validateToken")) @target' \
  --note "Validates JWT tokens. checks expiry and cache." \
  --context "Called by API middleware on all authenticated endpoints" \
  --tag feature:auth --tag role:validation \
  --created-by agent
```

### 3. Cross-language query examples
```bash
# Swift
codemark add-from-query --file AuthService.swift \
  --query '(function_declaration name: (simple_identifier) @name (#eq? @name "validateToken")) @target'

# Rust
codemark add-from-query --file auth.rs \
  --query '(function_item name: (identifier) @name (#eq? @name "validate_token")) @target'

# TypeScript
codemark add-from-query --file AuthService.ts \
  --query '(method_definition name: (property_identifier) @name (#eq? @name "validateToken")) @target'

# Python
codemark add-from-query --file auth.py \
  --query '(function_definition name: (identifier) @name (#eq? @name "validate_token")) @target'
```

## Organizing an Ordered Collection

For complex call chains, use ordered collections.

```bash
# 1. Create collection
codemark collection create login-flow --description "Step-by-step login execution path"

# 2. Add bookmarks in execution order
codemark collection add login-flow <id_handler> <id_middleware> <id_db_query>

# 3. Verify order
codemark collection show login-flow
```

## Searching and Filtering

```bash
# Find all auth-related entry points
codemark list --tag feature:auth --tag role:entrypoint

# Search for bookmarks involving "JWT"
codemark search "JWT"

# List bookmarks created by agents
codemark list --author agent
```

## Checking Impact After Changes

```bash
# See what's affected by recent commits
codemark diff --since HEAD~3

# Validate all bookmarks are still healthy
codemark heal
```

## Session Workflow Example

```bash
# 1. Load existing bookmarks at session start
codemark resolve --status active

# 2. During exploration, bookmark critical findings
codemark add-from-query \
  --file src/api/middleware.go \
  --query '(function_declaration name: (identifier) @name (#eq? @name "AuthMiddleware")) @target' \
  --note "Middleware that validates JWT tokens on all protected routes" \
  --tag feature:auth --tag role:middleware \
  --created-by agent

# 3. At session end, validate bookmarks
codemark heal --auto-archive

# 4. Check overall health
codemark status
```
