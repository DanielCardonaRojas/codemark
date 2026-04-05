# Codemark: Query Generation & Resolution

## Overview

Query generation is the core differentiator of Codemark. Instead of storing line numbers, we store **tree-sitter queries** — S-expression patterns that match code by its AST structure. This document specifies how queries are generated from code and how they're used to re-find code later.

## Tree-sitter Query Primer

Tree-sitter queries use an S-expression syntax to match AST patterns:

```scheme
;; Match a function named "validateToken"
(function_declaration
  name: (identifier) @name
  (#eq? @name "validateToken")) @target

;; Match any method inside a class named "AuthMiddleware"
(class_declaration
  name: (type_identifier) @class_name
  (#eq? @class_name "AuthMiddleware")
  body: (class_body
    (method_definition
      name: (property_identifier) @method_name) @target))
```

Key concepts:
- **Named nodes**: `function_declaration`, `identifier` — the meaningful parts of the AST.
- **Field names**: `name:`, `body:` — how child nodes relate to parents.
- **Captures**: `@name`, `@target` — labels for extracting matched nodes.
- **Predicates**: `#eq?`, `#match?` — constraints on captured text.

## Query Generation Algorithm

### Input
- A parsed tree-sitter `Tree` for the file.
- A target node (identified by byte range or snippet matching).

### Step 1: Identify the target node
Find the smallest **named** node that fully contains the byte range. Skip anonymous nodes (punctuation, operators) — they're grammar artifacts, not structural.

### Step 2: Build the structural path
Walk up from the target node to collect structural context:

```
target: function_declaration "validateToken"
  └─ parent: class_body
       └─ parent: class_declaration "AuthMiddleware"
            └─ parent: program (root — stop here)
```

The path is: `program > class_declaration("AuthMiddleware") > class_body > function_declaration("validateToken")`.

### Step 3: Generate the query

**Tier 1 — Exact query**: Full path with field names and text predicates.
```scheme
(class_declaration
  name: (type_identifier) @class_name
  (#eq? @class_name "AuthMiddleware")
  body: (class_body
    (function_declaration
      name: (identifier) @fn_name
      (#eq? @fn_name "validateToken")) @target))
```

**Tier 2 — Relaxed query**: Same structure, text predicates removed.
```scheme
(class_declaration
  body: (class_body
    (function_declaration
      name: (identifier) @fn_name) @target))
```

**Tier 3 — Minimal query**: Just the target node type with its name.
```scheme
(function_declaration
  name: (identifier) @fn_name
  (#eq? @fn_name "validateToken")) @target
```

### Step 4: Validate uniqueness
Run the Tier 1 query against the file. If it matches exactly one node, accept it. If it matches multiple, add additional distinguishing context (sibling index, parameter types, return type).

### Step 5: Store
Store the Tier 1 query as the primary query. The relaxed and minimal tiers are generated on-the-fly during resolution — they don't need storage.

## Resolution Algorithm

```
resolve(bookmark):
    file = read(bookmark.file_path)
    tree = parse(file, bookmark.language)

    # Tier 1: Exact
    matches = run_query(bookmark.query, tree)
    if matches.len() == 1:
        if hash(matches[0].text) == bookmark.content_hash:
            return Resolution(method: "exact", node: matches[0])
        else:
            # Structure matches but content changed
            return Resolution(method: "exact", node: matches[0], hash_mismatch: true)

    # Tier 2: Relaxed
    relaxed = relax_query(bookmark.query)
    matches = run_query(relaxed, tree)
    if matches.len() == 1:
        return Resolution(method: "relaxed", node: matches[0])
    if matches.len() > 1:
        # Multiple matches — try to disambiguate by content hash
        for m in matches:
            if hash(m.text) == bookmark.content_hash:
                return Resolution(method: "relaxed", node: m)

    # Tier 3: Minimal
    minimal = minimize_query(bookmark.query)
    matches = run_query(minimal, tree)
    # Same disambiguation logic as above

    # Tier 4: Hash fallback
    for node in walk_all_named_nodes(tree):
        if hash(node.text) == bookmark.content_hash:
            # Found by content — regenerate query for this location
            new_query = generate_query(tree, node)
            update_bookmark_query(bookmark, new_query, node)
            return Resolution(method: "hash_fallback", node: node)

    # Failed
    return Resolution(method: "failed")
```

## Query Relaxation Rules

| Component          | Exact (Tier 1) | Relaxed (Tier 2) | Minimal (Tier 3) |
|--------------------|-----------------|-------------------|-------------------|
| Node types         | All ancestors   | Immediate parent + target | Target only  |
| Field names        | All             | Parent→target only | Target only      |
| Text predicates    | All             | None              | Target name only  |
| Sibling position   | If needed       | Removed           | Removed           |

## Content Hashing

Content hashes serve as a last-resort identification mechanism.

### Normalization
Before hashing, normalize the text:
1. Strip leading/trailing whitespace from each line.
2. Collapse consecutive blank lines to a single blank line.
3. Replace all whitespace runs with a single space.
4. Encode as UTF-8.

This makes the hash resilient to:
- Indentation changes (reformatting)
- Trailing whitespace additions/removals
- Blank line insertion/removal

### Hash function
SHA-256, stored as `sha256:<first 16 hex chars>` (64-bit truncation is sufficient for file-local uniqueness).

## Snippet Matching (add-from-snippet)

When the input is a code snippet rather than a byte range:

1. Parse both the snippet and the target file.
2. Get the root node type of the snippet's AST.
3. Query the target file for all nodes of that type.
4. For each candidate:
   a. Compare the candidate's text against the snippet text (normalized).
   b. Score by: exact match (1.0), containment (0.8), edit distance ratio (0.0–0.7).
5. Return the highest-scoring candidate above threshold (0.5).
6. If no candidate scores above threshold, fall back to substring search in the raw file text, then find the enclosing AST node.

## Language-Specific Considerations

### Swift
- `function_declaration` vs `function_call_expression` — ensure we bookmark declarations, not calls.
- Protocol conformances appear in `type_annotation` nodes — may need to include these in disambiguation.
- `guard` statements have their own node type; bookmarking inside guards should capture the guard as context.

### TypeScript
- `function_declaration` vs `arrow_function` vs `method_definition` — all are bookmarkable.
- JSX/TSX: `jsx_element` nodes have distinct structure; tag names are identifiers.
- Type annotations are part of the AST; they help with disambiguation.

### Rust
- `function_item` (not `function_declaration`).
- `impl_item` blocks provide strong disambiguation context.
- Macro invocations (`macro_invocation`) may need special handling — their bodies are often opaque to tree-sitter.

### Python
- Indentation-based structure means the AST is heavily nested.
- `decorated_definition` wraps functions with decorators — bookmark the inner function, not the decorator.
- f-strings and comprehensions have complex AST structures.

## Edge Cases

1. **Generated code**: Files with many structurally identical functions (e.g., protobuf stubs). Content hash fallback is essential here.
2. **Minified code**: AST structure is preserved even in minified files, but content hashes will break on reformatting. Query-based resolution handles this well.
3. **Partial files**: If a file has syntax errors, tree-sitter produces an error-recovery tree. Bookmarks in the valid portion should still resolve.
4. **Empty files**: No nodes to match — bookmark creation fails with a clear error.
5. **Binary/non-text files**: Reject at the CLI level before parsing.
