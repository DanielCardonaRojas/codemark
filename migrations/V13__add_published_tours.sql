-- Migration 013: Add published_tours table to track local collections published to servers
CREATE TABLE IF NOT EXISTS published_tours (
    source_collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    server_url TEXT NOT NULL,
    tour_id TEXT NOT NULL,
    last_published_at TEXT NOT NULL,
    PRIMARY KEY (source_collection_id, server_url)
);
