# Note & Context Templates

When adding a bookmark, follow these templates to ensure high-quality context that survives session transitions.

## 1. Note Template (Summary)
The `--note` should be concise and focus on the purpose of the code. **Do not repeat the node name or type**, as these are already captured by `codemark`.

**Format:** `<Action/Role>: <Brief Description>`
- **<Action/Role>**: The primary function or role of this code (e.g., Auth Validator, Configuration, Entry point).
- **<Brief Description>**: Why this piece of code is important or what it does.

*Example:* `Auth Validator: Handles all JWT verification for incoming requests.`

## 2. Context Template (Extended)
Include rationale and relationships within the note to provide deeper context.

**Format:** `Context: <Why this matters> | Relationships: <How it relates to other bookmarks>`

*Example:* `Core auth validator. Context: entry point for all signed requests. Relationships: depends on Claims struct.`

### Module/Package Context (Required)
Always include the module or package information inferred from the file path. Each language ecosystem has its own conventions.

**Examples:**
```bash
# Swift - Module from SPM target
codemark add --file Sources/AuthService/Validator.swift --range 10 \
  --context "Module: AuthService | JWT validation logic" \
  --tag module:AuthService --tag feature:auth

# Rust - Workspace crate
codemark add --file crates/auth/src/lib.rs --range 10 \
  --context "Crate: auth | Provides JWT validation" \
  --tag crate:auth --tag feature:auth

# Rust - Module within src/
codemark add --file src/auth/service.rs --range 42 \
  --context "Module: auth | User authentication service" \
  --tag module:auth --tag feature:auth

# Go - Package path
codemark add --file internal/auth/handler.go --range 25 \
  --context "Package: internal/auth | HTTP handlers for authentication" \
  --tag package:internal.auth --tag layer:api

# Python - Package path (dot notation)
codemark add --file app/auth/service.py --range 42 \
  --context "Package: app.auth | Business logic for user authentication" \
  --tag package:app.auth --tag layer:business

# TypeScript - Module from directory
codemark add --file src/auth/service.ts --range 15 \
  --context "Module: auth | Authentication service" \
  --tag module:auth --tag feature:auth

# Java - Package name (dot notation)
codemark add --file com/app/auth/AuthService.java --range 30 \
  --context "Package: com.app.auth | Authentication service implementation" \
  --tag package:com.app.auth --tag layer:business

# C# - Namespace
codemark add --file App/Auth/Services/AuthService.cs --range 20 \
  --context "Namespace: App.Auth.Services | Authentication service" \
  --tag namespace:App.Auth.Services --tag layer:business

# Dart - Package name
codemark add --file lib/auth/service.dart --range 10 \
  --context "Package: auth | Authentication service" \
  --tag package:auth --tag feature:auth
```

### Additional Context Categories

**Domain Context:**
```
Context: Domain: User authentication | Bounded context: Identity and Access Management
```

**Usage Context:**
```
Context: Used by: API middleware, websocket handler, cron jobs
```

**Evolution Context:**
```
Context: Added in: v2.3.0 | Deprecated in: v3.0.0 | Replacement: src/auth/v2/
```

**Risk Context:**
```
Context: Risk: High | Change impact: Affects all authenticated routes
```

**Dependency Context:**
```
Context: Depends on: [[bookmark-id-claims]] | Called by: [[bookmark-id-middleware]]
```

**Performance Context:**
```
Context: Performance: O(n) where n = user roles | Cache: 30s TTL
```

**Security Context:**
```
Context: Security: Validates JWT signature | Checks expiry | Handles token rotation
```

## 3. Tagging Strategy
Use structured, colon-prefixed tags for powerful filtering. Each language uses its own module/package convention.

### Module Tags (Always Include)

**Swift:** `module:<name>` — SPM target/module name
```bash
--tag module:AuthService
```

**Rust:** `crate:<name>` for workspace crates, `module:<name>` for src/ modules
```bash
--tag crate:auth          # crates/auth/
--tag module:auth         # src/auth/
```

**Go:** `package:<path>` — Full package path with dots
```bash
--tag package:internal.auth
--tag package:pkg.api
--tag package:cmd.server
```

**Python:** `package:<path>` — Python package path (dot notation)
```bash
--tag package:app.auth
--tag package:src.backend.db
```

**TypeScript/JavaScript:** `module:<name>` — Directory or workspace package
```bash
--tag module:auth
--tag module:backend
--tag module:components.auth
```

**Java:** `package:<name>` — Java package name (dot notation)
```bash
--tag package:com.app.auth
--tag package:org.mycompany.api
```

**C#:** `namespace:<name>` — C# namespace
```bash
--tag namespace:App.Auth.Services
```

**Dart:** `package:<name>` — Dart package name
```bash
--tag package:auth
--tag package:models
```

### Feature Tags
- `feature:<name>` — e.g., `feature:auth`, `feature:logging`
- `domain:<name>` — e.g., `domain:user-management`, `domain:analytics`

### Architectural Layer Tags
- `layer:<name>` — e.g., `layer:api`, `layer:business`, `layer:data`, `layer:infra`, `layer:ui`

### Role Tags
- `role:<name>` — e.g., `role:entrypoint`, `role:handler`, `role:service`, `role:repository`

### Other Tags
- `type:<name>` — e.g., `type:function`, `type:class`
- `status:<name>` — e.g., `status:active`, `status:deprecated`
- `task:<id>` — e.g., `task:fix-123`, `task:refactor`
- `risk:<level>` — e.g., `risk:high`, `risk:medium`, `risk:low`
- `security:<type>` — e.g., `security:auth`, `security:crypto`

*Example:* `--tag module:auth --tag feature:auth --tag layer:api --tag role:entrypoint --tag security:auth`
