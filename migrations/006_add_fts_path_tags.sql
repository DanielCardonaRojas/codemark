-- Migration 006: Add file_path and tags to FTS5 full-text search index.
-- This drops the old FTS table and recreates it with additional columns.

-- Drop old triggers
DROP TRIGGER IF EXISTS bookmarks_ai;
DROP TRIGGER IF EXISTS bookmarks_ad;
DROP TRIGGER IF EXISTS bookmarks_au;

-- Drop old FTS table
DROP TABLE IF EXISTS bookmarks_fts;

-- Recreate FTS with file_path and tags included
CREATE VIRTUAL TABLE IF NOT EXISTS bookmarks_fts USING fts5(
    notes,
    context,
    file_path,
    tags,
    content='bookmarks',
    content_rowid='rowid'
);

-- Populate FTS from existing bookmarks
INSERT INTO bookmarks_fts(rowid, notes, context, file_path, tags)
    SELECT rowid,
           COALESCE(notes, ''),
           COALESCE(context, ''),
           COALESCE(file_path, ''),
           COALESCE(tags, '')
    FROM bookmarks;

-- Keep FTS in sync: insert
CREATE TRIGGER IF NOT EXISTS bookmarks_ai AFTER INSERT ON bookmarks BEGIN
    INSERT INTO bookmarks_fts(rowid, notes, context, file_path, tags)
        VALUES (new.rowid,
                COALESCE(new.notes, ''),
                COALESCE(new.context, ''),
                COALESCE(new.file_path, ''),
                COALESCE(new.tags, ''));
END;

-- Keep FTS in sync: delete
CREATE TRIGGER IF NOT EXISTS bookmarks_ad AFTER DELETE ON bookmarks BEGIN
    INSERT INTO bookmarks_fts(bookmarks_fts, rowid, notes, context, file_path, tags)
        VALUES ('delete',
                old.rowid,
                COALESCE(old.notes, ''),
                COALESCE(old.context, ''),
                COALESCE(old.file_path, ''),
                COALESCE(old.tags, ''));
END;

-- Keep FTS in sync: update notes/context/file_path/tags
CREATE TRIGGER IF NOT EXISTS bookmarks_au AFTER UPDATE OF notes, context, file_path, tags ON bookmarks BEGIN
    INSERT INTO bookmarks_fts(bookmarks_fts, rowid, notes, context, file_path, tags)
        VALUES ('delete',
                old.rowid,
                COALESCE(old.notes, ''),
                COALESCE(old.context, ''),
                COALESCE(old.file_path, ''),
                COALESCE(old.tags, ''));
    INSERT INTO bookmarks_fts(rowid, notes, context, file_path, tags)
        VALUES (new.rowid,
                COALESCE(new.notes, ''),
                COALESCE(new.context, ''),
                COALESCE(new.file_path, ''),
                COALESCE(new.tags, ''));
END;
