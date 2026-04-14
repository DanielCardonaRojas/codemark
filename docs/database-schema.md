# Database Schema

Codemark uses SQLite as its storage backend. The database is located at `.codemark/codemark.db` relative to the git repository root.

## Entity Relationship Diagram

```mermaid
erDiagram
    BOOKMARKS ||--o{ RESOLUTIONS : "has"
    BOOKMARKS ||--o{ BOOKMARK_ANNOTATIONS : "annotated with"
    BOOKMARKS ||--o{ BOOKMARK_TAGS : "tagged with"
    COLLECTIONS ||--o{ COLLECTION_BOOKMARKS : "contains"
    BOOKMARKS ||--o{ COLLECTION_BOOKMARKS : "member of"

    BOOKMARKS {
        TEXT id PK "UUIDv4"
        TEXT query FK "tree-sitter S-expression"
        TEXT language FK "swift|rust|typescript|python|go|java|c_sharp|dart"
        TEXT file_path FK "relative to repo root"
        TEXT content_hash FK "sha256 of normalized content"
        TEXT commit_hash FK "git HEAD at creation"
        TEXT status FK "active|drifted|stale|archived"
        TEXT resolution_method FK "exact|relaxed|hash_fallback"
        TEXT last_resolved_at FK "ISO 8601 timestamp"
        TEXT stale_since FK "ISO 8601 timestamp"
        TEXT created_at FK "ISO 8601 timestamp"
        TEXT created_by FK "agent session identifier"
    }

    BOOKMARK_ANNOTATIONS {
        TEXT id PK "UUIDv4"
        TEXT bookmark_id FK "references BOOKMARKS(id)"
        TEXT added_at FK "ISO 8601 timestamp"
        TEXT added_by FK "agent or user identifier"
        TEXT notes FK "semantic annotation"
        TEXT context FK "what the agent was doing"
        TEXT source FK "where annotation came from"
    }

    BOOKMARK_TAGS {
        TEXT bookmark_id FK "references BOOKMARKS(id)"
        TEXT tag FK "tag label"
        TEXT added_at FK "ISO 8601 timestamp"
        TEXT added_by FK "agent or user identifier"
    }

    RESOLUTIONS {
        TEXT id PK "UUIDv4"
        TEXT bookmark_id FK "references BOOKMARKS(id)"
        TEXT resolved_at FK "ISO 8601 timestamp"
        TEXT commit_hash FK "git HEAD at resolution time"
        TEXT method FK "exact|relaxed|hash_fallback|failed"
        INTEGER match_count FK "number of query matches"
        TEXT file_path FK "where code was found (may differ from bookmark)"
        TEXT byte_range FK "start:end bytes"
        TEXT line_range FK "start:end lines"
        TEXT content_hash FK "hash of matched content"
    }

    COLLECTIONS {
        TEXT id PK "UUIDv4"
        TEXT name FK "unique collection name"
        TEXT description FK "human-readable description"
        TEXT created_at FK "ISO 8601 timestamp"
        TEXT created_by FK "agent session identifier"
    }

    COLLECTION_BOOKMARKS {
        TEXT collection_id FK "references COLLECTIONS(id)"
        TEXT bookmark_id FK "references BOOKMARKS(id)"
        TEXT added_at FK "ISO 8601 timestamp"
    }

    BOOKMARKS }o--|| RESOLUTIONS : "cascade delete"
    BOOKMARKS }o--|| BOOKMARK_ANNOTATIONS : "cascade delete"
    BOOKMARKS }o--|| BOOKMARK_TAGS : "cascade delete"
    COLLECTIONS }o--|| COLLECTION_BOOKMARKS : "cascade delete"
    BOOKMARKS }o--|| COLLECTION_BOOKMARKS : "cascade delete"
```

## Table Details

### bookmarks

The core table storing bookmark metadata and the tree-sitter query used to re-find the code.

| Column | Type | Description |
|--------|------|-------------|
| `id` | TEXT PRIMARY KEY | UUIDv4 identifier |
| `query` | TEXT NOT NULL | Tree-sitter S-expression query |
| `language` | TEXT NOT NULL | Programming language |
| `file_path` | TEXT NOT NULL | Relative path from repo root |
| `content_hash` | TEXT | SHA-256 of normalized content (64-bit) |
| `commit_hash` | TEXT | Git HEAD at creation time |
| `status` | TEXT NOT NULL | `active`, `drifted`, `stale`, or `archived` |
| `resolution_method` | TEXT | Last resolution method: `exact`, `relaxed`, `hash_fallback` |
| `last_resolved_at` | TEXT | ISO 8601 timestamp of last resolution |
| `stale_since` | TEXT | ISO 8601 timestamp when first marked stale |
| `created_at` | TEXT NOT NULL | ISO 8601 creation timestamp |
| `created_by` | TEXT | Agent or user identifier |

**Constraints:**
- `UNIQUE(file_path, query)` - Prevents duplicate bookmarks for the same code location

**Indexes:**
- `idx_bookmarks_status` on `status`
- `idx_bookmarks_file` on `file_path`
- `idx_bookmarks_language` on `language`

### bookmark_annotations

Append-only metadata for each bookmark. Every time an AI agent or user adds context to a bookmark, a new annotation row is created.

| Column | Type | Description |
|--------|------|-------------|
| `id` | TEXT PRIMARY KEY | UUIDv4 identifier |
| `bookmark_id` | TEXT NOT NULL | Foreign key to bookmarks |
| `added_at` | TEXT NOT NULL | ISO 8601 timestamp |
| `added_by` | TEXT | Agent or user identifier |
| `notes` | TEXT | Semantic annotation |
| `context` | TEXT | What the agent was doing when bookmarking |
| `source` | TEXT | Where annotation came from (e.g., "cli", "claude-code") |

**Indexes:**
- `idx_annotations_bookmark` on `bookmark_id`
- `idx_annotations_added` on `added_at`

### bookmark_tags

Many-to-many relationship between bookmarks and tags. Tags are append-only - adding the same tag twice is ignored.

| Column | Type | Description |
|--------|------|-------------|
| `bookmark_id` | TEXT NOT NULL | Foreign key to bookmarks |
| `tag` | TEXT NOT NULL | Tag label |
| `added_at` | TEXT NOT NULL | ISO 8601 timestamp |
| `added_by` | TEXT | Agent or user identifier |

**Primary Key:** `(bookmark_id, tag)`

**Indexes:**
- `idx_tags_bookmark` on `bookmark_id`
- `idx_tags_tag` on `tag`

### resolutions

Audit trail of where bookmarks were found over time. Records are pruned based on `max_resolutions_per_bookmark` config.

| Column | Type | Description |
|--------|------|-------------|
| `id` | TEXT PRIMARY KEY | UUIDv4 identifier |
| `bookmark_id` | TEXT NOT NULL | Foreign key to bookmarks |
| `resolved_at` | TEXT NOT NULL | ISO 8601 timestamp |
| `commit_hash` | TEXT | Git HEAD at resolution time |
| `method` | TEXT NOT NULL | `exact`, `relaxed`, `hash_fallback`, or `failed` |
| `match_count` | INTEGER | Number of query matches (for debugging) |
| `file_path` | TEXT | Where code was found (may differ from bookmark) |
| `byte_range` | TEXT | Byte range as `start:end` |
| `line_range` | TEXT | Line range as `start:end` |
| `content_hash` | TEXT | Hash of matched content |

**Deduplication:** A new resolution is considered a duplicate if an existing resolution has the same `byte_range`, `line_range`, and `method`. When a duplicate is detected, the existing resolution's `commit_hash` and `resolved_at` are updated instead of creating a new entry.

**Indexes:**
- `idx_resolutions_bookmark` on `bookmark_id`
- `idx_resolutions_resolved` on `resolved_at`

### collections

Named groups of bookmarks for organizing related code.

| Column | Type | Description |
|--------|------|-------------|
| `id` | TEXT PRIMARY KEY | UUIDv4 identifier |
| `name` | TEXT NOT NULL UNIQUE | Collection name |
| `description` | TEXT | Human-readable description |
| `created_at` | TEXT NOT NULL | ISO 8601 creation timestamp |
| `created_by` | TEXT | Agent or user identifier |

**Indexes:**
- `idx_collections_name` on `name`

### collection_bookmarks

Many-to-many relationship between collections and bookmarks. Maintains insertion order via `added_at`.

| Column | Type | Description |
|--------|------|-------------|
| `collection_id` | TEXT NOT NULL | Foreign key to collections |
| `bookmark_id` | TEXT NOT NULL | Foreign key to bookmarks |
| `added_at` | TEXT NOT NULL | ISO 8601 timestamp |

**Primary Key:** `(collection_id, bookmark_id)`

**Indexes:**
- `idx_cb_bookmark` on `bookmark_id`

## Cascade Deletion

- When a bookmark is deleted, all its resolutions, annotations, and tags are automatically deleted
- When a collection is deleted, all `collection_bookmarks` entries are deleted
- When a bookmark is deleted, all `collection_bookmarks` references are deleted

## Append-Only Metadata Design

The schema uses an append-only pattern for metadata (notes, context, tags) to support multi-agent workflows:

1. **Bookmarks are unique**: The `UNIQUE(file_path, query)` constraint prevents duplicate bookmarks for the same code
2. **Annotations accumulate**: Each time context is added, a new `bookmark_annotations` row is created
3. **Tags are append-only**: Adding the same tag twice is ignored (PRIMARY KEY constraint)
4. **Full history preserved**: Every annotation shows who added it and when

This design enables AI agents to collaboratively build context around code without creating sparse, duplicate bookmarks.

## Deduplication Strategy

Resolutions are deduplicated based on the code location, not the commit:

```sql
-- A duplicate is detected when all three match:
byte_range = "806:3337"
line_range = "27:102"
method = "exact"
```

When a duplicate is detected:
- The existing resolution's `commit_hash` and `resolved_at` are **updated**
- No new row is inserted
- This keeps the resolution history clean while maintaining accurate metadata

This means if you heal a bookmark at commit A, then make an unrelated change at commit B and heal again, you'll have one resolution entry (updated with commit B's info) rather than two redundant entries.
