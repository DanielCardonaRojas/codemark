-- V17__rename_bookmark_status_to_health.sql

ALTER TABLE bookmarks RENAME COLUMN status TO health;
DROP INDEX IF EXISTS idx_bookmarks_status;
CREATE INDEX idx_bookmarks_health ON bookmarks(health);
