# C# Tree-Sitter Query Patterns

Query patterns for bookmarking C# code using `codemark add-from-query`.

## Quick Reference

| Target | Pattern |
|--------|---------|
| Method | `(method_declaration name: (identifier) @name (#eq? @name "NAME")) @target` |
| Class | `(class_declaration name: (identifier) @name (#eq? @name "NAME")) @target` |
| Property | `(property_declaration name: (identifier) @name (#eq? @name "NAME")) @target` |
| Interface | `(interface_declaration name: (identifier) @name (#eq? @name "NAME")) @target` |
| Struct | `(struct_declaration name: (identifier) @name (#eq? @name "NAME")) @target` |
| Enum | `(enum_declaration name: (identifier) @name (#eq? @name "NAME")) @target` |

## Patterns

### Method Declaration

```scheme
(method_declaration
  name: (identifier) @name
  (#eq? @name "ValidateToken")) @target
```

### Class Declaration

```scheme
(class_declaration
  name: (identifier) @name
  (#eq? @name "AuthService")) @target
```

### Property Declaration

```scheme
(property_declaration
  name: (identifier) @name
  (#eq? @name "Secret")) @target
```

### Interface Declaration

```scheme
(interface_declaration
  name: (identifier) @name
  (#eq? @name "IAuthProvider")) @target
```

### Struct Declaration

```scheme
(struct_declaration
  name: (identifier) @name
  (#eq? @name "Claims")) @target
```

### Enum Declaration

```scheme
(enum_declaration
  name: (identifier) @name
  (#eq? @name "AuthError")) @target
```

### Public Method

Any public method:

```scheme
(method_declaration
  modifiers: (modifiers
    (access_modifier_modifier
      "public"))) @target
```

### Async Method

Any async method:

```scheme
(method_declaration
  modifiers: (modifiers
    (async_modifier))) @target
```

### Method with Specific Return Type

```scheme
(method_declaration
  name: (identifier) @name
  (#eq? @name "ValidateToken")
  type: (predefined_type) @ret
  (#eq? @ret "Task")) @target
```

### Class Method

```scheme
(class_declaration
  name: (identifier) @class
  (#eq? @class "AuthService")
  body: (declaration_list
    (method_declaration
      name: (identifier) @method
      (#eq? @method "InvalidateCache")) @target))
```

## Module Tagging

C# uses `namespace:<name>` with the full C# namespace:

| File path | Module tag |
|-----------|------------|
| `App/Auth/Services/AuthService.cs` | `--tag namespace:App.Auth.Services` |
| `MyCompany/Data/Repositories/UserRepo.cs` | `--tag namespace:MyCompany.Data.Repositories` |
| `Auth/Validator.cs` | `--tag namespace:Auth` |

## Examples

### Bookmark an Auth Validator

```bash
codemark add-from-query \
  --file App/Auth/Services/AuthService.cs \
  --query '(method_declaration name: (identifier) @name (#eq? @name "ValidateToken")) @target' \
  --note "Core JWT validation. Entry point for all authenticated requests." \
  --context "Namespace: App.Auth.Services | Validates JWT tokens with expiry check" \
  --tag namespace:App.Auth.Services --tag feature:auth --tag role:validator \
  --created-by agent
```

### Bookmark a Class Method

```bash
codemark add-from-query \
  --file App/Auth/Services/AuthService.cs \
  --query '(class_declaration name: (identifier) @class (#eq? @class "AuthService") body: (declaration_list (method_declaration name: (identifier) @method (#eq? @method "InvalidateCache")) @target))' \
  --note "Clears the JWT token cache" \
  --context "Namespace: App.Auth.Services | Cache invalidation logic" \
  --tag namespace:App.Auth.Services --tag feature:auth --tag layer:business \
  --created-by agent
```
