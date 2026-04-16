# Swift Tree-Sitter Query Patterns

Query patterns for bookmarking Swift code using `codemark add-from-query`.

## Quick Reference

| Target | Pattern |
|--------|---------|
| Top-level function | `(function_declaration name: (simple_identifier) @name (#eq? @name "NAME")) @target` |
| Class method | See "Class method" below |
| Extension method | See "Extension method" below |
| Protocol | `(protocol_declaration name: (type_identifier) @name (#eq? @name "NAME")) @target` |
| Struct | `(struct_declaration name: (type_identifier) @name (#eq? @name "NAME")) @target` |
| Class | `(class_declaration name: (type_identifier) @name (#eq? @name "NAME")) @target` |
| Property | `(property_declaration name: (simple_identifier) @name (#eq? @name "NAME")) @target` |

## Patterns

### Function by Name

Top-level or nested function:

```scheme
(function_declaration
  name: (simple_identifier) @name
  (#eq? @name "validateToken")) @target
```

### Class Method

Method within a class:

```scheme
(class_declaration
  name: (type_identifier) @class
  (#eq? @class "AuthService")
  body: (class_body
    (function_declaration
      name: (simple_identifier) @method
      (#eq? @method "validateToken")) @target))
```

### Extension Method

Method within an extension:

```scheme
(extension_declaration
  type: (user_type
    (type_identifier) @type
    (#eq? @type "AuthService"))
  (function_declaration
    name: (simple_identifier) @method
    (#eq? @method "invalidateCache")) @target)
```

### Protocol Declaration

```scheme
(protocol_declaration
  name: (type_identifier) @name
  (#eq? @name "AuthProvider")) @target
```

### Struct Declaration

```scheme
(struct_declaration
  name: (type_identifier) @name
  (#eq? @name "Claims")) @target
```

### Class Declaration

```scheme
(class_declaration
  name: (type_identifier) @name
  (#eq? @name "AuthService")) @target
```

### Property Declaration

```scheme
(property_declaration
  name: (simple_identifier) @name
  (#eq? @name "tokenCache")) @target
```

### Private Method

Any private method:

```scheme
(function_declaration
  modifiers: (modifiers
    (visibility_modifier
      (case_keyword) @vis
      (#eq? @vis "private")))) @target
```

### Async Function

Any async function:

```scheme
(function_declaration
  effect: (simple_identifier) @effect
  (#eq? @effect "async")) @target
```

### Function with Specific Signature

Function taking String and returning Claims:

```scheme
(function_declaration
  parameters: (parameter_list
    (parameter
      (type_annotation
        (type_identifier) @type
        (#eq? @type "String"))))
  return_type: (type_identifier) @ret
  (#eq? @ret "Claims")) @target
```

## Module Tagging

Swift uses `module:<name>` tags based on SPM target names. Infer from `Sources/<ModuleName>/` directory structure:

| File path | Module tag |
|-----------|------------|
| `Sources/AuthService/Validator.swift` | `--tag module:AuthService` |
| `Sources/App/Models/User.swift` | `--tag module:App` |
| `Sources/Networking/Client.swift` | `--tag module:Networking` |

## Examples

### Bookmark an Auth Validator

```bash
codemark add-from-query --file Sources/AuthService/Validator.swift --query '(function_declaration name: (simple_identifier) @name (#eq? @name "validateToken")) @target' --note "Core JWT validation. Entry point for all authenticated requests." --context "Module: AuthService | Validates JWT tokens with expiry check" --tag module:AuthService --tag feature:auth --tag role:validator --created-by agent
```

### Bookmark a Class Method

```bash
codemark add-from-query --file Sources/AuthService/Cache.swift --query '(class_declaration name: (type_identifier) @class (#eq? @class "AuthService") body: (class_body (function_declaration name: (simple_identifier) @method (#eq? @method "invalidateCache")) @target))' --note "Clears the JWT token cache" --context "Module: AuthService | Cache invalidation logic" --tag module:AuthService --tag feature:auth --tag layer:business --created-by agent
```
