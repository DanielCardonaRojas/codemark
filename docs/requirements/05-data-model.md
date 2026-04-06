# Codemark: Data Model

## Database Location

`.codemark/codemark.db` relative to the git repository root.

The `.codemark/` directory should be added to `.gitignore` — bookmark databases are agent-local, not shared across developers (each agent builds its own contextual memory).

## Schema

### Metadata Table

```sql
CREATE TABLE schema_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Populated on first migration:
-- ('schema_version', '1')
-- ('created_at', '2026-03-23T00:00:00Z')
```

### Bookmarks Table

```sql
CREATE TABLE bookmarks (
    id                TEXT PRIMARY KEY,       -- UUIDv4
    query             TEXT NOT NULL,          -- tree-sitter query string
    language          TEXT NOT NULL,          -- swift, typescript, rust, python
    file_path         TEXT NOT NULL,          -- relative to repo root
    content_hash      TEXT,                   -- sha256 of matched node text

    -- git context (nullable — works without git)
    commit_hash       TEXT,                   -- HEAD at bookmark creation

    -- health tracking
    status            TEXT NOT NULL DEFAULT 'active',    -- active | drifted | stale | archived
    resolution_method TEXT,                              -- exact | relaxed | hash_fallback
    last_resolved_at  TEXT,                              -- ISO 8601
    stale_since       TEXT,                              -- ISO 8601, set when first failing

    -- metadata
    created_at        TEXT NOT NULL,          -- ISO 8601
    created_by        TEXT,                   -- agent session identifier
    tags              TEXT,                   -- JSON array: ["auth", "api-boundary"]
    notes             TEXT,                   -- agent semantic annotation
    context           TEXT                    -- what the agent was doing
);

CREATE INDEX idx_bookmarks_status ON bookmarks(status);
CREATE INDEX idx_bookmarks_file ON bookmarks(file_path);
CREATE INDEX idx_bookmarks_language ON bookmarks(language);
```

**Note on tags**: Stored as a JSON array in a TEXT column. Queried via `json_each()` for filtering. This avoids a join table for what is fundamentally a simple label system with low cardinality. SQLite's JSON support is sufficient.

### Resolutions Table

```sql
CREATE TABLE resolutions (
    id            TEXT PRIMARY KEY,           -- UUIDv4
    bookmark_id   TEXT NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
    resolved_at   TEXT NOT NULL,             -- ISO 8601
    commit_hash   TEXT,                      -- HEAD at resolution time
    method        TEXT NOT NULL,             -- exact | relaxed | hash_fallback | failed
    match_count   INTEGER,                   -- how many nodes matched the query
    file_path     TEXT,                      -- where it resolved (may differ from bookmark)
    byte_range    TEXT,                      -- start:end in the resolved file
    content_hash  TEXT                       -- hash of what matched this time
);

CREATE INDEX idx_resolutions_bookmark ON resolutions(bookmark_id);
CREATE INDEX idx_resolutions_resolved ON resolutions(resolved_at);
```

### Collections Table

```sql
CREATE TABLE collections (
    id          TEXT PRIMARY KEY,       -- UUIDv4
    name        TEXT NOT NULL UNIQUE,   -- slug: "bugfix-auth", "feature-dashboard"
    description TEXT,                   -- human-readable purpose
    created_at  TEXT NOT NULL,          -- ISO 8601
    created_by  TEXT                    -- session identifier
);

CREATE INDEX idx_collections_name ON collections(name);
```

### Collection Bookmarks (Join Table)

```sql
CREATE TABLE collection_bookmarks (
    collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    bookmark_id   TEXT NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
    added_at      TEXT NOT NULL,        -- ISO 8601
    PRIMARY KEY (collection_id, bookmark_id)
);

CREATE INDEX idx_cb_bookmark ON collection_bookmarks(bookmark_id);
```

### Bookmark Embeddings Table (Phase 2a)

```sql
-- Virtual table for vector similarity search using sqlite-vec
CREATE VIRTUAL TABLE bookmark_embeddings USING vec0(
    bookmark_id TEXT PRIMARY KEY,  -- References bookmarks(id)
    embedding FLOAT[384]            -- Size depends on model (all-MiniLM-L6-v2 = 384)
);
```

The `bookmark_embeddings` table stores vector representations of bookmark metadata for semantic search. Embeddings are generated from concatenated tags, notes, and context fields. This table is created in a migration when semantic search is first used.

**Cascade semantics**:
- Deleting a **collection** removes the membership rows in `collection_bookmarks`, but **never** the bookmarks themselves.
- Deleting a **bookmark** removes its membership rows across all collections.

**Collection names**: Lowercase alphanumeric plus hyphens. Must be unique. Examples: `bugfix-auth`, `feature-dashboard`, `sprint-42`.

## Data Types and Constraints

### Bookmark ID
- UUIDv4, stored as lowercase hex with hyphens: `a1b2c3d4-e5f6-7890-abcd-ef1234567890`.
- CLI commands accept unambiguous prefixes (minimum 4 characters).

### Content Hash
- SHA-256 of the matched node's text content (whitespace-normalized).
- Stored as `sha256:<hex>`.
- Whitespace normalization: collapse all runs of whitespace to single spaces, trim leading/trailing. This makes hashes resilient to formatting changes.

### File Paths
- Always stored relative to the repository root.
- Normalized: no leading `./`, forward slashes only.

### Tags
- JSON array of strings: `["auth-flow", "api-boundary", "needs-review"]`.
- Tag names are lowercase, alphanumeric plus hyphens.
- Queried with: `SELECT * FROM bookmarks WHERE EXISTS (SELECT 1 FROM json_each(tags) WHERE value = ?)`.

### Timestamps
- ISO 8601 format with timezone: `2026-03-23T14:30:00Z`.
- Always UTC.

### Status Transitions

```
                  ┌──────────┐
       create ──> │  active   │
                  └──┬───────┘
                     │ resolve succeeds (relaxed/hash)
                  ┌──▼───────┐
                  │  drifted  │ <── resolve succeeds (relaxed/hash)
                  └──┬───────┘
                     │ resolve fails
                  ┌──▼───────┐
                  │  stale    │ <── resolve fails
                  └──┬───────┘
                     │ auto-archive or manual
                  ┌──▼───────┐
                  │ archived  │
                  └──────────┘

  Any state can transition back to `active` if exact resolution succeeds.
  `drifted` can transition back to `active` if exact resolution succeeds.
  `stale` can transition to `active` or `drifted` if resolution succeeds again.
```

## Query Examples

### Find all active bookmarks for a file
```sql
SELECT id, notes, tags, last_resolved_at
FROM bookmarks
WHERE file_path = 'src/auth/middleware.swift'
  AND status = 'active'
ORDER BY last_resolved_at DESC;
```

### Find bookmarks by tag
```sql
SELECT b.id, b.file_path, b.notes, b.status
FROM bookmarks b
WHERE EXISTS (
    SELECT 1 FROM json_each(b.tags) WHERE value = 'auth'
)
AND b.status IN ('active', 'drifted')
ORDER BY b.created_at DESC;
```

### Get resolution history for a bookmark
```sql
SELECT resolved_at, method, match_count, file_path, byte_range
FROM resolutions
WHERE bookmark_id = ?
ORDER BY resolved_at DESC
LIMIT 10;
```

### Health summary
```sql
SELECT status, COUNT(*) as count
FROM bookmarks
GROUP BY status;
```

### List all collections with bookmark counts
```sql
SELECT c.name, c.description, COUNT(cb.bookmark_id) AS bookmark_count
FROM collections c
LEFT JOIN collection_bookmarks cb ON c.id = cb.collection_id
GROUP BY c.id
ORDER BY c.name;
```

### Get bookmarks in a collection
```sql
SELECT b.id, b.file_path, b.notes, b.status, b.tags
FROM bookmarks b
JOIN collection_bookmarks cb ON b.id = cb.bookmark_id
JOIN collections c ON cb.collection_id = c.id
WHERE c.name = ?
AND b.status IN ('active', 'drifted')
ORDER BY b.file_path, b.created_at;
```

### Stale bookmarks older than N days
```sql
SELECT id, file_path, notes, stale_since
FROM bookmarks
WHERE status = 'stale'
  AND stale_since < datetime('now', '-7 days');
```

## Migration Strategy

Migrations are embedded in the binary as SQL strings. On startup:
1. Open (or create) the database.
2. Check `schema_meta.schema_version`.
3. Apply any unapplied migrations in order.
4. Update `schema_version`.

Each migration file is numbered and idempotent:
```
migrations/
├── 001_initial.sql          # Creates bookmarks, resolutions, collections, collection_bookmarks, schema_meta
├── 002_add_fts.sql          # Adds FTS5 virtual table (future)
└── ...
```

## Future Considerations

- **FTS5**: Add a full-text search virtual table over `notes` and `context` for efficient text search at scale.
- **Semantic Search (Phase 2a)**: ✅ Vector embeddings via `sqlite-vec` for natural language queries. See [Semantic Search](./10-semantic-search.md).
- **Collections**: ✅ Promoted to core schema — see `collections` and `collection_bookmarks` tables above.
- **Cross-repo queries**: The `--db` flag accepts multiple paths. Read commands open each database, query them independently, and merge results. Each result is annotated with a `source` label derived from the database path (the parent directory name, e.g., `service-auth`). The schema is unchanged — cross-repo is a query-time concern, not a storage concern. No data is written to secondary databases.
- **Cross-repo bookmark relationships** (Phase 4): "See also" links between bookmarks, including cross-repo references via `source:id` notation (e.g., `service-auth:a1b2c3d4`).
- **Bookmark relationships**: Parent/child or "see also" links between bookmarks.
