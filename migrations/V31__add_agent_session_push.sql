-- V31__add_agent_session_push.sql
--
-- Whether replies are pushed to the session (Claude's channel, the pi
-- extension) or wait for the agent to pull them on its next turn (a plain MCP
-- server for Codex, Gemini and other clients, or `codemark agent register`).
-- The TUI says "waiting for its next turn" for a pull session with no live
-- watcher.
--
-- Unreleased: fold into V29 before the bridge ships.
ALTER TABLE agent_sessions ADD COLUMN push INTEGER NOT NULL DEFAULT 1;
UPDATE agent_sessions SET push = 0 WHERE kind = 'pull';
