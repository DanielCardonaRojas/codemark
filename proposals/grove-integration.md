# Proposal: Adopting `grove-core` for Multi-Language Support

## 1. Problem Statement
Adding a new language to the `codemark` project is currently a manual, compile-time effort. To support a new language, we must:

1.  **Add Dependencies**: Explicitly add the specific `tree-sitter-<language>` crate to `Cargo.toml`.
2.  **Manually Classify Nodes**: Update `crates/codemark-core/src/query/classifier.rs` to map the new language's node types to human-readable labels.
3.  **Update Query Logic**: Adjust `query/generator.rs` and `query/summarizer.rs` if the language has unique AST patterns.

This process is rigid, increases binary size, and requires a full recompile for every new language added.

## 2. Proposed Solution: Integrating `grove-core`
We propose integrating `Entelligentsia/grove`—specifically its upcoming library component, `grove-core`—as a core dependency for `codemark`.

`grove` provides structural, byte-precise AST parsing using tree-sitter, but with a critical architectural advantage: **it loads grammars at runtime from a WASM registry.**

### Key Technical Advantages
*   **Runtime Grammar Loading**: Adding a new language becomes a registry entry rather than a code change and recompile.
*   **Shared AST Engine**: `grove` provides a mature, language-agnostic AST parsing engine, which would allow `codemark` to focus on higher-level bookmarking logic rather than maintaining low-level tree-sitter mappings.
*   **Token Efficiency**: `grove`'s architecture is explicitly optimized for token-efficient codebase access, which complements `codemark`'s goal of durable, AI-agent-friendly bookmarks.

## 3. Implementation Path
The prerequisite for this integration is **[Issue #50: Refactor grove into a Cargo Workspace](https://github.com/Entelligentsia/grove/issues/50)**.

Once `grove` refactors its engine into `grove-core`, the integration plan for `codemark` would be:
### Node Classification Logic
Unlike `codemark`'s imperative `classifier.rs` (which hardcodes mapping logic in Rust), `grove` uses a **declarative, data-driven approach**. It uses tree-sitter query files (specifically `tags.scm` in the `registry/` folder) to define what constitutes a "symbol" or "definition" for each language. Adopting `grove-core` would allow `codemark` to move away from maintaining complex imperative code for node classification and instead leverage these declarative query files.

#### Declarative Node Classification via `tags.scm`
A key architectural benefit of `grove` is that it moves node classification from **imperative Rust code** to **declarative Tree-sitter query files** (`tags.scm`).

*   **How it works**: These query files use S-expressions to match AST nodes and assign semantic metadata (like `kind: "function"`) using `#set!` directives.
*   **Comparison**: 
    *   **`codemark` (Current)**: Hardcodes language-specific mapping in `classifier.rs` using large `match` statements. This is rigid and maintenance-heavy.
    *   **`grove` (Proposed)**: Uses a data-driven registry. Adding a language simply requires creating a `tags.scm` query file, decoupling the parsing logic from the engine.

Adopting `grove-core` would allow `codemark` to abandon manual, compile-time node classification and instead leverage this flexible, registry-based approach, drastically simplifying the process of adding and maintaining support for new languages.




1.  **Evaluate `grove-core` API**: Verify `grove-core` exposes the necessary AST parsing and symbol retrieval functions required by `codemark`.
2.  **Dependency Swap**: Replace existing `tree-sitter-<lang>` dependencies in `codemark` with `grove-core`.
3.  **Adapt Core Engine**: Refactor `codemark-core/src/parser/` and query matching logic to utilize `grove-core`'s AST interface.
4.  **Registry Adoption**: Point `codemark` to the `grove` WASM registry for grammar acquisition.

## 4. Impact Comparison

| Feature | Current `codemark` | Proposed `grove` Integration |
| :--- | :--- | :--- |
| **Language Management** | Manual (Compile-time) | Registry-based (Runtime) |
| **Adding New Languages** | Code change + Recompile | Registry Entry only |
| **AST Parsing Logic** | Custom / Manual mapping | Shared, optimized `grove-core` engine |
| **Maintenance Burden** | High (per language) | Low (centralized in `grove`) |

## 5. Recommendation
We recommend monitoring the progress of [Issue #50 in the `grove` repository](https://github.com/Entelligentsia/grove/issues/50). Upon completion, `codemark` should initiate a pilot migration to `grove-core` to drastically reduce the complexity of expanding multi-language support.
