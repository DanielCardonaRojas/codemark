-- V26__add_artifact_link_kind.sql
--
-- Adds the `artifact` collection link kind, for URLs of artifacts an agent
-- produced during a session (plans, generated docs/diagrams, gists, reports).
-- SQLite cannot alter a CHECK constraint in place, so the table is rebuilt.
-- Nothing holds a foreign key into collection_links, so the rebuild is safe
-- with foreign_keys enabled.

CREATE TABLE collection_links_new (
    id            TEXT PRIMARY KEY NOT NULL,       -- ULID or UUID
    collection_id TEXT NOT NULL,
    kind          TEXT NOT NULL,
    label         TEXT NOT NULL,
    url           TEXT NOT NULL,
    sort_order    INTEGER NOT NULL DEFAULT 0,
    added_at      TEXT NOT NULL,
    added_by      TEXT,
    FOREIGN KEY (collection_id) REFERENCES collections(id) ON DELETE CASCADE,
    CHECK (kind IN ('pr', 'issue', 'doc', 'discussion', 'dashboard', 'repo', 'tour', 'artifact', 'other'))
);

INSERT INTO collection_links_new (id, collection_id, kind, label, url, sort_order, added_at, added_by)
SELECT id, collection_id, kind, label, url, sort_order, added_at, added_by FROM collection_links;

DROP TABLE collection_links;
ALTER TABLE collection_links_new RENAME TO collection_links;

CREATE INDEX idx_collection_links_collection ON collection_links(collection_id, sort_order);
