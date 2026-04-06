# Codemark: Semantic Search

## Overview

Semantic search enables users to find bookmarks using natural language queries rather than exact keyword matches. Instead of searching for "auth", users can ask "Where is the login logic?" or "How do we validate tokens?" and get relevant results even when those exact words don't appear in the bookmark's notes or tags.

## Motivation

The current `codemark search` command (FR-4.3) provides full-text search across `notes` and `context` fields using SQLite FTS or LIKE matching. This has limitations:

1. **Vocabulary mismatch**: Users may not remember the exact words used when creating a bookmark
2. **Concept vs keyword**: Searching for "user authentication" won't find a bookmark annotated with "login flow"
3. **Intent-based queries**: Users want to ask questions like "Where's the error handling for network failures?" rather than guessing keywords

Semantic search addresses these by encoding the *meaning* of bookmark metadata into vector embeddings, allowing similarity-based retrieval.

## Goals

- **Natural language queries**: Find bookmarks using questions and descriptive phrases
- **Conceptual matching**: Results based on semantic similarity, not keyword overlap
- **Privacy-first**: Support local embedding models; no required cloud dependencies
- **SQLite-native**: Use `sqlite-vec` for zero-config vector search within the existing database
- **Backwards compatible**: Semantic search is an optional enhancement; existing keyword search continues to work

## Non-Goals

- **Code content embeddings**: Initial version embeds only metadata (tags, notes, context). Embedding the actual code snippets is deferred.
- **Cloud-only operation**: The feature must work entirely offline with local embedding models
- **Re-ranking infrastructure**: No separate re-ranking stage beyond vector similarity
- **Multi-modal embeddings**: No image or diagram embeddings

## Technical Approach

### Vector Storage: `sqlite-vec`

**`sqlite-vec`** (by Alex Garcia) is a pure C SQLite extension for vector similarity search. It replaces the older `sqlite-vss` and offers:

- **Zero dependencies**: No C++ libraries like Faiss
- **Portability**: Compiles anywhere SQLite runs (macOS, Linux, Windows, iOS, Android, WASM)
- **Native vector types**: `float32`, `int8`, `bit` vectors
- **SIMD acceleration**: Hardware-accelerated nearest-neighbor search

### Data Model Changes

```sql
-- New table for storing embeddings (created in migration)
CREATE VIRTUAL TABLE bookmark_embeddings USING vec0(
    bookmark_id TEXT PRIMARY KEY REFERENCES bookmarks(id) ON DELETE CASCADE,
    embedding FLOAT[384]  -- Size depends on model; all-MiniLM-L6-v2 is 384
);

-- Trigger to auto-generate embeddings on insert/update
CREATE TRIGGER bookmark_embedding_insert
AFTER INSERT ON bookmarks
BEGIN
    -- Application generates embedding and inserts into bookmark_embeddings
END;

CREATE TRIGGER bookmark_embedding_update
AFTER UPDATE OF notes, context, tags ON bookmarks
BEGIN
    -- Regenerate embedding when metadata changes
END;
```

### Embedding Source Text

The text to be embedded is concatenated from bookmark metadata:

```
Tags: {tags}
Note: {notes}
Context: {context}
```

Example input:
```
Tags: auth, middleware, jwt
Note: JWT validation entry point for API requests
Context: Fixing token expiration bug in authentication flow
```

### Embedding Models

| Model | Dimensions | Size | When to Use |
|-------|------------|------|-------------|
| `all-MiniLM-L6-v2` | 384 | ~80MB | Default local model |
| `bge-small-en-v1.5` | 384 | ~130MB | Better quality, still fast |
| `text-embedding-3-small` | 1536 | N/A (remote) | OpenAI API option |

### CLI Integration

```bash
# Semantic search (new flag on search command)
codemark search "how are tokens validated" --semantic

# Hybrid: semantic + keyword filter
codemark search "network error handling" --semantic --tag swift

# Limit results
codemark search "database connection" --semantic --limit 5

# JSON output for agents
codemark search "user authentication" --semantic --json
```

## Functional Requirements

### FR-10.1: Embedding Generation

**Input**: Bookmark metadata (tags, notes, context)

**Behavior**:
1. Concatenate metadata into a single searchable string
2. Pass to embedding model (local or remote)
3. Store resulting vector in `bookmark_embeddings` table

**Timing**: Embeddings are generated:
- On bookmark creation (synchronous or async)
- On bookmark update when notes/context/tags change
- On-demand via `codemark reindex` for existing bookmarks

### FR-10.2: Semantic Search

**Input**: Natural language query string, optional filters

**Behavior**:
1. Generate embedding for the query
2. Perform k-nearest-neighbor search via `sqlite-vec`
3. Return results ranked by cosine similarity (or L2 distance)
4. Apply any additional filters (tag, status, collection)

**Output**: Same format as `codemark list`, with an additional `similarity` score column when `--verbose` is set

### FR-10.3: Reindex Command

**Input**: Optional `--force` flag

**Behavior**:
1. Find all bookmarks without embeddings
2. Generate embeddings in batches
3. Report progress and any failures

**Example**:
```bash
codemark reindex              # Embed bookmarks missing vectors
codemark reindex --force      # Regenerate all embeddings
```

### FR-10.4: Model Configuration

**Configuration file** (`.codemark/config.toml`):
```toml
[semantic]
enabled = true
model = "local"  # "local" or "openai"
local_model = "all-MiniLM-L6-v2"  # or path to custom model
openai_model = "text-embedding-3-small"
openai_api_key = ""  # from env var OPENAI_API_KEY
batch_size = 32
```

### FR-10.5: Fallback Behavior

If semantic search is requested but:
- No embeddings exist: Return helpful message suggesting `codemark reindex`
- Embedding model not found: Return error with installation instructions
- Only some bookmarks have embeddings: Search only those with embeddings

## Implementation Phases

### Phase 2a: Core Semantic Search (Post-Phase 2)

**Deliverables**:
1. `sqlite-vec` integration with `rusqlite`
2. Local embedding model via `candle` or `ort` (ONNX Runtime)
3. Database migration for `bookmark_embeddings` table
4. `--semantic` flag on `search` command
5. `reindex` command
6. Basic model configuration

**Acceptance Criteria**:
- `codemark search "login flow" --semantic` returns relevant bookmarks
- `codemark reindex` generates embeddings for existing bookmarks
- Semantic search works offline with local model
- Embeddings are auto-generated on bookmark creation

**Estimated Scope**: ~1,000–1,500 lines of Rust

### Phase 2b: Enhanced Semantic Features

**Deliverables**:
1. OpenAI API embedding provider option
2. Hybrid search (semantic + keyword fusion)
3. Similarity score display in output
4. Performance optimizations (batch embedding, caching)
5. Per-collection semantic search

**Acceptance Criteria**:
- `codemark search "..." --semantic --provider openai` works with API key
- Hybrid search combines vector similarity with FTS scores
- Batch reindex of 1K bookmarks completes in < 30 seconds

**Estimated Scope**: ~500–800 lines of Rust

### Phase 2c: Code Content Embeddings (Future)

**Deliverables**:
1. Embed actual code snippets (not just metadata)
2. Configurable inclusion/exclusion of code in embeddings
3. Search for similar code patterns across bookmarks

**Note**: Deferred due to complexity of code normalization and token limits

## Architecture Impact

### New Crate: `src/embeddings/`

```
src/
├── embeddings/
│   ├── mod.rs
│   ├── provider.rs          # EmbeddingProvider trait
│   ├── local.rs             # Local model (candle/ort)
│   ├── openai.rs            # OpenAI API client
│   ├── vec_store.rs         # sqlite-vec operations
│   └── config.rs            # Model configuration
```

### Updated Modules

- `cli/search.rs`: Add `--semantic` flag handling
- `storage/db.rs`: Load `sqlite-vec` extension
- `storage/bookmark_repo.rs`: Add embedding CRUD
- `cli/maintenance.rs`: Add `reindex` command

### Dependencies

| Crate | Purpose |
|-------|---------|
| `sqlite-vec` | Vector search extension (loaded via rusqlite) |
| `candle` or `ort` | Local embedding model inference |
| `async-trait` | Async embedding generation |
| `reqwest` | OpenAI API client (optional feature) |

## Performance Targets

| Operation | Target |
|-----------|--------|
| Single embedding generation (local) | < 50ms |
| Single embedding generation (OpenAI) | < 500ms (network) |
| Semantic search query (1K bookmarks) | < 100ms |
| Reindex 100 bookmarks | < 10 seconds |

## Migration Strategy

1. **Opt-in feature**: Semantic search is disabled by default
2. **Lazy initialization**: Embeddings generated on first `--semantic` use or explicit `reindex`
3. **Backwards compatible**: Databases without `bookmark_embeddings` table continue to work
4. **Graceful degradation**: If embedding model fails, keyword search still works

## Open Questions

1. **Should embeddings include file path and language?**
   - Pro: Helps distinguish "auth" in Swift vs TypeScript
   - Con: May skew semantic similarity
   - Decision: Exclude initially, include if testing shows benefit

2. **Should we support hybrid search (semantic + keyword)?**
   - Pro: Best of both worlds
   - Con: More complex scoring
   - Decision: Implement in Phase 2b

3. **What's the default local model?**
   - `all-MiniLM-L6-v2`: Fast, small, good quality
   - `bge-small-en-v1.5`: Better quality, slightly larger
   - Decision: `all-MiniLM-L6-v2` as default, configurable

## Testing Strategy

### Unit Tests
- Embedding generation produces correct vector dimensions
- Vector similarity calculations are accurate
- Configuration parsing and validation

### Integration Tests
- End-to-end: `reindex` → `search --semantic` returns relevant results
- Fallback: Missing embeddings handled gracefully
- Cross-repo: Semantic search across multiple databases

### Quality Tests
- Human evaluation: Query result relevance for common developer questions
- Regression: Known queries continue to return expected results

## Television Integration

### Challenge

Semantic search has significant latency (~40ms local, ~350ms cloud) compared to fuzzy filtering (<1ms). Television's fuzzy filter works in-memory on a cached list, but semantic search requires re-running the source command for each query.

**Solution**: Use a two-mode workflow with explicit semantic search trigger. No debouncing needed because semantic search only runs on explicit Enter press.

### Integration Approach

**Default mode**: Fuzzy filter over full bookmark list (instant)
**Semantic mode**: Triggered via keybinding, runs on Enter press

The workflow:
1. User opens `tv bookmarks` → sees full list
2. Types normally to fuzzy-filter (instant, in-memory)
3. Presses `Ctrl-/` → enters semantic search prompt
4. Types natural language query → presses Enter
5. Source command re-runs with `--semantic` flag
6. Results replace the main list

### Channel Configuration

```toml
# extras/tv-channel-bookmarks.toml

[metadata]
name = "bookmarks"
description = "Codemark structural bookmarks"
requirements = ["codemark"]

# Default: fuzzy filter over cached list
[source]
command = "codemark list --format line"
display = "{split:\\t:1}  {split:\\t:3}  {split:\\t:4}"
output = "{split:\\t:0}"

[preview]
command = "codemark preview {split:\\t:0}"

[ui.preview_panel]
size = 55

[keybindings]
# Semantic search: opens prompt, runs on Enter (not per-keystroke)
ctrl-/ = "actions:semantic_search"
# Standard actions
ctrl-e = "actions:edit"
ctrl-r = "actions:resolve"
ctrl-d = "actions:details"

# Semantic search action
[actions.semantic_search]
description = "Search bookmarks by meaning (natural language)"
# Television prompts for query, then runs this with {query} replaced
command = "codemark search --semantic '{query}' --format line"
mode = "source"
prompt = "Search by meaning: "
prompt_placeholder = "e.g., 'how are tokens validated?'"

# Standard actions
[actions.edit]
description = "Open in $EDITOR"
command = "codemark resolve {split:\\t:0} --format '{file}:{line}' | xargs $EDITOR"
mode = "fork"

[actions.resolve]
description = "Re-resolve bookmark"
command = "codemark resolve {split:\\t:0}"
mode = "execute"

[actions.details]
description = "Show full details"
command = "codemark show {split:\\t:0}"
mode = "execute"
```

### User Experience

```
┌─────────────────────────────────────────────────────────────┐
│  tv bookmarks                                                │
├─────────────────────────────────────────────────────────────┤
│  a1b2c3d4  src/auth/middleware.swift:42  active  auth,middleware │
│  f5e6d7c8  src/api/router.swift:118      drifted  api,routing   │
│  ...                                                         │
│                                                              │
│  Press Ctrl-/ for semantic search                           │
└─────────────────────────────────────────────────────────────┘

                    (user presses Ctrl-/)

┌─────────────────────────────────────────────────────────────┐
│  Search by meaning: _____________________________            │
│                     e.g., 'how are tokens validated?'        │
└─────────────────────────────────────────────────────────────┘

                    (user types "login", presses Enter)

┌─────────────────────────────────────────────────────────────┐
│  a1b2c3d4  src/auth/middleware.swift:42  active  auth,middleware │
│  b2d3e4f5  src/user/login.swift:15        active  login,form    │
│  c3e4f5g6  src/auth/token_validator.swift:8  active  jwt,token    │
│                                                              │
│  (semantic results - ranked by meaning, not keywords)       │
└─────────────────────────────────────────────────────────────┘
```

### fzf Integration (Similar Pattern)

```bash
# Fuzzy filter by default
codemark list --format line | fzf --preview 'codemark preview {1}'

# Semantic search: use fzf's prompt as query input
fzf --prompt="Search by meaning> " --print-query \
  | head -1 \
  | xargs -I{} codemark search --semantic "{}" --format line \
  | fzf --preview 'codemark preview {1}'
```

Or using fzf's `execute` action:
```bash
codemark list --format line | fzf \
  --preview 'codemark preview {1}' \
  --bind 'ctrl-/:execute-silent(echo {} > /tmp/codemark_semantic_query)+reload(codemark search --semantic "$(cat /tmp/codemark_semantic_query)" --format line)'
```

## References

- [`sqlite-vec` documentation](https://github.com/asg017/sqlite-vec)
- [Sentence Transformers documentation](https://www.sbert.net/)
- [Candle: Rust ML framework](https://github.com/huggingface/candle)
- [ONNX Runtime Rust bindings](https://github.com/microsoft/onnxruntime-rust)
