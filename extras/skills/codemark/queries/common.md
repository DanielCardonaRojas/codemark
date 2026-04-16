# Common Tree-Sitter Query Patterns

This guide provides tree-sitter query patterns and strategies that apply across all supported languages.

## Bookmark Longevity Strategy

Understanding how codemark resolves bookmarks helps you write queries that survive refactoring.

### The 4-Tier Resolution Engine

Codemark uses a cascading resolution strategy:

1. **Exact Query** - Your stored query with all predicates (`#eq?` constraints)
2. **Relaxed Query** - Same structure, but text predicates stripped → matches ANY node of that type
3. **Minimal Query** - Just the target node type → very broad match
4. **Hash Fallback** - Walks ALL named nodes looking for content hash match

### What This Means for Your Queries

| Query Pattern | Survives Rename? | Survives Restructure? | Falls Back To |
|---------------|------------------|----------------------|---------------|
| Function with `#eq? @name "foo"` | ❌ No | ✅ Yes | Hash fallback (if content preserved) |
| Class-qualified method | ❌ No (double failure) | ⚠️ Maybe | Hash fallback |
| Structural signature (types only) | ✅ Yes | ⚠️ Maybe | Hash fallback |

### Strategic Recommendations

1. **Names change often**: Method names are frequently renamed during refactoring. Name-based queries will fail to Tier 1 but may recover via hash fallback if content is similar.

2. **Classes change less often**: Class/struct names are more stable than method names. Including parent context (class name + method name) is more robust than method name alone.

3. **Types change least**: Parameter types, return types, and structural signatures are most stable. When possible, match by signature rather than name.

4. **Content hash is the safety net**: The hash fallback will find your code even if structure changes dramatically, as long as the content is preserved. This is why `--note` context matters—it helps you identify recovered bookmarks.

## Query Syntax Basics

Tree-sitter queries use S-expressions to match AST nodes:

```
(node_type
  field: (child_type) @capture
  (#predicate? @capture "value")
) @target
```

- `node_type`: The AST node type to match
- `field:`: Named field in the parent node
- `@capture`: A capture name for referencing
- `(#predicate? ...)`: Special predicates like `#eq?` for string matching
- `@target`: The capture that codemark uses to identify the bookmark

## Common Capture Predicates

- `#eq? @capture "value"` — Exact string match
- `#match? @capture "regex"` — Regex match
- `#not-eq? @capture "value"` — Negative match
- `#contains? @capture "value"` — Substring match

## Agent Query Strategies

### Choosing Your Bookmarking Method

| Method | Best For | Agent Capability |
|--------|----------|-----------------|
| `codemark add --range` | Humans who know line numbers | ⭐⭐⭐ (generates optimal query) |
| `codemark add-from-snippet` | Quick bookmark when you have code but not location | ⭐⭐ (text search, first match wins) |
| `codemark add-from-query` | Agents with semantic understanding of code | ⭐⭐⭐⭐⭐ (strategic control) |

**Agent Recommendation**: Use `add-from-query` when possible. You can:
- Analyze the code structure to determine the best matching strategy
- Disambiguate between multiple similar functions
- Choose specific vs broad matching based on context
- Generate queries optimized for the specific refactoring scenario

### Always Test with `--dry-run`

Before creating any bookmark, verify your query captures exactly what you intend:

```bash
codemark add-from-query \
  --file src/auth.swift \
  --query '(function_declaration name: (simple_identifier) @name (#eq? @name "validateToken")) @target' \
  --dry-run
```

Check the output:
- `"match_count": 1` — Good, unique match
- `"match_count": 0` — Query doesn't match anything
- `"match_count": >1` — Ambiguous, add more context
- `"unique": true` — Confirms single match
- `"node_type": "function_declaration"` — Confirms right node type
- `"lines": "38-55"` — Verify it's the right location

### Analyze Uniqueness First

Before bookmarking, check if your target is unique:

```bash
# Find all functions with similar names/signatures
codemark add-from-query \
  --file src/auth.swift \
  --query '(function_declaration name: (simple_identifier) @name) @target' \
  --dry-run
```

If `match_count > 1`, you need more context.

### Strategy: Unique Targets

For uniquely named functions, simple queries are sufficient. Always verify first:

```bash
# Test first
codemark add-from-query \
  --file src/auth.swift \
  --query '(function_declaration name: (simple_identifier) @name (#eq? @name "validateToken")) @target' \
  --dry-run

# Then create if match_count: 1
codemark add-from-query \
  --file src/auth.swift \
  --query '(function_declaration name: (simple_identifier) @name (#eq? @name "validateToken")) @target'
```

```
(function_declaration
  name: (simple_identifier) @name
  (#eq? @name "validateToken")) @target
```

### Strategy: Overloaded/Common Names

For methods like `handle()` or `process()` that appear multiple times, include parent context:

```
(class_declaration
  name: (type_identifier) @class
  (#eq? @class "RequestHandler")
  body: (class_body
    (function_declaration
      name: (simple_identifier) @name
      (#eq? @name "handle")) @target))
```

### Strategy: Likely to Be Renamed

If you're bookmarking code that seems like it will be refactored:

1. **Match by structural signature** instead of name
2. **Include stable parent context** (class names change less than method names)
3. **Rely on hash fallback** as your safety net—write good notes

```
# Matches any function taking String and returning Claims
(function_declaration
  parameters: (parameter_list
    (parameter
      (type_annotation
        (type_identifier) @type
        (#eq? @type "String"))))
  return_type: (type_identifier) @ret
  (#eq? @ret "Claims")) @target
```

### Strategy: Bookmark Multiple Related Functions

When a function has siblings you also care about, bookmark them explicitly rather than relying on broad queries:

```bash
# Bookmark all auth validation methods
for method in validateToken refreshToken checkPermission; do
  codemark add-from-query \
    --file src/auth.swift \
    --query "(function_declaration name: (simple_identifier) @name (#eq? @name \"$method\")) @target" \
    --note "Auth validation: $method" \
    --tag feature:auth
done
```

### Structural Signature Matching

Instead of matching by name, match by parameter/return types which change less often:

```
# Function with specific signature (language varies)
(function_declaration
  name: (simple_identifier)
  parameters: (parameter_list
    (parameter
      (type_annotation
        (type_identifier) @param
        (#eq? @param "String"))))
  return_type: (type_identifier) @ret
  (#eq? @ret "Claims")) @target
```

### Parent-Context Queries (More Robust)

Including the parent class/struct makes the query more specific while being less brittle than matching method names:

```
# Any method in AuthService (language varies)
(class_declaration
  name: (type_identifier) @class
  (#eq? @class "AuthService")
  body: (class_body
    (function_declaration) @target))
```

This bookmark will:
- ✅ Survive method renaming (Tier 2 relaxed match + hash disambiguation)
- ✅ Survive method signature changes
- ⚠️ Fail if AuthService is renamed (then falls back to hash)

### Name-Free Pattern Matching

For truly robust bookmarks, match by structural patterns without any names:

```
# Any async function returning Claims (language varies)
(function_declaration
  effect: (simple_identifier) @effect
  (#eq? @effect "async")
  return_type: (type_identifier) @ret
  (#eq? @ret "Claims")) @target
```

### Hybrid: Name + Structural Backup

Write a query that primarily matches by name but has enough structural context to disambiguate at Tier 2:

```
(class_declaration
  name: (type_identifier) @class
  (#eq? @class "AuthService")           ; Primary: class anchor
  body: (class_body
    (function_declaration
      name: (simple_identifier) @fn
      (#eq? @fn "validateToken")         ; Secondary: method name
      parameters: (parameter_list         ; Tertiary: structural hint
        (parameter
          (type_annotation
            (type_identifier) @type
            (#eq? @type "String")))))) @target)
```

This query provides:
- Tier 1: Exact match (class + method + signature)
- Tier 2: Relaxed (class + any method + any signature) → hash disambiguates
- Tier 3: Minimal (any function) → hash disambiguates
- Tier 4: Hash fallback (content search)

## Discovering Node Types

To discover node types for a specific language:

1. Use `codemark add --dry-run` and inspect `node_type` in output
2. Read the language's tree-sitter grammar source
3. Use tree-sitter CLI: `tree-sitter parse file.{ext}`

## Cross-Language Reference

| Concept | Swift | Rust | TypeScript | Python |
|---------|-------|------|------------|--------|
| Function | `function_declaration` | `function_item` | `function_declaration` | `function_definition` |
| Class method | `class_body > function_declaration` | `impl_item > function_item` | `class_body > method_definition` | `class_body > function_definition` |
| Interface | `protocol_declaration` | `trait_item` | `interface_declaration` | `class_definition` |
| Class/Struct | `class_declaration` / `struct_declaration` | `struct_item` | `class_declaration` | `class_definition` |
| Enum | `enum_declaration` | `enum_item` | `enum_declaration` | `class_definition` |
