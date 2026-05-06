-- Add breadcrumbs column to resolutions table
-- This column stores a JSON array of structural ancestors for sticky headers.

ALTER TABLE resolutions ADD COLUMN breadcrumbs TEXT;
