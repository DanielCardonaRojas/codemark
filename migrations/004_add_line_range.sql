-- Migration 004: Add line_range to resolutions
-- Store line numbers (1-indexed, inclusive) alongside byte ranges.
-- This allows preview to work without reading files for byte-to-line conversion.

ALTER TABLE resolutions ADD COLUMN line_range TEXT;
