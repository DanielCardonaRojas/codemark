-- V20__rename_preview_lines_to_snapshot.sql
-- Rename preview_lines to snapshot to reflect the "Exact Snapshot" model.
-- Also adds optional padding columns for future use.

-- Rename the column
ALTER TABLE resolutions RENAME COLUMN preview_lines TO snapshot;

-- Add optional padding columns (NULL for now, for future use)
ALTER TABLE resolutions ADD COLUMN snapshot_top_padding INTEGER DEFAULT NULL;
ALTER TABLE resolutions ADD COLUMN snapshot_bottom_padding INTEGER DEFAULT NULL;
