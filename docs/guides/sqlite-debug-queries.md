# SQLite Debug Queries

Practical queries for inspecting and debugging the Codemark database during development. Run these with the `sqlite3` CLI:

```bash
sqlite3 .codemark/codemark.db
```

Useful sqlite3 settings for readable output:

```sql
.mode column
.headers on
.width 8 40 10 8 20 40
```

---

## Schema Inspection

```sql
-- Check schema version
SELECT * FROM schema_meta;

-- List all tables
.tables

-- Show table schemas
.schema bookmarks
.schema resolutions
.schema collections
.schema collection_bookmarks

-- List all indexes
SELECT name, tbl_name FROM sqlite_master WHERE type = 'index';

-- Check WAL mode is enabled
PRAGMA journal_mode;

-- Database size and page stats
PRAGMA page_count;
PRAGMA page_size;
```

---

## Bookmarks

### Overview

```sql
-- Total bookmark count by status
SELECT status, COUNT(*) AS count
FROM bookmarks
GROUP BY status
ORDER BY CASE status
    WHEN 'active' THEN 1
    WHEN 'drifted' THEN 2
    WHEN 'stale' THEN 3
    WHEN 'archived' THEN 4
END;

-- Total bookmarks per language
SELECT language, COUNT(*) AS count
FROM bookmarks
GROUP BY language
ORDER BY count DESC;

-- Total bookmarks per file (top 20)
SELECT file_path, COUNT(*) AS count
FROM bookmarks
GROUP BY file_path
ORDER BY count DESC
LIMIT 20;
```

### Browse Bookmarks

```sql
-- All non-archived bookmarks, most recent first
SELECT substr(id, 1, 8) AS short_id, file_path, status, tags, substr(notes, 1, 50) AS note
FROM bookmarks
WHERE status != 'archived'
ORDER BY created_at DESC;

-- Bookmarks for a specific file
SELECT substr(id, 1, 8) AS short_id, status, resolution_method, tags, substr(notes, 1, 50) AS note
FROM bookmarks
WHERE file_path = 'src/auth/middleware.swift'
ORDER BY created_at DESC;

-- Find bookmark by ID prefix
SELECT *
FROM bookmarks
WHERE id LIKE 'a1b2c3d4%';

-- Recently created bookmarks (last 24h)
SELECT substr(id, 1, 8) AS short_id, file_path, tags, substr(notes, 1, 50) AS note
FROM bookmarks
WHERE created_at > datetime('now', '-1 day')
ORDER BY created_at DESC;

-- Bookmarks that have never been resolved
SELECT substr(id, 1, 8) AS short_id, file_path, status, created_at
FROM bookmarks
WHERE last_resolved_at IS NULL;
```

### Tags

```sql
-- All distinct tags in use (unnested from JSON arrays)
SELECT DISTINCT j.value AS tag, COUNT(*) AS count
FROM bookmarks b, json_each(b.tags) j
GROUP BY j.value
ORDER BY count DESC;

-- Find bookmarks with a specific tag
SELECT substr(b.id, 1, 8) AS short_id, b.file_path, b.status, b.notes
FROM bookmarks b
WHERE EXISTS (
    SELECT 1 FROM json_each(b.tags) WHERE value = 'auth'
);

-- Bookmarks with multiple tags (show tag count)
SELECT substr(id, 1, 8) AS short_id, file_path,
       json_array_length(tags) AS tag_count, tags
FROM bookmarks
WHERE json_array_length(tags) > 1
ORDER BY tag_count DESC;

-- Bookmarks with no tags
SELECT substr(id, 1, 8) AS short_id, file_path, notes
FROM bookmarks
WHERE tags IS NULL OR tags = '[]' OR tags = 'null';
```

### Health & Drift

```sql
-- Drifted bookmarks (resolved but not via exact match)
SELECT substr(id, 1, 8) AS short_id, file_path, resolution_method, last_resolved_at
FROM bookmarks
WHERE status = 'drifted'
ORDER BY last_resolved_at DESC;

-- Stale bookmarks with how long they've been stale
SELECT substr(id, 1, 8) AS short_id, file_path,
       stale_since,
       CAST((julianday('now') - julianday(stale_since)) AS INTEGER) AS days_stale
FROM bookmarks
WHERE status = 'stale'
ORDER BY stale_since ASC;

-- Bookmarks approaching archive threshold (stale > 5 days)
SELECT substr(id, 1, 8) AS short_id, file_path, stale_since,
       CAST((julianday('now') - julianday(stale_since)) AS INTEGER) AS days_stale
FROM bookmarks
WHERE status = 'stale'
  AND stale_since < datetime('now', '-5 days');

-- Resolution method distribution
SELECT resolution_method, COUNT(*) AS count
FROM bookmarks
WHERE resolution_method IS NOT NULL
GROUP BY resolution_method
ORDER BY count DESC;
```

### Queries (tree-sitter)

```sql
-- Show stored queries (truncated for readability)
SELECT substr(id, 1, 8) AS short_id, language, substr(query, 1, 80) AS query_preview
FROM bookmarks
ORDER BY created_at DESC
LIMIT 20;

-- Full query for a specific bookmark
SELECT query
FROM bookmarks
WHERE id LIKE 'a1b2c3d4%';

-- Bookmarks with identical queries (possible duplicates)
SELECT query, COUNT(*) AS count, GROUP_CONCAT(substr(id, 1, 8), ', ') AS bookmark_ids
FROM bookmarks
GROUP BY query
HAVING count > 1;

-- Bookmarks with identical content hashes (same code, different queries)
SELECT content_hash, COUNT(*) AS count,
       GROUP_CONCAT(substr(id, 1, 8), ', ') AS bookmark_ids,
       GROUP_CONCAT(file_path, ', ') AS files
FROM bookmarks
WHERE content_hash IS NOT NULL
GROUP BY content_hash
HAVING count > 1;
```

---

## Resolutions

```sql
-- Recent resolutions (last 20)
SELECT substr(r.bookmark_id, 1, 8) AS bookmark, r.method, r.match_count,
       r.file_path, r.byte_range, r.resolved_at
FROM resolutions r
ORDER BY r.resolved_at DESC
LIMIT 20;

-- Resolution history for a specific bookmark
SELECT method, match_count, file_path, byte_range, content_hash, resolved_at
FROM resolutions
WHERE bookmark_id LIKE 'a1b2c3d4%'
ORDER BY resolved_at DESC;

-- Failed resolutions (bookmark couldn't be found)
SELECT substr(r.bookmark_id, 1, 8) AS bookmark, r.resolved_at,
       b.file_path AS original_file, b.notes
FROM resolutions r
JOIN bookmarks b ON r.bookmark_id = b.id
WHERE r.method = 'failed'
ORDER BY r.resolved_at DESC;

-- Bookmarks that have moved files (resolved in a different file than stored)
SELECT substr(r.bookmark_id, 1, 8) AS bookmark,
       b.file_path AS original, r.file_path AS resolved_to,
       r.method, r.resolved_at
FROM resolutions r
JOIN bookmarks b ON r.bookmark_id = b.id
WHERE r.file_path IS NOT NULL AND r.file_path != b.file_path
ORDER BY r.resolved_at DESC;

-- Resolution success rate
SELECT
    method,
    COUNT(*) AS count,
    ROUND(100.0 * COUNT(*) / (SELECT COUNT(*) FROM resolutions), 1) AS pct
FROM resolutions
GROUP BY method
ORDER BY count DESC;

-- Average match count per method (higher = less precise query)
SELECT method, ROUND(AVG(match_count), 1) AS avg_matches, COUNT(*) AS count
FROM resolutions
WHERE match_count IS NOT NULL
GROUP BY method;

-- Content hash changes over time for a bookmark (detect drift)
SELECT content_hash, method, resolved_at
FROM resolutions
WHERE bookmark_id LIKE 'a1b2c3d4%'
ORDER BY resolved_at ASC;
```

---

## Collections

```sql
-- All collections with bookmark counts
SELECT c.name, c.description,
       COUNT(cb.bookmark_id) AS bookmark_count,
       c.created_at
FROM collections c
LEFT JOIN collection_bookmarks cb ON c.id = cb.collection_id
GROUP BY c.id
ORDER BY c.name;

-- Bookmarks in a specific collection
SELECT substr(b.id, 1, 8) AS short_id, b.file_path, b.status, b.tags,
       substr(b.notes, 1, 50) AS note
FROM bookmarks b
JOIN collection_bookmarks cb ON b.id = cb.bookmark_id
JOIN collections c ON cb.collection_id = c.id
WHERE c.name = 'bugfix-auth'
ORDER BY b.file_path;

-- Collections a specific bookmark belongs to
SELECT c.name, c.description, cb.added_at
FROM collections c
JOIN collection_bookmarks cb ON c.id = cb.collection_id
WHERE cb.bookmark_id LIKE 'a1b2c3d4%';

-- Empty collections (no bookmarks)
SELECT c.name, c.description, c.created_at
FROM collections c
LEFT JOIN collection_bookmarks cb ON c.id = cb.collection_id
WHERE cb.collection_id IS NULL;

-- Orphaned bookmarks (not in any collection)
SELECT substr(b.id, 1, 8) AS short_id, b.file_path, b.tags, b.notes
FROM bookmarks b
LEFT JOIN collection_bookmarks cb ON b.id = cb.bookmark_id
WHERE cb.bookmark_id IS NULL
  AND b.status != 'archived';

-- Collection health summary (status breakdown per collection)
SELECT c.name,
       SUM(CASE WHEN b.status = 'active' THEN 1 ELSE 0 END) AS active,
       SUM(CASE WHEN b.status = 'drifted' THEN 1 ELSE 0 END) AS drifted,
       SUM(CASE WHEN b.status = 'stale' THEN 1 ELSE 0 END) AS stale
FROM collections c
JOIN collection_bookmarks cb ON c.id = cb.collection_id
JOIN bookmarks b ON cb.bookmark_id = b.id
GROUP BY c.name
ORDER BY c.name;
```

---

## Data Integrity Checks

```sql
-- Bookmarks with invalid status values
SELECT substr(id, 1, 8) AS short_id, status
FROM bookmarks
WHERE status NOT IN ('active', 'drifted', 'stale', 'archived');

-- Bookmarks with invalid resolution methods
SELECT substr(id, 1, 8) AS short_id, resolution_method
FROM bookmarks
WHERE resolution_method IS NOT NULL
  AND resolution_method NOT IN ('exact', 'relaxed', 'hash_fallback');

-- Resolutions with invalid methods
SELECT substr(id, 1, 8) AS short_id, method
FROM resolutions
WHERE method NOT IN ('exact', 'relaxed', 'hash_fallback', 'failed');

-- Bookmarks with malformed tags (not valid JSON arrays)
SELECT substr(id, 1, 8) AS short_id, tags
FROM bookmarks
WHERE tags IS NOT NULL
  AND json_valid(tags) = 0;

-- Orphaned resolutions (bookmark was deleted but resolution remains — shouldn't happen with CASCADE)
SELECT substr(r.id, 1, 8) AS resolution_id, substr(r.bookmark_id, 1, 8) AS bookmark_id
FROM resolutions r
LEFT JOIN bookmarks b ON r.bookmark_id = b.id
WHERE b.id IS NULL;

-- Orphaned collection_bookmarks (shouldn't happen with CASCADE)
SELECT substr(cb.collection_id, 1, 8) AS collection_id, substr(cb.bookmark_id, 1, 8) AS bookmark_id
FROM collection_bookmarks cb
LEFT JOIN collections c ON cb.collection_id = c.id
LEFT JOIN bookmarks b ON cb.bookmark_id = b.id
WHERE c.id IS NULL OR b.id IS NULL;

-- Duplicate bookmark IDs (should never happen — PK constraint)
SELECT id, COUNT(*) AS count
FROM bookmarks
GROUP BY id
HAVING count > 1;

-- Bookmarks with stale_since set but status is not stale
SELECT substr(id, 1, 8) AS short_id, status, stale_since
FROM bookmarks
WHERE stale_since IS NOT NULL AND status NOT IN ('stale', 'archived');

-- Stale bookmarks without stale_since timestamp
SELECT substr(id, 1, 8) AS short_id, status, stale_since
FROM bookmarks
WHERE status = 'stale' AND stale_since IS NULL;
```

---

## Test Data Seeding

Use these to populate the database for manual testing:

```sql
-- Insert a test bookmark
INSERT INTO bookmarks (id, query, language, file_path, content_hash, commit_hash,
                       status, created_at, tags, notes)
VALUES (
    'a1b2c3d4-e5f6-7890-abcd-ef1234567890',
    '(function_declaration name: (simple_identifier) @name (#eq? @name "validateToken")) @target',
    'swift',
    'src/auth/middleware.swift',
    'sha256:abcdef1234567890',
    'abc123def456',
    'active',
    datetime('now'),
    '["auth", "middleware"]',
    'JWT validation entry point'
);

-- Insert a drifted bookmark
INSERT INTO bookmarks (id, query, language, file_path, content_hash, commit_hash,
                       status, resolution_method, last_resolved_at, created_at, tags, notes)
VALUES (
    'b2c3d4e5-f6a7-8901-bcde-f12345678901',
    '(function_declaration name: (simple_identifier) @name (#eq? @name "refreshToken")) @target',
    'swift',
    'src/auth/token_store.swift',
    'sha256:bcdef12345678901',
    'abc123def456',
    'drifted',
    'relaxed',
    datetime('now'),
    datetime('now', '-2 days'),
    '["auth", "tokens"]',
    'Token refresh logic'
);

-- Insert a stale bookmark
INSERT INTO bookmarks (id, query, language, file_path, content_hash, commit_hash,
                       status, stale_since, created_at, tags, notes)
VALUES (
    'c3d4e5f6-a7b8-9012-cdef-123456789012',
    '(class_declaration name: (type_identifier) @name (#eq? @name "OldAuthProvider")) @target',
    'swift',
    'src/auth/old_provider.swift',
    'sha256:cdef123456789012',
    'abc123def456',
    'stale',
    datetime('now', '-3 days'),
    datetime('now', '-10 days'),
    '["auth", "deprecated"]',
    'Old auth provider — likely deleted'
);

-- Insert a test collection and add bookmarks
INSERT INTO collections (id, name, description, created_at)
VALUES (
    'd4e5f6a7-b8c9-0123-defa-234567890123',
    'bugfix-auth',
    'Token validation regression fix',
    datetime('now')
);

INSERT INTO collection_bookmarks (collection_id, bookmark_id, added_at)
VALUES
    ('d4e5f6a7-b8c9-0123-defa-234567890123', 'a1b2c3d4-e5f6-7890-abcd-ef1234567890', datetime('now')),
    ('d4e5f6a7-b8c9-0123-defa-234567890123', 'b2c3d4e5-f6a7-8901-bcde-f12345678901', datetime('now'));

-- Insert test resolution history
INSERT INTO resolutions (id, bookmark_id, resolved_at, commit_hash, method, match_count, file_path, byte_range, content_hash)
VALUES
    ('e5f6a7b8-c9d0-1234-efab-345678901234', 'a1b2c3d4-e5f6-7890-abcd-ef1234567890',
     datetime('now', '-1 day'), 'abc123def456', 'exact', 1,
     'src/auth/middleware.swift', '1024:1280', 'sha256:abcdef1234567890'),
    ('f6a7b8c9-d0e1-2345-fabc-456789012345', 'a1b2c3d4-e5f6-7890-abcd-ef1234567890',
     datetime('now'), 'def456abc789', 'exact', 1,
     'src/auth/middleware.swift', '1030:1290', 'sha256:abcdef1234567890'),
    ('a7b8c9d0-e1f2-3456-abcd-567890123456', 'b2c3d4e5-f6a7-8901-bcde-f12345678901',
     datetime('now'), 'def456abc789', 'relaxed', 2,
     'src/auth/token_store.swift', '2048:2300', 'sha256:bcdef12345678901');
```

### Quick cleanup

```sql
-- Delete all test data
DELETE FROM collection_bookmarks;
DELETE FROM collections;
DELETE FROM resolutions;
DELETE FROM bookmarks;

-- Reset to empty state (keeps schema)
DELETE FROM schema_meta WHERE key != 'schema_version';
```

---

## Performance Diagnostics

```sql
-- Query planner analysis for common queries
EXPLAIN QUERY PLAN
SELECT * FROM bookmarks WHERE file_path = 'src/auth/middleware.swift' AND status = 'active';

EXPLAIN QUERY PLAN
SELECT * FROM bookmarks b WHERE EXISTS (SELECT 1 FROM json_each(b.tags) WHERE value = 'auth');

EXPLAIN QUERY PLAN
SELECT * FROM resolutions WHERE bookmark_id = 'a1b2c3d4-e5f6-7890-abcd-ef1234567890';

-- Check index usage stats (if available)
PRAGMA index_list(bookmarks);
PRAGMA index_info(idx_bookmarks_status);
PRAGMA index_info(idx_bookmarks_file);

-- Database file size
SELECT
    page_count * page_size AS total_bytes,
    ROUND(page_count * page_size / 1024.0, 1) AS total_kb
FROM pragma_page_count(), pragma_page_size();

-- Table row counts
SELECT 'bookmarks' AS tbl, COUNT(*) AS rows FROM bookmarks
UNION ALL SELECT 'resolutions', COUNT(*) FROM resolutions
UNION ALL SELECT 'collections', COUNT(*) FROM collections
UNION ALL SELECT 'collection_bookmarks', COUNT(*) FROM collection_bookmarks;
```
