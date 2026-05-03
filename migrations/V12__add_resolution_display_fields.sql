-- V12__add_resolution_display_fields.sql
ALTER TABLE resolutions ADD COLUMN headline      TEXT;
ALTER TABLE resolutions ADD COLUMN preview_lines TEXT;
