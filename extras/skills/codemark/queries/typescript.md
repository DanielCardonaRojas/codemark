# TypeScript / JavaScript Tree-Sitter Query Patterns

Query patterns for bookmarking TypeScript and JavaScript code using `codemark add-from-query`.

## Quick Reference

| Target | Pattern |
|--------|---------|
| Named function | `(function_declaration name: (identifier) @name (#eq? @name "NAME")) @target` |
| Class method | See "Class method" below |
| Arrow function (const) | See "Arrow function" below |
| Interface | `(interface_declaration name: (type_identifier) @name (#eq? @name "NAME")) @target` |
| Class | `(class_declaration name: (type_identifier) @name (#eq? @name "NAME")) @target` |
| Type alias | `(type_alias_declaration name: (type_identifier) @name (#eq? @name "NAME")) @target` |
| Enum | `(enum_declaration name: (type_identifier) @name (#eq? @name "NAME")) @target` |

## Patterns

### Named Function

```scheme
(function_declaration
  name: (identifier) @name
  (#eq? @name "validateToken")) @target
```

### Class Method

```scheme
(class_declaration
  name: (type_identifier) @class
  (#eq? @class "AuthService")
  body: (class_body
    (method_definition
      name: (property_identifier) @method
      (#eq? @method "validateToken")) @target))
```

### Arrow Function Assigned to Const

```scheme
(lexical_declaration
  (variable_declarator
    name: (identifier) @name
    (#eq? @name "createDefaultAuthService")
    value: (arrow_function) @target))
```

### Interface Declaration

```scheme
(interface_declaration
  name: (type_identifier) @name
  (#eq? @name "Claims")) @target
```

### Type Alias

```scheme
(type_alias_declaration
  name: (type_identifier) @name
  (#eq? @name "AuthProvider")) @target
```

### Class Declaration

```scheme
(class_declaration
  name: (type_identifier) @name
  (#eq? @name "AuthService")) @target
```

### Enum Declaration

```scheme
(enum_declaration
  name: (type_identifier) @name
  (#eq? @name "AuthError")) @target
```

### Exported Function

Any exported function:

```scheme
(export_statement
  (function_declaration)) @target
```

### Async Function

Any async function:

```scheme
(function_declaration
  async: "async") @target
```

### Class Property

```scheme
(class_declaration
  name: (type_identifier) @class
  (#eq? @class "AuthService")
  body: (class_body
    (property_definition
      name: (property_identifier) @name
      (#eq? @name "tokenCache")) @target))
```

### Method with Specific Signature

```scheme
(method_definition
  parameters: (formal_parameters
    (required_parameter
      (type_annotation
        (type_identifier) @type
        (#eq? @type "Claims"))))) @target
```

## Module Tagging

TypeScript uses `module:<name>` based on directory structure or workspace package name:

| File path | Module tag |
|-----------|------------|
| `src/auth/service.ts` | `--tag module:auth` |
| `src/api/handler.ts` | `--tag module:api` |
| `packages/backend/src/db.ts` | `--tag module:backend` |
| `components/auth/Login.tsx` | `--tag module:components.auth` |
| `lib/utils/helpers.ts` | `--tag module:lib.utils` |

## Examples

### Bookmark an Auth Validator

```bash
codemark add-from-query --file src/auth/service.ts --query '(function_declaration name: (identifier) @name (#eq? @name "validateToken")) @target' --note "Core JWT validation. Entry point for all authenticated requests." --context "Module: auth | Validates JWT tokens with expiry check" --tag module:auth --tag feature:auth --tag role:validator --created-by agent
```

### Bookmark a Class Method

```bash
codemark add-from-query --file src/auth/AuthService.ts --query '(class_declaration name: (type_identifier) @class (#eq? @class "AuthService") body: (class_body (method_definition name: (property_identifier) @method (#eq? @method "invalidateCache")) @target))' --note "Clears the JWT token cache" --context "Module: auth | Cache invalidation logic" --tag module:auth --tag feature:auth --tag layer:business --created-by agent
```
