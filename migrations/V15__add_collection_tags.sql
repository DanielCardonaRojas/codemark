-- V15__add_collection_tags.sql

CREATE TABLE collection_tags (
    collection_id TEXT NOT NULL,
    tag           TEXT NOT NULL,
    added_at      TEXT NOT NULL,
    added_by      TEXT,                            -- agent or user identifier; '__auto__' for auto-populated
    PRIMARY KEY (collection_id, tag),
    FOREIGN KEY (collection_id) REFERENCES collections(id) ON DELETE CASCADE
);

CREATE INDEX idx_collection_tags_tag ON collection_tags(tag);
