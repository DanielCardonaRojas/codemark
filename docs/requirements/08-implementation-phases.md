# Codemark: Implementation Phases

## Status Overview

| Phase | Status | Completion |
|-------|--------|------------|
| **Phase 1a** (CLI Skeleton) | ✅ **COMPLETE** | 100% |
| **Phase 1b** (Storage & Core Engine) | ✅ **COMPLETE** | 100% |
| **Phase 1c** (Wire CLI to Engine) | ✅ **COMPLETE** | 100% |
| **Phase 2** (Multi-Language & Polish) | ✅ **COMPLETE** | 100% |
| **Phase 2a** (Semantic Search) | ❌ **NOT STARTED** | 0% |
| **Phase 2b** (Enhanced Semantic) | ❌ **NOT STARTED** | 0% |
| **Phase 3** (Hook Integration) | ❌ **NOT STARTED** | 0% |
| **Phase 4** (Advanced Features) | ⚠️ **PARTIAL** | 30% |

---

## Phase 1: Core Engine (MVP) — ✅ COMPLETE

**Goal**: A working CLI that can create bookmarks, resolve them, and track health.

### Phase 1a: CLI Skeleton — ✅ COMPLETE

All deliverables implemented:
- ✅ Project scaffolding with Cargo workspace
- ✅ All 15 subcommands defined with clap
- ✅ JSON output by default with `--format` support (table, line, tv, custom)
- ✅ Shell completions for bash, zsh, fish
- ✅ Unified error types via `thiserror`
- ✅ Tool integration assets (`extras/tv-channel-bookmarks.toml`)

### Phase 1b: Storage & Core Engine — ✅ COMPLETE

All deliverables implemented:
- ✅ SQLite storage layer with 4 migrations (bookmarks, resolutions, collections, collection_bookmarks)
- ✅ Tree-sitter integration for **8 languages**: Swift, Rust, TypeScript, Python, Go, Java, C#, Dart
- ✅ Query generator with 4-tier resolution (exact → relaxed → minimal → hash fallback)
- ✅ Resolution engine with hash-based disambiguation
- ✅ Health state machine (active → drifted → stale → archived)
- ✅ Git context detection and commit capture

### Phase 1c: Wire CLI to Engine — ✅ COMPLETE

All deliverables implemented:
- ✅ All commands wired to real engine (no stubs remaining)
- ✅ Preview command with multiple resolution selection modes
- ✅ Collection commands full CRUD + reorder
- ✅ All output modes populated with real data

---

## Phase 2: Multi-Language & Polish — ✅ COMPLETE

**Goal**: Support multiple languages, improve CLI ergonomics.

All deliverables implemented:
- ✅ **8 languages supported**: Swift, Rust, TypeScript, Python, Go, Java, C#, Dart
- ✅ Language-specific query generation strategies
- ✅ Full-text search across notes and context with FTS5
- ✅ Diff command with git integration
- ✅ Export/Import (JSON and CSV)
- ✅ Garbage collection for archived bookmarks
- ✅ Custom format templates (`--format '{file}:{line} # {note}'`)
- ✅ Cross-repository queries with source labeling (`--db` flag multiple times)
- ✅ Multi-database support with source annotation

### What's Actually Implemented (beyond original plan):

The implementation exceeded the original Phase 2 scope:

| Feature | Original Plan | Actual Implementation |
|---------|---------------|----------------------|
| Languages | Swift, TS, Rust, Python | + Go, Java, C#, Dart (8 total) |
| Resolution tiers | 3 | 4 (added hash fallback) |
| Cross-repo | Basic merge | Full source labeling + multi-db filtering |

---

## Phase 2a: Semantic Search Foundation — ❌ NOT STARTED

**Goal**: Add natural language search using vector embeddings.

### Deliverables
1. **sqlite-vec integration**: Load vector search extension
2. **Embedding storage**: Virtual table with vector column
3. **Local embedding model**: Bundle `all-MiniLM-L6-v2`
4. **Semantic search flag**: `--semantic` for natural language queries
5. **Reindex command**: Generate embeddings for existing bookmarks
6. **Model configuration**: `.codemark/config.toml` settings

---

## Phase 2b: Enhanced Semantic Features — ❌ NOT STARTED

**Goal**: Improve semantic search quality.

### Deliverables
1. OpenAI provider support
2. Hybrid search (semantic + keyword)
3. Similarity score display
4. Performance optimizations
5. Per-collection semantic search

---

## Phase 3: Hook Integration — ❌ NOT STARTED

**Goal**: Automatic bookmark creation through Claude Code hooks.

### Deliverables
1. Observer mode (`codemark observe`)
2. Heuristic engine for bookmark-worthy code
3. Auto-tagging and auto-annotation
4. Hook configuration template
5. Session lifecycle commands
6. Configuration file
7. Deduplication
8. Agent skill / plugin

---

## Phase 4: Advanced Features — ⚠️ PARTIAL (30%)

**Goal**: Cross-file search, bookmark relationships, performance.

### Status
| Feature | Status |
|---------|--------|
| Cross-file resolution | ❌ Not implemented |
| Bookmark relationships | ❌ Not implemented |
| Collections | ✅ Complete (done in Phase 1) |
| Resolution caching | ❌ Not implemented |
| Parallel resolution | ❌ Not implemented |
| Metrics | ❌ Not implemented |

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

## Testing Strategy — ✅ MOSTLY COMPLETE

### Unit Tests — ✅ COMPLETE
- ✅ Query generation: 1,000+ lines of tests across all languages
- ✅ Query relaxation: All tiers tested
- ✅ Content hashing: Normalization and hash stability verified
- ✅ Status transitions: State machine fully tested
- ✅ Resolution engine: All 4 tiers tested
- ✅ Health transitions: Active → Drifted → Stale → Archived

### Integration Tests — ✅ COMPLETE
- ✅ CLI integration: 645 lines of integration tests
- ✅ End-to-end: `add` → `resolve` round-trip
- ✅ Per-language fixtures: All 8 languages have test fixtures
- ✅ Git integration: HEAD capture, diff analysis
- ✅ Storage layer: CRUD operations, migrations

### Test Coverage Summary
| Module | Tests | Status |
|--------|-------|--------|
| Query Generator | 1,000+ lines | ✅ Complete |
| CLI Integration | 645 lines | ✅ Complete |
| Resolution Engine | Full coverage | ✅ Complete |
| Health/Status | Full coverage | ✅ Complete |
| Storage Layer | Full coverage | ✅ Complete |
| Git Context | Full coverage | ✅ Complete |

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

| Milestone | Phase | Status     | Key Deliverable                        |
|-----------|-------|------------|----------------------------------------|
| v0.1.0    | 1     | ✅ RELEASED| Core engine, 8 languages, full CLI    |
| v0.2.0    | 2     | ✅ RELEASED| Multi-language, search, diff, cross-repo |
| v0.2.5    | 2a    | ❌ PLANNED  | Semantic search with local embeddings  |
| v0.3.0    | 2b    | ❌ PLANNED  | Enhanced semantic features, hybrid search |
| v0.4.0    | 3     | ❌ PLANNED  | Hook integration, auto-bookmarking     |
| v0.5.0    | 4     | ❌ PLANNED  | Cross-file resolution, performance     |
| v1.0.0    | —     | ❌ PLANNED  | Stable API, documented, battle-tested  |

### Current Release: v0.2.0

The v0.2.0 release includes all Phase 1 and Phase 2 features:

- ✅ 8 language support (Swift, Rust, TypeScript, Python, Go, Java, C#, Dart)
- ✅ 4-tier resolution engine with hash fallback
- ✅ Full-text search with FTS5
- ✅ Git integration (diff, commit tracking)
- ✅ Collection management with reorder support
- ✅ Export/Import (JSON and CSV)
- ✅ Cross-repository queries with source labeling
- ✅ JSON-first output format (default for agents)
- ✅ Comprehensive test coverage

## Non-Goals (Explicitly Out of Scope)

- **IDE plugin**: Codemark is CLI-first. IDE integration is a separate project.
- **Cloud sync**: Bookmark databases are local. Syncing is the user's problem.
- **Collaborative bookmarks**: Each developer/agent has their own database.
- **Natural language queries**: The agent translates NL to `codemark` CLI calls; codemark itself uses structured queries.
- **Code generation**: Codemark finds code, it doesn't write it.
