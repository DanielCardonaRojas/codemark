-- Migration 021: Add repo_id foreign keys to bookmarks and collections
-- This implements RR-1.1 from the Repo Registry feature.
-- Each bookmark and collection will be associated with a repository.

-- Add repo_id to bookmarks table
ALTER TABLE bookmarks ADD COLUMN repo_id TEXT REFERENCES repos(id);

-- Add repo_id to collections table
ALTER TABLE collections ADD COLUMN repo_id TEXT REFERENCES repos(id);

-- For existing bookmarks/collections, repo_id will be NULL.
-- New bookmarks will populate repo_id from the repos table.
-- The UNIQUE constraint on (file_path, query) will be updated to (repo_id, file_path, query)
-- in a future migration once all existing records have a repo_id.

-- Create index for repo_id lookups
CREATE INDEX IF NOT EXISTS idx_bookmarks_repo ON bookmarks(repo_id);
CREATE INDEX IF NOT EXISTS idx_collections_repo ON collections(repo_id);
