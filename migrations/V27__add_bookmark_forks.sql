-- V27__add_bookmark_forks.sql
--
-- Bookmark forks for collection copy/move. See ADR 0001 (GitHub discussion
-- #300) and issue #297.
--
-- Knowledge follows the tour: when a collection is copied (cloned as a
-- template) or moved to another branch, each step gets its *own* bookmark row,
-- so the new tour's notes, comments and resolution history never leak into the
-- original's.
--
-- * `forked_from` records lineage: the bookmark this row was copied from. NULL
--   for ordinary bookmarks. There is deliberately no FOREIGN KEY on it, so
--   deleting the original never cascades into, or is blocked by, its forks.
--   The lineage may then dangle, which is harmless.
-- * `branch` records the branch a fork belongs to. Forks are only re-resolved
--   when HEAD is on that branch.
--
-- The dedup rule ("same repo, path and query is the same bookmark") must no
-- longer apply to forks, because a fork usually keeps its original's path and
-- query. That rule lives in two places:
--   1. V7's inline `UNIQUE(file_path, query)` table constraint, which can only
--      be removed by rebuilding the table;
--   2. V22's partial unique indexes.
-- Both are recreated below as partial indexes with `forked_from IS NULL`.
--
-- The rebuild preserves `rowid` explicitly: `bookmarks_fts` is an
-- external-content FTS5 table keyed on the bookmarks rowid. The runner executes
-- this migration with foreign keys OFF, otherwise `DROP TABLE bookmarks` would
-- cascade-delete resolutions, notes, tags, comments and collection membership.

CREATE TABLE bookmarks_v27 (
    id                    TEXT PRIMARY KEY,
    query                 TEXT NOT NULL,
    language              TEXT NOT NULL,
    file_path             TEXT NOT NULL,
    content_hash          TEXT,
    commit_hash           TEXT,
    created_at            TEXT NOT NULL,
    created_by            TEXT,
    current_resolution_id TEXT REFERENCES resolutions(id),
    repo_id               TEXT REFERENCES repos(id),
    grammar_source        TEXT,
    forked_from           TEXT,
    branch                TEXT
);

INSERT INTO bookmarks_v27 (rowid, id, query, language, file_path, content_hash, commit_hash,
                           created_at, created_by, current_resolution_id, repo_id, grammar_source)
    SELECT rowid, id, query, language, file_path, content_hash, commit_hash,
           created_at, created_by, current_resolution_id, repo_id, grammar_source
    FROM bookmarks;

DROP TABLE bookmarks;
ALTER TABLE bookmarks_v27 RENAME TO bookmarks;

CREATE INDEX IF NOT EXISTS idx_bookmarks_file ON bookmarks(file_path);
CREATE INDEX IF NOT EXISTS idx_bookmarks_language ON bookmarks(language);
CREATE INDEX IF NOT EXISTS idx_bookmarks_repo ON bookmarks(repo_id);
CREATE INDEX IF NOT EXISTS idx_bookmarks_forked_from ON bookmarks(forked_from);

-- Dedup applies to ordinary (unforked) bookmarks only.
CREATE UNIQUE INDEX IF NOT EXISTS idx_bookmarks_unique
ON bookmarks(repo_id, file_path, query)
WHERE repo_id IS NOT NULL AND forked_from IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_bookmarks_unique_legacy
ON bookmarks(file_path, query)
WHERE repo_id IS NULL AND forked_from IS NULL;

-- V7's inline UNIQUE(file_path, query) applied regardless of repo_id. Keep
-- exactly that guarantee for unforked rows, so a repo-scoped row and a legacy
-- repo_id-less row still can't duplicate each other.
CREATE UNIQUE INDEX IF NOT EXISTS idx_bookmarks_unique_path_query
ON bookmarks(file_path, query)
WHERE forked_from IS NULL;

-- FTS sync triggers were dropped along with the old table; recreate them
-- unchanged (as last defined by V7).
CREATE TRIGGER IF NOT EXISTS bookmarks_ai AFTER INSERT ON bookmarks BEGIN
    INSERT INTO bookmarks_fts(rowid, notes, context, file_path, tags)
        VALUES (new.rowid, '', '', COALESCE(new.file_path, ''), '');
END;

CREATE TRIGGER IF NOT EXISTS bookmarks_ad AFTER DELETE ON bookmarks BEGIN
    INSERT INTO bookmarks_fts(bookmarks_fts, rowid, notes, context, file_path, tags)
        VALUES ('delete', old.rowid, '', '', COALESCE(old.file_path, ''), '');
END;

CREATE TRIGGER IF NOT EXISTS bookmarks_au AFTER UPDATE OF file_path ON bookmarks BEGIN
    INSERT INTO bookmarks_fts(bookmarks_fts, rowid, notes, context, file_path, tags)
        VALUES ('delete', old.rowid, '', '', COALESCE(old.file_path, ''), '');
    INSERT INTO bookmarks_fts(rowid, notes, context, file_path, tags)
        VALUES (new.rowid, '', '', COALESCE(new.file_path, ''), '');
END;
