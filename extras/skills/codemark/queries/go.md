# Go Tree-Sitter Query Patterns

Query patterns for bookmarking Go code using `codemark add-from-query`.

## Quick Reference

| Target | Pattern |
|--------|---------|
| Function | `(function_declaration name: (identifier) @name (#eq? @name "NAME")) @target` |
| Method | See "Method declaration" below (includes receiver) |
| Struct type | See "Type declaration (struct)" below |
| Interface type | See "Interface declaration" below |

## Patterns

### Function Declaration

```scheme
(function_declaration
  name: (identifier) @name
  (#eq? @name "ValidateToken")) @target
```

### Method Declaration

Method with receiver (the receiver is part of the function_declaration):

```scheme
(function_declaration
  name: (identifier) @name
  (#eq? @name "ValidateToken")) @target
```

To target a specific type's method, include the receiver:

```scheme
(function_declaration
  receiver: (parameter_list
    (parameter
      (type_identifier) @receiver
      (#eq? @receiver "AuthService")))
  name: (identifier) @name
  (#eq? @name "ValidateToken")) @target
```

### Type Declaration (Struct)

```scheme
(type_declaration
  (type_spec
    name: (type_identifier) @name
    (#eq? @name "Claims")
    type: (struct_type) @target))
```

### Type Declaration (Interface)

```scheme
(type_declaration
  (type_spec
    name: (type_identifier) @name
    (#eq? @name "AuthProvider")
    type: (interface_type) @target))
```

### Type Declaration (Any)

```scheme
(type_declaration
  (type_spec
    name: (type_identifier) @name
    (#eq? @name "Claims"))) @target
```

### Public Function

Any exported (capitalized) function:

```scheme
(function_declaration
  name: (identifier) @name
  (#match? @name "^[A-Z]")) @target
```

### Function with Specific Parameters

```scheme
(function_declaration
  name: (identifier) @name
  (#eq? @name "ValidateToken")
  parameters: (parameter_list
    (parameter
      (type_identifier) @type
      (#eq? @type "string")))) @target
```

## Module Tagging

Go uses `package:<path>` with the full package path relative to module root (using dots):

| File path | Module tag |
|-----------|------------|
| `internal/auth/handler.go` | `--tag package:internal.auth` |
| `pkg/api/middleware.go` | `--tag package:pkg.api` |
| `cmd/server/main.go` | `--tag package:cmd.server` |
| `handler.go` (root) | `--tag package:root` |

## Examples

### Bookmark an Auth Validator

```bash
codemark add-from-query --file internal/auth/validator.go --query '(function_declaration name: (identifier) @name (#eq? @name "ValidateToken")) @target' --note "Core JWT validation. Entry point for all authenticated requests." --context "Package: internal/auth | Validates JWT tokens with expiry check" --tag package:internal.auth --tag feature:auth --tag role:validator --created-by agent
```

### Bookmark a Method

```bash
codemark add-from-query --file internal/auth/cache.go --query '(function_declaration receiver: (parameter_list (parameter (type_identifier) @receiver (#eq? @receiver "AuthService"))) name: (identifier) @name (#eq? @name "InvalidateCache")) @target' --note "Clears the JWT token cache" --context "Package: internal/auth | Cache invalidation logic" --tag package:internal.auth --tag feature:auth --tag layer:business --created-by agent
```
