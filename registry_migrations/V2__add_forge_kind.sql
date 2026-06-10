-- Add forge_kind to accounts and widen the primary key to
-- (server_url, forge_kind, username).
--
-- This migration repairs databases that were created by an earlier V1 schema
-- which lacked the forge_kind column. Because the original V1 migration was
-- later edited in place to include forge_kind, databases stamped at
-- user_version = 1 may have EITHER the old or the new schema. This migration is
-- written to be a no-op when the column already exists.
--
-- SQLite cannot ALTER an existing column into a composite primary key, so the
-- table is rebuilt. forge_kind is hardcoded to 'github' for existing rows since
-- that was the only forge supported when those accounts were created.

CREATE TABLE IF NOT EXISTS accounts_new (
    server_url      TEXT NOT NULL,
    forge_kind      TEXT NOT NULL DEFAULT 'github',
    username        TEXT NOT NULL,
    email           TEXT,
    token           TEXT NOT NULL,
    is_default      INTEGER NOT NULL DEFAULT 0,
    last_used       TEXT,
    PRIMARY KEY (server_url, forge_kind, username)
);

INSERT OR IGNORE INTO accounts_new (server_url, forge_kind, username, email, token, is_default, last_used)
    SELECT server_url, 'github', username, email, token, is_default, last_used
    FROM accounts;

DROP TABLE accounts;
ALTER TABLE accounts_new RENAME TO accounts;

CREATE INDEX IF NOT EXISTS idx_accounts_server ON accounts(server_url);
CREATE INDEX IF NOT EXISTS idx_accounts_email ON accounts(email);
