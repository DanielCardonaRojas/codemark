-- Migration 001: Initial schema
-- Creates all core tables for the codemark bookmark system.

CREATE TABLE IF NOT EXISTS schema_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT OR IGNORE INTO schema_meta (key, value) VALUES ('created_at', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));

CREATE TABLE IF NOT EXISTS bookmarks (
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
    tags              TEXT,
    notes             TEXT,
    context           TEXT
);

CREATE INDEX IF NOT EXISTS idx_bookmarks_status ON bookmarks(status);
CREATE INDEX IF NOT EXISTS idx_bookmarks_file ON bookmarks(file_path);
CREATE INDEX IF NOT EXISTS idx_bookmarks_language ON bookmarks(language);

CREATE TABLE IF NOT EXISTS resolutions (
    id            TEXT PRIMARY KEY,
    bookmark_id   TEXT NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
    resolved_at   TEXT NOT NULL,
    commit_hash   TEXT,
    method        TEXT NOT NULL,
    match_count   INTEGER,
    file_path     TEXT,
    byte_range    TEXT,
    content_hash  TEXT
);

CREATE INDEX IF NOT EXISTS idx_resolutions_bookmark ON resolutions(bookmark_id);
CREATE INDEX IF NOT EXISTS idx_resolutions_resolved ON resolutions(resolved_at);

CREATE TABLE IF NOT EXISTS collections (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    description TEXT,
    created_at  TEXT NOT NULL,
    created_by  TEXT
);

CREATE INDEX IF NOT EXISTS idx_collections_name ON collections(name);

CREATE TABLE IF NOT EXISTS collection_bookmarks (
    collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    bookmark_id   TEXT NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
    added_at      TEXT NOT NULL,
    PRIMARY KEY (collection_id, bookmark_id)
);

CREATE INDEX IF NOT EXISTS idx_cb_bookmark ON collection_bookmarks(bookmark_id);
