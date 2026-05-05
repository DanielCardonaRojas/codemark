-- V18__add_collection_health.sql

ALTER TABLE collections ADD COLUMN health              TEXT
    CHECK (health IS NULL OR health IN ('active', 'drifted', 'stale'));
ALTER TABLE collections ADD COLUMN health_computed_at  TEXT;
