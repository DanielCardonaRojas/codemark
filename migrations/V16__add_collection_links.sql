-- V16__add_collection_links.sql

CREATE TABLE collection_links (
    id            TEXT PRIMARY KEY,                -- ULID or UUID
    collection_id TEXT NOT NULL,
    kind          TEXT NOT NULL,
    label         TEXT NOT NULL,
    url           TEXT NOT NULL,
    sort_order    INTEGER NOT NULL DEFAULT 0,
    added_at      TEXT NOT NULL,
    added_by      TEXT,
    FOREIGN KEY (collection_id) REFERENCES collections(id) ON DELETE CASCADE,
    CHECK (kind IN ('pr', 'issue', 'doc', 'discussion', 'dashboard', 'repo', 'tour', 'other'))
);

CREATE INDEX idx_collection_links_collection ON collection_links(collection_id, sort_order);
