-- Add visibility to collections
ALTER TABLE collections ADD COLUMN visibility TEXT NOT NULL DEFAULT 'private';

-- Create bookmark_comments table
CREATE TABLE bookmark_comments (
    id TEXT PRIMARY KEY,       -- UUIDv4
    bookmark_id TEXT NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
    author TEXT NOT NULL,      -- user identifier
    body TEXT NOT NULL,        -- comment text (markdown)
    created_at TEXT NOT NULL,  -- ISO 8601 timestamp
    parent_id TEXT,            -- for threading
    FOREIGN KEY (parent_id) REFERENCES bookmark_comments(id) ON DELETE CASCADE
);

CREATE INDEX idx_comments_bookmark ON bookmark_comments(bookmark_id);
CREATE INDEX idx_comments_parent ON bookmark_comments(parent_id);
