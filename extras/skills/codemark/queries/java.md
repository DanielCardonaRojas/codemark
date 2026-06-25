# Java Tree-Sitter Query Patterns

Query patterns for bookmarking Java code using `codemark add --query`.

## Quick Reference

| Target | Pattern |
|--------|---------|
| Method | `(method_declaration name: (identifier) @name (#eq? @name "NAME")) @target` |
| Class method | See "Class method" below |
| Constructor | `(constructor_declaration name: (identifier) @name (#eq? @name "ClassName")) @target` |
| Interface | `(interface_declaration name: (identifier) @name (#eq? @name "NAME")) @target` |
| Class | `(class_declaration name: (identifier) @name (#eq? @name "NAME")) @target` |
| Enum | `(enum_declaration name: (identifier) @name (#eq? @name "NAME")) @target` |

## Patterns

### Method Declaration

```scheme
(method_declaration
  name: (identifier) @name
  (#eq? @name "validateToken")) @target
```

### Class Method

```scheme
(class_declaration
  name: (identifier) @class
  (#eq? @class "AuthService")
  body: (class_body
    (method_declaration
      name: (identifier) @method
      (#eq? @method "validateToken")) @target))
```

### Constructor

```scheme
(constructor_declaration
  name: (identifier) @name
  (#eq? @name "AuthService")) @target
```

### Interface Declaration

```scheme
(interface_declaration
  name: (identifier) @name
  (#eq? @name "AuthProvider")) @target
```

### Class Declaration

```scheme
(class_declaration
  name: (identifier) @name
  (#eq? @name "AuthService")) @target
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
    (access_modifier
      "public"))) @target
```

### Method with Specific Return Type

```scheme
(method_declaration
  name: (identifier) @name
  (#eq? @name "validateToken")
  type: (type_identifier) @ret
  (#eq? @ret "Claims")) @target
```

### Static Method

Any static method:

```scheme
(method_declaration
  modifiers: (modifiers
    (static_modifier))) @target
```

### Method with Specific Parameter

```scheme
(method_declaration
  name: (identifier) @name
  (#eq? @name "validateToken")
  parameters: (formal_parameters
    (formal_parameter
      (type_identifier) @type
      (#eq? @type "String")))) @target
```

### Call Expression (Method Call)

Target where a specific method is invoked:

```scheme
(method_invocation
  name: (identifier) @method
  (#eq? @method "verifyToken")) @target
```

### Switch Statement

Target complex branching logic:

```scheme
(switch_statement
  condition: (parenthesized_expression
    (identifier) @val
    (#eq? @val "action"))) @target
```

### If Statement

Target specific logical decision points:

```scheme
(if_statement
  condition: (parenthesized_expression
    (identifier) @cond
    (#eq? @cond "isAdmin"))) @target
```

## Module Tagging

Java uses `package:<name>` with the full Java package name (dot notation):

| File path | Module tag |
|-----------|------------|
| `com/app/auth/AuthService.java` | `--tag package:com.app.auth` |
| `org/mycompany/api/handler.java` | `--tag package:org.mycompany.api` |
| `app/Main.java` | `--tag package:app` |

## Examples

### Bookmark an Auth Validator

```bash
codemark add --file com/app/auth/AuthService.java --query '(method_declaration name: (identifier) @name (#eq? @name "validateToken")) @target' --note "Core JWT validation. Entry point for all authenticated requests." --context "Package: com.app.auth | Validates JWT tokens with expiry check" --tag package:com.app.auth --tag feature:auth --tag role:validator --created-by claude
```

### Bookmark a Class Method

```bash
codemark add --file com/app/auth/AuthService.java --query '(class_declaration name: (identifier) @class (#eq? @class "AuthService") body: (class_body (method_declaration name: (identifier) @method (#eq? @method "invalidateCache")) @target))' --note "Clears the JWT token cache" --context "Package: com.app.auth | Cache invalidation logic" --tag package:com.app.auth --tag feature:auth --tag layer:business --created-by claude
```
