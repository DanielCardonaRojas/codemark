# Test Fixtures

Source files for testing Codemark's tree-sitter query generation and resolution. Each file is designed to exercise specific AST patterns and edge cases. These files are parsed by tree-sitter — they don't need to compile.

Comments starting with `// Target:` (or `# Target:` for Python) mark specific code regions intended for bookmark testing.

```
fixtures/
├── swift/          # Phase 1
├── typescript/     # Phase 2 (future)
├── rust/           # Phase 2 (future)
└── python/         # Phase 2 (future)
```

## Swift (Phase 1)

### `swift/auth_service.swift`
| Pattern | Target |
|---------|--------|
| Method inside a class | `validateToken(_:)` |
| Private method | `decode(_:)` |
| Method with complex params | `checkPermission(_:required:resource:)` |
| Method inside extension | `invalidateCache()` |
| Top-level free function | `createDefaultAuthService()` |
| Protocol declaration | `AuthProvider` |
| Struct with Codable | `Claims` |
| Enum with associated values | `AuthError` |

### `swift/api_client.swift`
| Pattern | Target |
|---------|--------|
| Class with initializer | `APIClient.init(config:authService:)` |
| Async throwing method | `send(_:)` |
| Private retry logic | `sendWithRetry(_:attempts:)` |
| Generic method | `sendAndDecode(_:as:)` |
| Extension shorthand methods | `get(_:)`, `post(_:body:)` |
| Static property | `APIConfig.default` |
| Enum with raw values | `HTTPMethod` |
| Nested struct hierarchy | `APIConfig`, `APIRequest`, `APIResponse` |

### `swift/data_models.swift`
| Pattern | Target |
|---------|--------|
| Deeply nested type (4 levels) | `User.Profile.Settings.Theme` |
| Constrained protocol extension | `Identifiable.matches(_:)` |
| Computed property | `Pagination.totalPages` |
| Closure parameter | `Pagination.mapItems(_:transform:)` |
| Guard-heavy function | `parseUserInput(json:)` |
| Overloaded functions (4 variants) | `format(_:)` — tests query disambiguation |

## Testing Workflow

### Manual: parse and inspect AST
```bash
tree-sitter parse tests/fixtures/swift/auth_service.swift --cst
```

### Manual: test a query against a fixture
```bash
cat > /tmp/query.scm << 'EOF'
(class_declaration
  name: (type_identifier) @cls (#eq? @cls "AuthService")
  (class_body
    (function_declaration
      name: (simple_identifier) @fn (#eq? @fn "validateToken")) @target))
EOF

tree-sitter query /tmp/query.scm tests/fixtures/swift/auth_service.swift
```

### In Rust tests
```rust
let source = include_str!("../fixtures/swift/auth_service.swift");
// parse with tree-sitter, generate queries, test resolution
```
