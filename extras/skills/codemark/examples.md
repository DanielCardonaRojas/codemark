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
```
