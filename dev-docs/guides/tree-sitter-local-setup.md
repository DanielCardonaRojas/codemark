# Tree-sitter Local Development Guide

This guide walks through setting up tree-sitter locally to experiment with parsing, AST inspection, and query development — the core operations that power Codemark's structural bookmarking.

## 1. Install the CLI

Pick one method:

```bash
# Homebrew (macOS/Linux) — recommended
brew install tree-sitter-cli

# Cargo (any platform with Rust)
cargo install tree-sitter-cli --locked

# npm (pre-built binaries)
npm install -g tree-sitter-cli
```

> **Note**: `brew install tree-sitter` installs the C library, not the CLI. You need `tree-sitter-cli`.

You also need a **C/C++ compiler** (Xcode command line tools on macOS) and **Node.js** (grammars are defined in JavaScript).

Verify:
```bash
tree-sitter --version
```

## 2. Configure Parser Directories

```bash
tree-sitter init-config
```

This creates `~/.config/tree-sitter/config.json`. Edit it to point at a directory where you'll clone grammars:

```json
{
  "parser-directories": [
    "/Users/you/tree-sitter-parsers"
  ]
}
```

Any subdirectory named `tree-sitter-*` inside these paths is auto-detected as a grammar.

## 3. Install Language Parsers

Clone the grammars Codemark will support:

```bash
mkdir -p ~/tree-sitter-parsers
cd ~/tree-sitter-parsers

git clone https://github.com/alex-pinkus/tree-sitter-swift.git
git clone https://github.com/tree-sitter/tree-sitter-typescript.git
git clone https://github.com/tree-sitter/tree-sitter-rust.git
git clone https://github.com/tree-sitter/tree-sitter-python.git
```

Verify the CLI detects them:

```bash
tree-sitter dump-languages
```

This lists all detected languages with their file extensions. If a language is missing, check that the repo contains a `tree-sitter.json` file and that your `parser-directories` config is correct.

## 4. Parse a File and Inspect the AST

This is the starting point for all query development — you need to see the AST node types and field names.

```bash
# S-expression output
tree-sitter parse path/to/file.swift

# Pretty-printed Concrete Syntax Tree (colored, easier to read)
tree-sitter parse --cst path/to/file.swift

# Parse from stdin (specify language with --scope)
echo 'func hello() { }' | tree-sitter parse --scope source.swift

# Restrict to a specific grammar path
tree-sitter parse -p ~/tree-sitter-parsers/tree-sitter-swift file.swift
```

**Example output** for a Swift file:

```
(source_file
  (function_declaration
    name: (simple_identifier)
    body: (function_body
      (statements
        (call_expression
          (simple_identifier)
          (call_suffix
            (value_arguments
              (value_argument
                (line_string_literal
                  (line_str_text))))))))))
```

The node types (`function_declaration`, `simple_identifier`) and field names (`name:`, `body:`) are what you use in queries.

## 5. Write and Test Queries

### Query syntax basics

Create a `.scm` file with S-expression patterns. Captures are marked with `@name`:

```scheme
; queries/find-functions.scm
(function_declaration
  name: (simple_identifier) @function.name) @function.def
```

### Run it

```bash
tree-sitter query queries/find-functions.scm path/to/file.swift
```

Output shows each match with the captured text and position:

```
  pattern: 0
    capture: function.name - text: "validateToken" - start: (42, 9) - end: (42, 22)
    capture: function.def - text: "func validateToken..." - start: (42, 4) - end: (67, 5)
```

### Useful flags

```bash
# Group output by capture name (easier to scan)
tree-sitter query --captures queries/find-functions.scm file.swift

# Restrict to a line range
tree-sitter query --row-range 40:70 queries/find-functions.scm file.swift

# Restrict to a byte range
tree-sitter query --byte-range 1024:2048 queries/find-functions.scm file.swift

# Show timing information
tree-sitter query --time queries/find-functions.scm file.swift
```

## 6. Query Syntax Reference

### Matching nodes

```scheme
; Named node
(function_declaration)

; With field names
(function_declaration
  name: (simple_identifier) @name)

; Literal/anonymous nodes (in quotes)
(binary_expression operator: "!=")

; Wildcard: (_) = any named node
(call_expression arguments: (_) @arg)
```

### Predicates

```scheme
; Exact text match
((simple_identifier) @name (#eq? @name "validateToken"))

; Regex match
((simple_identifier) @const (#match? @const "^[A-Z][A-Z_]+$"))

; One of several values
((simple_identifier) @keyword (#any-of? @keyword "self" "super" "nil"))

; Negated: #not-eq?, #not-match?, #not-any-of?
```

### Quantifiers

```scheme
(comment)+ @comments        ; one or more
(decorator)* @decorators    ; zero or more
(parameter)? @optional      ; zero or one
```

### Anchors

```scheme
(array . (identifier) @first)           ; first child
(block (_) @last .)                     ; last child
(identifier) @a . (identifier) @b      ; adjacent siblings
```

### Alternations

```scheme
["break" "continue" "return"] @keyword
[(string_literal) (integer_literal)] @literal
```

### Negated fields

```scheme
; Match functions without type parameters
(function_declaration name: (simple_identifier) @name !type_parameters)
```

## 7. The Playground

### Web playground (no install)

https://tree-sitter.github.io/tree-sitter/playground

Three panels: code editor, live syntax tree, and query editor. Supports Python, Rust, TypeScript, JavaScript, Go, and others — but **not Swift**.

### Local playground (for Swift and custom grammars)

```bash
cd ~/tree-sitter-parsers/tree-sitter-swift

# Build the grammar as WASM
tree-sitter build --wasm

# Launch local playground in browser
tree-sitter playground

# Without auto-opening browser
tree-sitter playground --quiet
```

This gives you the same three-panel experience with your custom grammar.

## 8. Syntax Highlighting

Test how tree-sitter highlight queries render:

```bash
# ANSI-colored terminal output
tree-sitter highlight path/to/file.rs

# HTML output
tree-sitter highlight --html path/to/file.py
```

The highlight command uses `queries/highlights.scm` from the grammar repo. You can customize colors in your `config.json` under the `theme` key.

## 9. Practical Workflow for Codemark Development

Here's the recommended loop when developing Codemark's query generation:

### Step 1: Parse the target file

```bash
tree-sitter parse --cst src/auth/middleware.swift
```

Note the exact node types and field names for the code you want to bookmark.

### Step 2: Write a candidate query

Create a `.scm` file that uniquely identifies the target node. Start specific (Tier 1), then relax:

```scheme
; Tier 1: exact — full path with text predicates
(class_declaration
  name: (type_identifier) @_class (#eq? @_class "AuthMiddleware")
  body: (class_body
    (function_declaration
      name: (simple_identifier) @_name (#eq? @_name "validateToken")) @target))

; Tier 2: relaxed — remove text predicates
(class_declaration
  body: (class_body
    (function_declaration
      name: (simple_identifier) @_name) @target))

; Tier 3: minimal — node type only
(function_declaration) @target
```

### Step 3: Test each tier

```bash
# Should match exactly one node
tree-sitter query tier1.scm src/auth/middleware.swift

# Should match a small set
tree-sitter query tier2.scm src/auth/middleware.swift

# Will match many — last resort
tree-sitter query tier3.scm src/auth/middleware.swift
```

### Step 4: Modify the source and re-test

Rename the function, add lines above it, move it to a different class — then re-run the queries to verify resolution behavior:

```bash
# Does the Tier 1 query still find it?
tree-sitter query tier1.scm src/auth/middleware.swift

# After rename, Tier 1 fails but Tier 2 should succeed
tree-sitter query tier2.scm src/auth/middleware.swift
```

### Step 5: Compare across languages

The same structural pattern may use different node types per language:

```bash
# Swift function
tree-sitter parse --cst sample.swift | head -20

# TypeScript function
tree-sitter parse --cst sample.ts | head -20

# Rust function
tree-sitter parse --cst sample.rs | head -20

# Python function
tree-sitter parse --cst sample.py | head -20
```

This is critical for building Codemark's per-language query generation strategies.

## 10. Example: Full Round-Trip

```bash
# 1. Create a test Swift file
cat > /tmp/test.swift << 'EOF'
class AuthService {
    func validateToken(_ token: String) -> Bool {
        guard let claims = decode(token) else {
            return false
        }
        return claims.expiry > Date()
    }

    func decode(_ token: String) -> Claims? {
        return try? JSONDecoder().decode(Claims.self, from: Data(token.utf8))
    }
}
EOF

# 2. Parse and inspect
tree-sitter parse --cst /tmp/test.swift

# 3. Write a query to find validateToken
cat > /tmp/find-validate.scm << 'EOF'
(class_declaration
  name: (type_identifier) @class.name
  body: (class_body
    (function_declaration
      name: (simple_identifier) @func.name
      (#eq? @func.name "validateToken")) @func.def))
EOF

# 4. Run the query
tree-sitter query /tmp/find-validate.scm /tmp/test.swift

# 5. Verify it matches exactly one function
tree-sitter query --captures /tmp/find-validate.scm /tmp/test.swift
```

## Troubleshooting

| Problem | Solution |
|---------|----------|
| `language not found` | Run `tree-sitter dump-languages` to check detection. Verify `parser-directories` in config. |
| `failed to load shared library` | The grammar needs compiling. Run `tree-sitter generate` inside the grammar repo, or ensure a C compiler is available. |
| Query returns no matches | Check node types with `tree-sitter parse --cst`. Node names vary across grammar versions. |
| Wrong language detected | Use `--scope source.swift` or `-p path/to/grammar` to force a specific language. |
| Swift not in web playground | Use the local playground: `tree-sitter build --wasm && tree-sitter playground` inside the Swift grammar repo. |
