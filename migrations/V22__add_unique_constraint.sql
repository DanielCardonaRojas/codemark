-- Migration 022: Add unique constraint for bookmarks with repo_id
-- This implements the final part of RR-1.1 from the Repo Registry feature.

-- Create a partial unique index that only applies to records with repo_id set.
-- This allows existing records with NULL repo_id to coexist while preventing
-- duplicates for records that have been associated with a repository.
CREATE UNIQUE INDEX IF NOT EXISTS idx_bookmarks_unique
ON bookmarks(repo_id, file_path, query)
WHERE repo_id IS NOT NULL;

-- For records without repo_id (legacy/migrated), use a filtered unique constraint
-- that only considers non-NULL file_path and query combinations where repo_id IS NULL
CREATE UNIQUE INDEX IF NOT EXISTS idx_bookmarks_unique_legacy
ON bookmarks(file_path, query)
WHERE repo_id IS NULL;
