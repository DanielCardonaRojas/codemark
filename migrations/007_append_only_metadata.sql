-- Migration 007: Append-only metadata model
-- This creates separate tables for annotations and tags, enabling
-- accumulation of context across multiple AI sessions without duplication.
--
-- NOTE: The data migration from old schema (notes, context, tags columns)
-- is handled in Rust code (db.rs::migrate_to_v7) to gracefully handle
-- both fresh installs and upgrades from schema 6.

-- Create bookmark_annotations table: one row per "interaction" with a bookmark
CREATE TABLE IF NOT EXISTS bookmark_annotations (
    id          TEXT PRIMARY KEY,
    bookmark_id TEXT NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
    added_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    added_by    TEXT,
    notes       TEXT,
    context     TEXT,
    source      TEXT
);

CREATE INDEX IF NOT EXISTS idx_annotations_bookmark ON bookmark_annotations(bookmark_id);
CREATE INDEX IF NOT EXISTS idx_annotations_added ON bookmark_annotations(added_at);

-- Create bookmark_tags table: many-to-many relationship
CREATE TABLE IF NOT EXISTS bookmark_tags (
    bookmark_id TEXT NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
    tag         TEXT NOT NULL,
    added_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    added_by    TEXT,
    PRIMARY KEY (bookmark_id, tag)
);

CREATE INDEX IF NOT EXISTS idx_tags_bookmark ON bookmark_tags(bookmark_id);
CREATE INDEX IF NOT EXISTS idx_tags_tag ON bookmark_tags(tag);

-- Recreate the bookmarks table without the metadata columns
-- and add the UNIQUE constraint on (file_path, query)

-- Create a new bookmarks table without the metadata columns
CREATE TABLE IF NOT EXISTS bookmarks_new (
    id                TEXT PRIMARY KEY,
    query             TEXT NOT NULL,
    language          TEXT NOT NULL,
    file_path         TEXT NOT NULL,
    content_hash      TEXT,
    commit_hash       TEXT,
    status            TEXT NOT NULL DEFAULT 'active',
    resolution_method TEXT,
    last_resolved_at  TEXT,
    stale_since       TEXT,
    created_at        TEXT NOT NULL,
    created_by        TEXT,
    UNIQUE(file_path, query)
);

CREATE INDEX IF NOT EXISTS idx_bookmarks_new_status ON bookmarks_new(status);
CREATE INDEX IF NOT EXISTS idx_bookmarks_new_file ON bookmarks_new(file_path);
CREATE INDEX IF NOT EXISTS idx_bookmarks_new_language ON bookmarks_new(language);

-- Copy data from old table to new table (only columns that always exist)
INSERT INTO bookmarks_new (id, query, language, file_path, content_hash, commit_hash,
                           status, resolution_method, last_resolved_at, stale_since,
                           created_at, created_by)
    SELECT id, query, language, file_path, content_hash, commit_hash,
           status, resolution_method, last_resolved_at, stale_since,
           created_at, created_by
    FROM bookmarks;

-- Drop old table and rename new one
DROP TABLE bookmarks;
ALTER TABLE bookmarks_new RENAME TO bookmarks;

-- Recreate indexes on the renamed table
CREATE INDEX IF NOT EXISTS idx_bookmarks_status ON bookmarks(status);
CREATE INDEX IF NOT EXISTS idx_bookmarks_file ON bookmarks(file_path);
CREATE INDEX IF NOT EXISTS idx_bookmarks_language ON bookmarks(language);

-- Update FTS5 triggers to work with the new schema
-- Drop old triggers
DROP TRIGGER IF EXISTS bookmarks_ai;
DROP TRIGGER IF EXISTS bookmarks_ad;
DROP TRIGGER IF EXISTS bookmarks_au;

-- Drop old FTS table and recreate with annotations
DROP TABLE IF EXISTS bookmarks_fts;

-- Recreate FTS with aggregated annotation content
CREATE VIRTUAL TABLE IF NOT EXISTS bookmarks_fts USING fts5(
    notes,
    context,
    file_path,
    tags,
    content='bookmarks',
    content_rowid='rowid'
);

-- Populate FTS from existing bookmarks (empty now since we moved metadata)
INSERT INTO bookmarks_fts(rowid, notes, context, file_path, tags)
    SELECT rowid, '', '', COALESCE(file_path, ''), ''
    FROM bookmarks;

-- Keep FTS in sync: insert (empty strings for notes/context/tags)
CREATE TRIGGER IF NOT EXISTS bookmarks_ai AFTER INSERT ON bookmarks BEGIN
    INSERT INTO bookmarks_fts(rowid, notes, context, file_path, tags)
        VALUES (new.rowid, '', '', COALESCE(new.file_path, ''), '');
END;

-- Keep FTS in sync: delete
CREATE TRIGGER IF NOT EXISTS bookmarks_ad AFTER DELETE ON bookmarks BEGIN
    INSERT INTO bookmarks_fts(bookmarks_fts, rowid, notes, context, file_path, tags)
        VALUES ('delete', old.rowid, '', '', COALESCE(old.file_path, ''), '');
END;

-- Keep FTS in sync: update (only file_path now)
CREATE TRIGGER IF NOT EXISTS bookmarks_au AFTER UPDATE OF file_path ON bookmarks BEGIN
    INSERT INTO bookmarks_fts(bookmarks_fts, rowid, notes, context, file_path, tags)
        VALUES ('delete', old.rowid, '', '', COALESCE(old.file_path, ''), '');
    INSERT INTO bookmarks_fts(rowid, notes, context, file_path, tags)
        VALUES (new.rowid, '', '', COALESCE(new.file_path, ''), '');
END;
