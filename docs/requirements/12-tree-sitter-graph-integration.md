# Phase 5: Tree-sitter Graph Integration

## 1. Objective
Integrate the `tree-sitter-graph` library and its DSL to enable graph-based structural resolution (Intra-file) and dependency subgraph context extraction. This will make Codemark bookmarks highly resilient to complex internal logic changes and provide AI agents with a richer, more relevant context window.

## 2. Background & Motivation
Codemark currently uses a tiered resolution strategy (Exact → Relaxed → Minimal → Hash Fallback). While powerful, the "Hash Fallback" is brittle: if a function's name changes *and* its internal text is significantly modified (preventing a hash match), the bookmark fails to resolve.
Furthermore, when an AI agent requests a bookmark's context, it only receives the text of the isolated AST node.

By using `tree-sitter-graph`, Codemark can:
1.  **Extract a structural signature:** Understand a function not by its hash, but by its "shape" (e.g., "a function taking 2 arguments, containing a `match` statement and returning a `Result`").
2.  **Enrich AI context:** Traverse the generated graph to locate and pull in definitions of local helper functions or constants used within the bookmarked code.

## 3. Scope & Impact

### Key Files & Context
- `Cargo.toml`: Addition of `tree-sitter-graph = "0.11"`.
- `migrations/`: A new migration (e.g., `008_add_graph_topology.sql`) to add a column or table for storing the graph signature/topology hash.
- `queries/`: A new directory structure to host language-specific `.tsg` files alongside existing `.scm` files.
- `src/parser/languages.rs`: Modifications to `ParseCache` to compile and store `tree_sitter_graph::Graph` instances.
- `src/engine/resolution.rs`: Introduction of a new resolution method (`ResolutionMethod::GraphTopology`).
- `src/engine/bookmark.rs`: Updates to the `Bookmark` struct to hold the pre-computed graph topology string.

## 4. Proposed Solution

### A. The TSG DSL Rules
For each supported language, we will write a `codemark.tsg` file. 
Example for Rust:
```tsg
(function_item
  name: (identifier) @func_name
  parameters: (parameters) @params
)
{
  node func_node
  set func_node.type = "function"
  set func_node.name = (source-text @func_name)
  // Extract structural signature
  set func_node.signature_hash = (node-id @func_name) // Simplified logic
}
```

### B. The Graph Resolution Tier
In `src/engine/resolution.rs`, we will introduce a new tier just before or after the Hash Fallback:
1. When a bookmark is created, Codemark generates its graph topology and stores a serialized "Graph Signature" (e.g., a hash of the sorted edges and node types).
2. During resolution, if Exact, Relaxed, and Minimal fail, Codemark builds the graph for the current file.
3. It searches the graph for a subgraph that matches the stored Graph Signature.
4. If found, it resolves the bookmark to that location.

### C. Context Expansion
When generating the output payload (for AI agents or `codemark show`), Codemark will inspect the generated `tree-sitter-graph`. If the bookmarked node has outbound edges to local definitions (e.g., `has_dependency`), those related AST nodes will be appended to the context payload.

## 5. Alternatives Considered
- **Global Cross-File Graph (tree-sitter-stack-graphs):** Considered building a full repository index to track cross-file dependencies. Rejected because it dramatically increases complexity, requires significant SQLite schema rewrites, and deviates from Codemark's fast, single-file primary resolution paradigm. The user explicitly chose the Intra-file approach.

## 6. Implementation Steps

1. **Dependency & Scaffolding**: 
    - Add `tree-sitter-graph`.
    - Create `queries/<lang>/codemark.tsg` for an initial test language (e.g., Rust or Swift).
2. **Parser Integration**:
    - Update the parser module to generate and cache the `Graph` object alongside the standard tree-sitter `Tree`.
3. **Database Schema Update**:
    - Add `008_add_graph_topology.sql` migration.
    - Add `graph_signature: Option<String>` to the `Bookmark` model.
4. **Resolution Engine Update**:
    - Implement the graph traversal logic in `resolution.rs`.
    - Add `ResolutionMethod::GraphTopology`.
5. **Context Generation Update**:
    - Modify the payload generator to traverse graph edges and include local dependencies.
6. **Language Rollout**:
    - Write `.tsg` files for the remaining 7 supported languages.

## 7. Verification & Testing
- **Unit Tests**: Add tests in `resolution.rs` that create a bookmark on a function, completely scramble its internal text and name, but retain its core graph structure (e.g., same number of arguments and internal variable declarations), and assert that the `GraphTopology` resolution tier successfully finds it.
- **Integration Tests**: Verify that `codemark show` correctly includes a local helper function in its output if the bookmarked function calls it.
