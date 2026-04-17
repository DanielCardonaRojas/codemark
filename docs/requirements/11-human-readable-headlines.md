# Codemark: Human-Readable Bookmark Headlines

## Overview

Bookmark headlines currently display only a truncated UUID (e.g., "Bookmark: a1b2c3d4"), providing no semantic information about what code the bookmark references. This feature proposes extracting meaningful, human-readable headlines from the tree-sitter AST node data that is already captured during bookmark creation.

## Motivation

### Current State

When users list or show bookmarks, they see non-descriptive identifiers:

```
# Bookmark: a1b2c3d4
File: src/auth/middleware.swift
Status: active
```

Without opening each bookmark, users cannot tell what code it references. This makes browsing bookmarks inefficient and requires additional cognitive overhead.

### Desired State

Bookmarks should display headlines that identify the bookmarked code element:

```
# Bookmark: AuthMiddleware.validateToken(config, options)
File: src/auth/middleware.swift
Status: active
```

Users can immediately understand what each bookmark references without opening it.

## Goals

- **Self-documenting bookmarks**: Headlines should identify the bookmarked code element at a glance
- **No external dependencies**: Use tree-sitter data already captured during query generation
- **Multi-language support**: Work across all supported languages (Swift, Rust, TypeScript, Python, Go, Java, C#, Dart)
- **Backwards compatible**: Existing bookmarks without headlines continue to work; headlines can be regenerated on demand
- **Fallback graceful**: When extraction fails, fall back to ID-based headline

## Non-Goals

- **AI/LLM summarization**: Headlines identify the code element, not describe what it does
- **Code content summarization**: The headline does not attempt to summarize the logic or behavior
- **Natural language generation**: Headlines follow a structured format, not prose
- **Cross-language normalization**: Different languages may have slightly different headline formats

## Functional Requirements

### FR-11.1: Headline Extraction

**Input**: Tree-sitter AST node captured during bookmark creation

**Behavior**:
1. Extract the node's "name" field (function name, class name, method name, variable name, etc.)
2. For methods, extract parent context (class/struct name)
3. For functions/methods, extract parameter names (not types)
4. Format as a concise, human-readable string

**Output**: Headline string stored in bookmark metadata

### FR-11.2: Headline Format by Node Type

| Node Type | Format | Example |
|-----------|--------|---------|
| Function | `functionName(params)` | `parseRequest(config, options)` |
| Method | `ClassName.methodName(params)` | `Parser.parseRequest(source)` |
| Class/Struct | `class ClassName` | `class HttpRequest` |
| Impl Block | `impl ClassName` | `impl Parser` |
| Enum | `enum EnumName` | `enum HttpMethod` |
| Enum Variant | `EnumName.variantName` | `HttpMethod.GET` |
| Trait/Protocol | `protocol ProtocolName` | `protocol RequestHandler` |
| Variable/Const | `let variableName` | `let config` |
| Module | `mod moduleName` | `mod auth` |

**Parameter formatting**:
- Include only parameter names (not types)
- Omit default values
- Truncate if > 3 parameters: `funcName(a, b, ... +2)`

### FR-11.3: Storage

Add a new column to the `bookmarks` table:

```sql
ALTER TABLE bookmarks ADD COLUMN headline TEXT;
```

The headline is:
- Generated on bookmark creation (synchronous)
- Stored alongside existing metadata
- Indexed for faster search
- Nullable for backwards compatibility (old bookmarks have NULL)

### FR-11.4: Template Integration

Update the Handlebars template to use the headline:

```handlebars
# {{#if headline}}{{headline}}{{else}}Bookmark: {{short_id}}{{/if}}
```

Available template variable:
- `{{headline}}` - Extracted headline (or empty if not available)

### FR-11.5: Regeneration Command

**Input**: Optional `--force` flag, optional filter criteria

**Behavior**:
1. Find bookmarks without headlines (or all if `--force` is set)
2. Re-parse the source file at the last known location
3. Re-extract the headline from the matched AST node
4. Update the database record

**CLI**:
```bash
codemark rehead              # Generate headlines for bookmarks without them
codemark rehead --force      # Regenerate all headlines
codemark rehead --tag auth   # Regenerate headlines for specific bookmarks
```

### FR-11.6: Fallback Behavior

When headline extraction fails:
- **Node has no name**: Use node type + line number (e.g., `statement at line 42`)
- **Parsing fails**: Fall back to ID-based headline (e.g., `Bookmark: a1b2c3d4`)
- **Bookmark is stale**: Display `Stale bookmark: {short_id}`

### FR-11.7: Line Output Format

Update the `--format line` output to include the headline:

```
<id>\t<headline>\t<file>:<line>\t<status>\t<tags>
```

Example:
```
a1b2c3d4	AuthMiddleware.validateToken	src/auth/middleware.swift:42	active	auth,jwt
```

## Technical Approach

### Extraction Module

Create a new module `src/query/headline.rs`:

```rust
/// Extract a human-readable headline from a tree-sitter node
pub fn extract_headline(
    node: &Node,
    source: &[u8],
    language: &Language,
) -> Option<String>

/// Extract the name from a node (function, class, variable, etc.)
fn extract_name(node: &Node, source: &[u8]) -> Option<String>

/// Extract parent context for methods (class/struct name)
fn extract_parent_context(node: &Node, source: &[u8]) -> Option<String>

/// Extract parameter names from function/method definitions
fn extract_parameters(node: &Node, source: &[u8]) -> Vec<String>
```

### Language-Specific Extraction

Each supported language may have slightly different AST structures. Use the existing `src/parser/languages.rs` pattern for language-specific handling:

```rust
impl HeadlineExtractor for SwiftLanguage {
    fn extract_function_name(&self, node: &Node) -> Option<String> { ... }
    fn extract_method_context(&self, node: &Node) -> Option<String> { ... }
    fn extract_parameters(&self, node: &Node) -> Vec<String> { ... }
}
```

### Integration Points

1. **Query Generator** (`src/query/generator.rs`): Extract headline after capturing the target node
2. **Bookmark Creation** (`src/cli/handlers.rs`): Include headline in bookmark metadata
3. **Database** (`src/storage/bookmark_repo.rs`): Add `headline` field to CRUD operations
4. **Templates** (`src/cli/templates.rs`): Use `{{headline}}` in markdown output
5. **Output Formatting** (`src/cli/output.rs`): Include headline in table/line formats

## Data Model Changes

### Bookmark Table

```sql
CREATE TABLE bookmarks (
    id TEXT PRIMARY KEY,
    file_path TEXT NOT NULL,
    language TEXT NOT NULL,
    query TEXT NOT NULL,
    node_type TEXT,
    line_range TEXT,              -- "start:end"
    content_hash TEXT,

    -- New field
    headline TEXT,

    -- Existing fields
    status TEXT NOT NULL DEFAULT 'active',
    tags TEXT,                    -- JSON array
    notes TEXT,
    context TEXT,
    created_by TEXT,
    created_at TEXT NOT NULL,
    commit_hash TEXT,
    last_resolved_at TEXT,
    resolution_method TEXT,
    stale_since TEXT
);

-- Index for headline search
CREATE INDEX idx_bookmarks_headline ON bookmarks(headline);
```

## Implementation Phases

### Phase 1: Core Extraction

**Deliverables**:
1. `src/query/headline.rs` module with extraction logic
2. Integration with query generator
3. Database migration for `headline` column
4. Updated markdown template

**Acceptance Criteria**:
- New bookmarks have meaningful headlines
- Template displays headline instead of ID
- Fallback to ID when extraction fails

**Estimated Scope**: ~400–600 lines of Rust

### Phase 2: Rehead Command

**Deliverables**:
1. `codemark rehead` CLI command
2. Batch headline regeneration
3. Progress reporting

**Acceptance Criteria**:
- `codemark rehead` generates headlines for existing bookmarks
- `--force` flag regenerates all headlines
- Filters (`--tag`, `--status`) work correctly

**Estimated Scope**: ~200–300 lines of Rust

### Phase 3: Output Format Updates

**Deliverables**:
1. Update `--format line` to include headline
2. Update `--format table` to show headline column
3. Make headline column optional/compact

**Acceptance Criteria**:
- Line format shows headline before file:line
- Table format has headline column (truncated if too long)
- `--no-headline` flag to hide headlines

**Estimated Scope**: ~100–200 lines of Rust

## Testing Strategy

### Unit Tests

- Extraction produces correct headline for each node type
- Parameter name extraction handles edge cases (variadic, defaults, generics)
- Parent context extraction works for nested structures
- Fallback behavior for unnamed nodes

### Integration Tests

- End-to-end: `codemark add` creates bookmark with headline
- Rehead: `codemark rehead` updates existing bookmarks
- Template: Markdown output displays headline correctly

### Language-Specific Tests

For each supported language (Swift, Rust, TypeScript, Python, Go, Java, C#, Dart):
- Test function headline extraction
- Test method headline extraction
- Test class/struct headline extraction
- Test language-specific edge cases

## Edge Cases

1. **Anonymous functions**: Use lambda location (e.g., `lambda at line 42`)
2. **Overloaded methods**: Include parameter types for disambiguation (e.g., `parse(String)`, `parse(Bytes)`)
3. **Generic functions**: Include generic parameters (e.g., `parse<T>(source)`)
4. **Nested functions**: Include outer function name (e.g., `outer_func > inner_func`)
5. **Macro-generated code**: May not extract cleanly; fall back to node type
6. **Multi-statement selections**: Use first named node or range (e.g., `statements at lines 10-15`)

## Open Questions

1. **Should parameter types be included for disambiguation?**
   - Pro: Helps distinguish overloaded methods
   - Con: Makes headlines longer
   - Decision: Exclude by default, include only for overloads

2. **Should headlines be searchable?**
   - Pro: Find bookmarks by function name
   - Con: May collide with other search terms
   - Decision: Index headline, add `--headline` filter to search command

3. **Should headlines update on resolution?**
   - Pro: Headline stays in sync if function is renamed
   - Con: Loses original context if bookmark drifts
   - Decision: Do not auto-update; manual `rehead` only

## References

- Current query generation: `src/query/generator.rs`
- Tree-sitter node documentation: [tree-sitter.github.io/tree-sitter/using-parsers#query-syntax](https://tree-sitter.github.io/tree-sitter/using-parsers#query-syntax)
- Template system: `docs/TEMPLATE_DESIGN.md`
