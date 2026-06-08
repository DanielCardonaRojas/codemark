CREATE TABLE IF NOT EXISTS known_repos (
    id              TEXT PRIMARY KEY,
    repo_owner      TEXT NOT NULL,
    repo_name       TEXT NOT NULL,
    origin_url      TEXT,
    repo_root       TEXT NOT NULL UNIQUE,
    db_owner_email  TEXT NOT NULL,
    db_owner_name   TEXT,
    detected_at     TEXT NOT NULL,
    last_seen_at    TEXT NOT NULL,
    server_url      TEXT,
    default_username TEXT
);

CREATE INDEX IF NOT EXISTS idx_known_repos_origin ON known_repos(origin_url);
CREATE INDEX IF NOT EXISTS idx_known_repos_root ON known_repos(repo_root);
CREATE INDEX IF NOT EXISTS idx_known_repos_owner_name ON known_repos(repo_owner, repo_name);

CREATE TABLE IF NOT EXISTS accounts (
    server_url      TEXT NOT NULL,
    username        TEXT NOT NULL,
    email           TEXT,
    token           TEXT NOT NULL,
    is_default      INTEGER NOT NULL DEFAULT 0,
    last_used       TEXT,
    PRIMARY KEY (server_url, username)
);

CREATE INDEX IF NOT EXISTS idx_accounts_server ON accounts(server_url);
CREATE INDEX IF NOT EXISTS idx_accounts_email ON accounts(email);
