-- Migration 014: Add imported_from_url column to collections table
ALTER TABLE collections ADD COLUMN imported_from_url TEXT;
