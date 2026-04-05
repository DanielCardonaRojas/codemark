-- Migration 002: Add FTS5 full-text search index over notes and context.
-- content= links to the bookmarks table so FTS stays in sync via triggers.

CREATE VIRTUAL TABLE IF NOT EXISTS bookmarks_fts USING fts5(
    notes,
    context,
    content='bookmarks',
    content_rowid='rowid'
);

-- Populate FTS from existing bookmarks
INSERT INTO bookmarks_fts(rowid, notes, context)
    SELECT rowid, COALESCE(notes, ''), COALESCE(context, '')
    FROM bookmarks;

-- Keep FTS in sync: insert
CREATE TRIGGER IF NOT EXISTS bookmarks_ai AFTER INSERT ON bookmarks BEGIN
    INSERT INTO bookmarks_fts(rowid, notes, context)
        VALUES (new.rowid, COALESCE(new.notes, ''), COALESCE(new.context, ''));
END;

-- Keep FTS in sync: delete
CREATE TRIGGER IF NOT EXISTS bookmarks_ad AFTER DELETE ON bookmarks BEGIN
    INSERT INTO bookmarks_fts(bookmarks_fts, rowid, notes, context)
        VALUES ('delete', old.rowid, COALESCE(old.notes, ''), COALESCE(old.context, ''));
END;

-- Keep FTS in sync: update notes/context
CREATE TRIGGER IF NOT EXISTS bookmarks_au AFTER UPDATE OF notes, context ON bookmarks BEGIN
    INSERT INTO bookmarks_fts(bookmarks_fts, rowid, notes, context)
        VALUES ('delete', old.rowid, COALESCE(old.notes, ''), COALESCE(old.context, ''));
    INSERT INTO bookmarks_fts(rowid, notes, context)
        VALUES (new.rowid, COALESCE(new.notes, ''), COALESCE(new.context, ''));
END;
