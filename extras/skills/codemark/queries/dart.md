# Dart Tree-Sitter Query Patterns

Query patterns for bookmarking Dart code using `codemark add-from-query`.

## Quick Reference

| Target | Pattern |
|--------|---------|
| Function | `(function_signature name: (simple_identifier) @name (#eq? @name "NAME")) @target` |
| Class | `(class_definition name: (class_name) @name (#eq? @name "NAME")) @target` |
| Method | See "Class method" below |
| Enum | `(enum_declaration name: (enum_name) @name (#eq? @name "NAME")) @target` |

## Patterns

### Function Signature

Top-level or method function:

```scheme
(function_signature
  name: (simple_identifier) @name
  (#eq? @name "validateToken")) @target
```

### Class Definition

```scheme
(class_definition
  name: (class_name) @name
  (#eq? @name "AuthService")) @target
```

### Enum Declaration

```scheme
(enum_declaration
  name: (enum_name) @name
  (#eq? @name "AuthError")) @target
```

### Class Method

```scheme
(class_definition
  name: (class_name) @class
  (#eq? @class "AuthService")
  body: (class_body
    (method_signature
      name: (simple_identifier) @method
      (#eq? @method "validateToken")) @target))
```

### Async Function

Any async function:

```scheme
(function_signature
  async: "async") @target
```

### Function with Specific Return Type

```scheme
(function_signature
  name: (simple_identifier) @name
  (#eq? @name "validateToken")
  return_type: (type_identifier) @ret
  (#eq? @ret "Claims")) @target
```

### Extension Method

```scheme
(extension_declaration
  name: (type_identifier) @type
  (#eq? @type "String")
  (method_signature
    name: (simple_identifier) @method
    (#eq? @method "parseToken")) @target)
```

## Examples

### Bookmark an Auth Validator

```bash
codemark add-from-query \
  --file src/auth.dart \
  --query '(function_signature name: (simple_identifier) @name (#eq? @name "validateToken")) @target' \
  --note "Core JWT validation. Entry point for all authenticated requests." \
  --tag feature:auth --tag role:validator \
  --created-by agent
```

### Bookmark a Class Method

```bash
codemark add-from-query \
  --file src/auth.dart \
  --query '(class_definition name: (class_name) @class (#eq? @class "AuthService") body: (class_body (method_signature name: (simple_identifier) @method (#eq? @method "invalidateCache")) @target))' \
  --note "Clears the JWT token cache" \
  --tag feature:auth --tag layer:business \
  --created-by agent
```
