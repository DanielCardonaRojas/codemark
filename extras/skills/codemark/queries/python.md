# Python Tree-Sitter Query Patterns

Query patterns for bookmarking Python code using `codemark add-from-query`.

## Quick Reference

| Target | Pattern |
|--------|---------|
| Function | `(function_definition name: (identifier) @name (#eq? @name "NAME")) @target` |
| Class method | See "Class method" below |
| Decorated function | Same as function (decorators are skipped) |
| Class | `(class_definition name: (identifier) @name (#eq? @name "NAME")) @target` |
| Async function | See "Async function" below |

## Patterns

### Function Definition

```scheme
(function_definition
  name: (identifier) @name
  (#eq? @name "validate_token")) @target
```

### Class Method

```scheme
(class_definition
  name: (identifier) @class
  (#eq? @class "AuthService")
  body: (block
    (function_definition
      name: (identifier) @method
      (#eq? @method "validate_token")) @target))
```

### Decorated Function

Decorators are automatically skipped; this targets the function itself:

```scheme
(function_definition
  name: (identifier) @name
  (#eq? @name "require_auth")) @target
```

### Class Definition

```scheme
(class_definition
  name: (identifier) @name
  (#eq? @name "AuthService")) @target
```

### Async Function

```scheme
(function_definition
  async: "async"
  name: (identifier) @name
  (#eq? @name "fetch_user")) @target
```

### Async Class Method

```scheme
(class_definition
  name: (identifier) @class
  (#eq? @class "AuthService")
  body: (block
    (function_definition
      async: "async"
      name: (identifier) @method
      (#eq? @method "validate_token")) @target))
```

### Function with Specific Parameters

Function with a specific parameter name:

```scheme
(function_definition
  parameters: (parameters
    (identifier) @param
    (#eq? @param "token"))) @target
```

### Method with Decorator

```scheme
(class_definition
  name: (identifier) @class
  (#eq? @class "AuthService")
  body: (block
    (function_definition
      decorators: (decorator
        (identifier) @decorator
        (#eq? @decorator "property"))
      name: (identifier) @method
      (#eq? @method "token_cache")) @target))
```

## Module Tagging

Python uses `package:<path>` with dot notation matching the import path:

| File path | Module tag |
|-----------|------------|
| `app/auth/service.py` | `--tag package:app.auth` |
| `src/backend/db/models.py` | `--tag package:src.backend.db` |
| `tests/test_auth.py` | `--tag package:tests` |
| `auth.py` (root) | `--tag package:auth` |

## Examples

### Bookmark an Auth Validator

```bash
codemark add-from-query \
  --file app/auth/service.py \
  --query '(function_definition name: (identifier) @name (#eq? @name "validate_token")) @target' \
  --note "Core JWT validation. Entry point for all authenticated requests." \
  --context "Package: app.auth | Validates JWT tokens with expiry check" \
  --tag package:app.auth --tag feature:auth --tag role:validator \
  --created-by agent
```

### Bookmark a Class Method

```bash
codemark add-from-query \
  --file app/auth/service.py \
  --query '(class_definition name: (identifier) @class (#eq? @class "AuthService") body: (block (function_definition name: (identifier) @method (#eq? @method "invalidate_cache")) @target))' \
  --note "Clears the JWT token cache" \
  --context "Package: app.auth | Cache invalidation logic" \
  --tag package:app.auth --tag feature:auth --tag layer:business \
  --created-by agent
```
