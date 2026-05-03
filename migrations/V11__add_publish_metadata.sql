-- V11__add_publish_metadata.sql
-- Migration 011: Add publish metadata and relax collection name uniqueness
-- This requires a table rebuild for the collections table to remove the UNIQUE constraint.

-- 1. Create the new collections table with the new columns and without the UNIQUE constraint on name
CREATE TABLE collections_new (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT,
    visibility  TEXT NOT NULL DEFAULT 'private',
    created_at  TEXT NOT NULL,
    created_by  TEXT,
    created_branch TEXT,
    published_at         TEXT,
    published_commit_sha TEXT,
    repo_url             TEXT,
    status               TEXT CHECK (status IS NULL OR status IN ('processing', 'ready', 'error')),
    updated_at           TEXT
);

-- 2. Copy data from the old table
INSERT INTO collections_new (id, name, description, visibility, created_at, created_by, created_branch)
SELECT id, name, description, visibility, created_at, created_by, created_branch FROM collections;

-- 3. Drop old table and rename new one
DROP TABLE collections;
ALTER TABLE collections_new RENAME TO collections;

-- 4. Recreate indices
CREATE INDEX idx_collections_name ON collections(name);
