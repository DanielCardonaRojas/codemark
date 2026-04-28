-- Migration 008: Add user identity and repository metadata
-- This creates a repos table for tracking repository metadata and database ownership.
-- Each database has its own repos table that identifies its owner and the repos worked on.

-- Create repository metadata table
CREATE TABLE IF NOT EXISTS repos (
    id TEXT PRIMARY KEY,
    repo_owner TEXT NOT NULL,      -- Extracted from origin URL (e.g., "owner" from github.com/owner/repo)
    repo_name TEXT NOT NULL,       -- Repository name
    origin_url TEXT UNIQUE,        -- Full git remote origin URL
    repo_root TEXT UNIQUE NOT NULL, -- Absolute path to repository root (unique constraint prevents duplicates for local repos)
    db_owner_email TEXT NOT NULL,  -- The owner of this database (git user.email)
    db_owner_name TEXT,            -- The owner of this database (git user.name)
    detected_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_repos_owner ON repos(repo_owner);
CREATE INDEX IF NOT EXISTS idx_repos_name ON repos(repo_name);
CREATE INDEX IF NOT EXISTS idx_repos_db_owner ON repos(db_owner_email);
