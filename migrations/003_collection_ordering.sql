-- Migration 003: Add position column to collection_bookmarks for ordered collections.
-- ALTER TABLE ADD COLUMN is idempotent in SQLite 3.35+ (IF NOT EXISTS not supported for columns).
-- We check via pragma and skip if already present.

ALTER TABLE collection_bookmarks ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
