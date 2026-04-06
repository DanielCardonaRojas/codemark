# Codemark: Implementation Phases

## Phase 1: Core Engine (MVP)

**Goal**: A working CLI that can create bookmarks, resolve them, and track health. Single-language support (Swift).

### Phase 1a: CLI Skeleton

**Goal**: Complete CLI interface with all subcommands, flags, help text, and output scaffolding. Commands return stub responses. This validates the API design before building internals.

#### Deliverables
1. **Project scaffolding**: Cargo workspace, CI (`cargo clippy`, `cargo test`), rustfmt config.
2. **clap CLI definition**: All subcommands and flags as defined in the CLI specification:
   - `add`, `add-from-snippet`, `resolve`, `show`, `list`, `status`, `validate`, `preview`
   - `search`, `diff`, `gc`, `export`, `import`
   - `collection` subcommand tree: `create`, `delete`, `add`, `remove`, `list`, `show`, `resolve`
   - Global options: `--db`, `--format`, `--verbose`
3. **Output framework**: JSON output by default (`{success, data, errors, metadata}`), `--format line` tab-separated output, `--format table` for human readability.
4. **Shell completions**: Generated via `clap_complete` for bash, zsh, fish.
5. **Error types**: Unified error type via `thiserror`, mapping to exit codes (0/1/2/3).
6. **Stub handlers**: Every command parses args, prints `--help` correctly, and returns a structured "not implemented" response with the correct output format.
7. **Tool integration assets**: Ship `extras/tv-channel-bookmarks.toml` for television.

#### Acceptance Criteria
- `codemark --help` and `codemark <subcommand> --help` render correct documentation for every command.
- `codemark add --file foo --range 0:10 --lang swift` returns `{"success": false, "error": "not implemented"}` with exit code 1.
- `codemark collection --help` shows all collection subcommands.
- `codemark list` outputs JSON by default, `--format table` produces human-readable output, `--format line` produces tab-separated output.
- Shell completions generate without errors.
- `cargo clippy` and `cargo test` pass.

#### Estimated Scope
~500–800 lines of Rust.

### Phase 1b: Storage & Core Engine

**Goal**: SQLite storage layer and tree-sitter engine, testable independently from the CLI.

#### Deliverables
1. **SQLite storage layer**: Schema creation, migrations, bookmark CRUD, resolution history CRUD, collection CRUD.
2. **Tree-sitter integration**: Parse Swift files, walk ASTs, run queries against parsed trees.
3. **Query generator**: Given a target node, produce Tier 1 (exact), Tier 2 (relaxed), and Tier 3 (minimal) queries.
4. **Resolution engine**: Exact → relaxed → hash fallback pipeline. Content hash computation with whitespace normalization.
5. **Health state machine**: Status transitions (active → drifted → stale → archived) with bidirectional recovery.
6. **Git context**: Detect repo root, capture HEAD commit on creation.

#### Acceptance Criteria
- Unit tests: query generation produces valid tree-sitter queries for Swift functions, classes, and methods.
- Unit tests: resolution engine correctly falls through tiers when code is modified.
- Unit tests: content hash is stable across whitespace-only changes.
- Unit tests: status transitions follow the state machine.
- Integration tests: SQLite schema creates cleanly, migrations are idempotent, CRUD operations work.
- Benchmark: single-file parse + query < 50ms on a 5K-line Swift file.

#### Estimated Scope
~1,500–2,000 lines of Rust.

### Phase 1c: Wire CLI to Engine

**Goal**: Connect the stub CLI handlers to the real engine. All commands become functional.

#### Deliverables
1. **Wire all commands**: Replace stub handlers with real engine calls.
2. **Preview command**: Thin wrapper — resolves the bookmark, prints metadata header, shells out to `bat` for syntax-highlighted code display (falls back to `cat -n` if bat is not installed). `--show-query` flag prints the stored tree-sitter query.
3. **Collection commands**: Full CRUD wired to storage, `--collection` filter on `list`/`resolve`/`validate`/`search`.
4. **Output formatting**: Populate all output modes (table, line, JSON) with real data from the engine.

#### Acceptance Criteria
- End-to-end: `add` → modify file → `resolve` returns the correct location.
- Round-trip: `add` → `resolve` returns the same location (without modifications).
- `validate` correctly transitions bookmarks through active → drifted → stale.
- `codemark list --format line | fzf --preview 'codemark preview {1}'` provides a working interactive browsing experience.
- `codemark preview --show-query <id>` displays the tree-sitter query alongside its resolved code.
- `--format line` produces tab-separated output suitable for fzf/tv/grep.
- Collections: create a collection, add bookmarks, resolve the collection as a unit, delete the collection without affecting bookmarks.
- Benchmark: single-bookmark resolution < 50ms on a 5K-line file.

#### Estimated Scope
~500–700 lines of Rust.

### Phase 1 Total Estimated Scope
~2,500–3,500 lines of Rust.

---

## Phase 2: Multi-Language & Polish

**Goal**: Support TypeScript, Rust, and Python. Improve CLI ergonomics and error messages.

### Deliverables
1. **Language registry**: Pluggable grammar loading, per-language query strategies.
2. **TypeScript/TSX support**: Grammar, query generation, test fixtures.
3. **~~Rust support~~**: Completed in Phase 1 — grammar, `impl` block handling, test fixtures.
4. **Python support**: Grammar, decorator handling, test fixtures.
5. **Full-text search**: `search` command with SQLite FTS5 or LIKE-based search.
6. **Diff command**: `codemark diff --since <commit>` showing affected bookmarks.
7. **~~Export/Import~~**: Completed in Phase 1c — JSON export and import with validation.
8. **~~Garbage collection~~**: Completed in Phase 1c — `gc` command for archived bookmark cleanup.
9. **Error messages**: Contextual suggestions, "did you mean?" for typos.
10. **Custom format templates**: `--format '{file}:{line} # {note}'` for flexible output shaping.
11. **Cross-repository queries**: `--db` flag accepts multiple paths for querying across repo databases. Read commands fan out across all databases; write commands use only the primary. Results annotated with `source` repo name.

### Acceptance Criteria
- Bookmarks created in any supported language resolve correctly after refactors.
- Language-specific edge cases handled (decorators in Python, `impl` blocks in Rust, JSX in TypeScript).
- `diff` correctly identifies bookmarks affected by a commit range.
- `codemark --db repo-a/.codemark/codemark.db --db repo-b/.codemark/codemark.db list` returns merged results with source labels.

### Estimated Scope
~1,500–2,000 additional lines.

---

## Phase 2a: Semantic Search Foundation

**Goal**: Add natural language search capabilities using vector embeddings.

### Deliverables
1. **sqlite-vec integration**: Load the vector search extension via rusqlite.
2. **Embedding storage**: `bookmark_embeddings` virtual table with vector column.
3. **Local embedding model**: Bundle `all-MiniLM-L6-v2` via `candle` or ONNX Runtime.
4. **Semantic search flag**: `codemark search "..." --semantic` for natural language queries.
5. **Reindex command**: `codemark reindex` to generate embeddings for existing bookmarks.
6. **Model configuration**: `.codemark/config.toml` settings for model selection and API keys.

### Acceptance Criteria
- `codemark search "login flow" --semantic` returns relevant bookmarks even without exact keyword matches.
- `codemark reindex` generates embeddings for all bookmarks without them.
- Embeddings are auto-generated on bookmark creation/update.
- Feature works offline with local model (no network required).

### Estimated Scope
~1,000–1,500 additional lines.

See [docs/requirements/10-semantic-search.md](./10-semantic-search.md) for complete specification.

---

## Phase 2b: Enhanced Semantic Features

**Goal**: Improve semantic search quality and add hybrid capabilities.

### Deliverables
1. **OpenAI provider**: Optional `--provider openai` for cloud embeddings.
2. **Hybrid search**: Combine semantic and keyword scores for better results.
3. **Similarity display**: Show relevance scores in verbose output.
4. **Performance optimizations**: Batch embedding, vector caching.
5. **Per-collection semantic search**: Scope semantic queries to collections.

### Acceptance Criteria
- `codemark search "..." --semantic --provider openai` works with `OPENAI_API_KEY`.
- Hybrid search outperforms pure semantic or keyword on common queries.
- Batch reindex of 1K bookmarks completes in < 30 seconds.

### Estimated Scope
~500–800 additional lines.

---

## Phase 3: Hook Integration

**Goal**: Automatic bookmark creation through Claude Code hooks. The note-taking sub-agent.

### Deliverables
1. **Observer mode**: `codemark observe` command that processes hook payloads from stdin.
2. **Heuristic engine**: Rules for deciding what's bookmark-worthy.
3. **Auto-tagging**: Infer tags from file paths and agent context.
4. **Auto-annotation**: Generate notes from tool use context.
5. **Hook configuration template**: `.claude/hooks.json` template for quick setup.
6. **Session commands**: `codemark session start`, `codemark session end` for lifecycle management.
7. **Configuration file**: `.codemark/config.toml` for observer and health settings.
8. **Deduplication**: Don't re-bookmark code that already has an active bookmark.
9. **Agent skill / plugin**: A Claude Code skill (slash command) that wraps common Codemark workflows — loading context at session start, bookmarking during work, managing collections. The skill translates natural language intent (e.g., "remember this function") into Codemark CLI calls, so agents don't need to know the exact CLI syntax. This includes:
   - A `/codemark` slash command for interactive bookmark management
   - A SessionStart hook that auto-loads relevant bookmarks into agent context
   - Prompt snippets that teach agents how to use Codemark effectively

### Acceptance Criteria
- Install hooks, run a Claude Code session, verify bookmarks are automatically created for edited files.
- Bookmarks have meaningful tags and notes without manual annotation.
- Observer overhead < 150ms per hook invocation.
- No duplicate bookmarks for the same code region.
- Agent skill: `/codemark load <collection>` loads bookmark context into the conversation.

### Estimated Scope
~1,000–1,500 additional lines.

---

## Phase 4: Advanced Features

**Goal**: Cross-file search, bookmark relationships, and performance optimization.

### Deliverables
1. **Cross-file resolution**: When a file is moved/renamed, search other files for hash matches.
2. **Bookmark relationships**: "See also" links between related bookmarks — including cross-repo links (e.g., a bookmark in `service-auth` can reference one in `service-api` via `source:id`).
3. **~~Collections~~**: Moved to Phase 1 — foundational for human workflow.
4. **Resolution caching**: Cache recent resolutions to avoid re-parsing unchanged files.
5. **Parallel resolution**: Resolve bookmarks across files concurrently using rayon.
6. **Metrics**: Resolution success rates, average resolution time, cache hit rates.

### Acceptance Criteria
- A bookmark survives a file rename (detected via cross-file hash search).
- Batch resolution of 100 bookmarks completes in < 2 seconds.
- Collections can be loaded in a single command.

---

## Testing Strategy

### Unit Tests
- Query generation: Given an AST node, verify the generated query.
- Query relaxation: Verify each tier produces the expected pattern.
- Content hashing: Verify normalization and hash stability.
- Status transitions: Verify the state machine.

### Integration Tests
- End-to-end: `add` → modify file → `resolve` → verify correct location.
- Per-language fixtures: Known code samples with expected bookmark behavior.
- Git integration: Verify HEAD capture, diff analysis.

### Property Tests
- For any function in a test file, `add` then `resolve` should return the same location (without modifications).
- Content hash should be stable across whitespace-only changes.

### Regression Tests
- Build a corpus of "refactoring scenarios" (rename function, move to different class, extract method) and verify resolution.

### Benchmarks
- Single-file parse + query time.
- Single bookmark resolution time.
- Batch resolution scaling (10, 50, 100, 500 bookmarks).
- Database query time at various sizes (100, 1K, 10K bookmarks).

---

## Release Milestones

| Milestone | Phase | Target     | Key Deliverable                        |
|-----------|-------|------------|----------------------------------------|
| v0.1.0    | 1     | Phase 1    | Core engine, Swift support, basic CLI  |
| v0.2.0    | 2     | Phase 2    | Multi-language, search, diff, cross-repo queries |
| v0.2.5    | 2a    | Phase 2a   | Semantic search with local embeddings  |
| v0.3.0    | 2b    | Phase 2b   | Enhanced semantic features, hybrid search |
| v0.4.0    | 3     | Phase 3    | Hook integration, auto-bookmarking, agent skill |
| v0.5.0    | 4     | Phase 4    | Cross-file, collections, performance   |
| v1.0.0    | —     | Post-Phase 4 | Stable API, documented, battle-tested |

## Non-Goals (Explicitly Out of Scope)

- **IDE plugin**: Codemark is CLI-first. IDE integration is a separate project.
- **Cloud sync**: Bookmark databases are local. Syncing is the user's problem.
- **Collaborative bookmarks**: Each developer/agent has their own database.
- **Natural language queries**: The agent translates NL to `codemark` CLI calls; codemark itself uses structured queries.
- **Code generation**: Codemark finds code, it doesn't write it.
