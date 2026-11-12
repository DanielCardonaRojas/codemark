-- V30__add_agent_session_watcher.sql
--
-- Heartbeat of a `codemark agent watch` delivering a session's replies (for
-- example under Claude Code's Monitor tool, when channels are unavailable).
-- While it is fresh, the session's channel adapter leaves deliveries to the
-- watcher instead of pushing them into a channel that may drop them.
--
-- Unreleased: fold into V29 before the bridge ships.
ALTER TABLE agent_sessions ADD COLUMN watcher_heartbeat_at TEXT;
