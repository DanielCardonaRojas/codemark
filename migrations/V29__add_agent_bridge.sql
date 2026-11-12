-- V29__add_agent_bridge.sql
--
-- Live agent bridge: a reply to a comment an agent wrote is pushed to that
-- agent's running session, and its answer lands back in the same thread.
-- The database is the bus: the TUI, the CLI and per-agent adapter processes
-- all read and write these tables; nothing else carries the messages.
--
-- All three tables are machine-local. Packs, sync and the registry server
-- never carry them: a session id means nothing on another machine. The pack
-- writer drops them and the pack inspector accepts them only when empty.
--
-- IF NOT EXISTS: tests (and pack pulls) re-run migrations from an older
-- user_version over a database that already has these tables.

-- One row per agent session an adapter registered (live or past).
CREATE TABLE IF NOT EXISTS agent_sessions (
    id           TEXT PRIMARY KEY,  -- UUIDv4 minted by the adapter
    kind         TEXT NOT NULL,     -- 'claude-code', 'codex', 'pi', 'pull', ...
    label        TEXT,              -- shown in the TUI, e.g. 'claude'
    external_ref TEXT,              -- agent's own session/thread id, for resume
    pid          INTEGER,           -- adapter process id
    started_at   TEXT NOT NULL,
    heartbeat_at TEXT NOT NULL,     -- live while recent (see AgentSession::is_live)
    ended_at     TEXT               -- set on clean shutdown
);

-- Threads an agent session owns: replies to them are delivered to it.
-- Keyed by the thread's root comment.
CREATE TABLE IF NOT EXISTS agent_threads (
    comment_id TEXT PRIMARY KEY REFERENCES bookmark_comments(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL
);

-- One row per user reply that must reach an agent session.
-- status: queued -> delivered -> answered, or failed / expired / cancelled.
CREATE TABLE IF NOT EXISTS agent_deliveries (
    id                TEXT PRIMARY KEY,
    comment_id        TEXT NOT NULL REFERENCES bookmark_comments(id) ON DELETE CASCADE,
    thread_id         TEXT NOT NULL,   -- root comment of the thread
    session_id        TEXT NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    status            TEXT NOT NULL,
    created_at        TEXT NOT NULL,
    delivered_at      TEXT,
    answered_at       TEXT,
    answer_comment_id TEXT,
    error             TEXT
);

CREATE INDEX IF NOT EXISTS idx_agent_deliveries_session_status ON agent_deliveries(session_id, status);
CREATE INDEX IF NOT EXISTS idx_agent_deliveries_thread ON agent_deliveries(thread_id);
CREATE INDEX IF NOT EXISTS idx_agent_threads_session ON agent_threads(session_id);
