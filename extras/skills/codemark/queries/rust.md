# Rust Tree-Sitter Query Patterns

Query patterns for bookmarking Rust code using `codemark add-from-query`.

## Quick Reference

| Target | Pattern |
|--------|---------|
| Function | `(function_item name: (identifier) @name (#eq? @name "NAME")) @target` |
| Impl method | See "Impl method" below |
| Trait impl method | See "Trait impl method" below |
| Struct | `(struct_item name: (type_identifier) @name (#eq? @name "NAME")) @target` |
| Enum | `(enum_item name: (type_identifier) @name (#eq? @name "NAME")) @target` |
| Trait | `(trait_item name: (type_identifier) @name (#eq? @name "NAME")) @target` |
| Mod | `(mod_item name: (identifier) @name (#eq? @name "NAME")) @target` |

## Patterns

### Function by Name

Top-level or free function:

```scheme
(function_item
  name: (identifier) @name
  (#eq? @name "validate_token")) @target
```

### Impl Method

Method within an impl block:

```scheme
(impl_item
  type: (type_identifier) @type
  (#eq? @type "AuthService")
  body: (declaration_list
    (function_item
      name: (identifier) @method
      (#eq? @method "validate_token")) @target))
```

### Trait Impl for Specific Type

Method from a trait implementation:

```scheme
(impl_item
  type: (type_identifier) @type
  (#eq? @type "AuthService")
  trait: (type_identifier) @trait
  (#eq? @trait "AuthProvider")
  body: (declaration_list
    (function_item
      name: (identifier) @method
      (#eq? @method "validate_token")) @target))
```

### Struct Declaration

```scheme
(struct_item
  name: (type_identifier) @name
  (#eq? @name "Claims")) @target
```

### Enum Declaration

```scheme
(enum_item
  name: (type_identifier) @name
  (#eq? @name "AuthError")) @target
```

### Trait Declaration

```scheme
(trait_item
  name: (type_identifier) @name
  (#eq? @name "AuthProvider")) @target
```

### Module Declaration

```scheme
(mod_item
  name: (identifier) @name
  (#eq? @name "auth")) @target
```

### Public Function

Any public function:

```scheme
(function_item
  visibility: (visibility_modifier
    (priv))) @target
```

### Async Function

Any async function:

```scheme
(function_item
  (async)) @target
```

### Function with Specific Signature

Function taking String and returning Claims:

```scheme
(function_item
  parameters: (parameters
    (parameter
      (type_identifier) @type
      (#eq? @type "String")))
  return_type: (type_identifier) @ret
  (#eq? @ret "Claims")) @target
```

### Impl Block

Entire impl block for a type:

```scheme
(impl_item
  type: (type_identifier) @type
  (#eq? @type "AuthService")) @target
```

## Examples

### Bookmark an Auth Validator

```bash
codemark add-from-query \
  --file src/auth.rs \
  --query '(function_item name: (identifier) @name (#eq? @name "validate_token")) @target' \
  --note "Core JWT validation. Entry point for all authenticated requests." \
  --tag feature:auth --tag role:validator \
  --created-by agent
```

### Bookmark a Method in an Impl

```bash
codemark add-from-query \
  --file src/auth.rs \
  --query '(impl_item type: (type_identifier) @type (#eq? @type "AuthService") body: (declaration_list (function_item name: (identifier) @method (#eq? @method "invalidate_cache")) @target))' \
  --note "Clears the JWT token cache" \
  --tag feature:auth --tag layer:business \
  --created-by agent
```
