//! Storage for the live agent bridge (see [`crate::engine::agent`]).
//!
//! Status changes are compare-and-set (`UPDATE … WHERE status = <expected>`),
//! so two processes racing on one delivery can't both claim it.

use std::collections::HashMap;

use rusqlite::{OptionalExtension, Row, params};

use crate::engine::agent::{AgentSession, Delivery, DeliveryStatus, ThreadAgentState, now_rfc3339};
use crate::engine::bookmark::BookmarkComment;
use crate::error::{Error, Result};
use crate::storage::db::Database;

const SESSION_COLS: &str =
    "id, kind, label, external_ref, pid, started_at, heartbeat_at, ended_at, watcher_heartbeat_at";
const DELIVERY_COLS: &str = "id, comment_id, thread_id, session_id, status, created_at, \
     delivered_at, answered_at, answer_comment_id, error";

fn session_from_row(row: &Row<'_>) -> rusqlite::Result<AgentSession> {
    Ok(AgentSession {
        id: row.get(0)?,
        kind: row.get(1)?,
        label: row.get(2)?,
        external_ref: row.get(3)?,
        pid: row.get(4)?,
        started_at: row.get(5)?,
        heartbeat_at: row.get(6)?,
        ended_at: row.get(7)?,
        watcher_heartbeat_at: row.get(8)?,
    })
}

fn delivery_from_row(row: &Row<'_>) -> rusqlite::Result<Delivery> {
    let status: String = row.get(4)?;
    Ok(Delivery {
        id: row.get(0)?,
        comment_id: row.get(1)?,
        thread_id: row.get(2)?,
        session_id: row.get(3)?,
        // An unknown status (a newer writer) reads as failed rather than
        // breaking every query that touches the row.
        status: DeliveryStatus::parse(&status).unwrap_or(DeliveryStatus::Failed),
        created_at: row.get(5)?,
        delivered_at: row.get(6)?,
        answered_at: row.get(7)?,
        answer_comment_id: row.get(8)?,
        error: row.get(9)?,
    })
}

impl Database {
    /// `PRAGMA data_version`: changes whenever *another* connection commits
    /// to this database. Cheap enough to poll every UI tick.
    pub fn data_version(&self) -> Result<i64> {
        Ok(self.conn().query_row("PRAGMA data_version", [], |r| r.get(0))?)
    }

    // ── sessions ────────────────────────────────────────────────────────

    pub fn insert_agent_session(&self, s: &AgentSession) -> Result<()> {
        self.conn().execute(
            &format!(
                "INSERT INTO agent_sessions ({SESSION_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)"
            ),
            params![
                s.id,
                s.kind,
                s.label,
                s.external_ref,
                s.pid,
                s.started_at,
                s.heartbeat_at,
                s.ended_at,
                s.watcher_heartbeat_at
            ],
        )?;
        tracing::debug!(target: "codemark::agent", id = %s.id, kind = %s.kind, "session registered");
        Ok(())
    }

    /// Refresh a session's heartbeat. Errors if the session is gone.
    pub fn heartbeat_agent_session(&self, id: &str) -> Result<()> {
        let n = self.conn().execute(
            "UPDATE agent_sessions SET heartbeat_at = ?2 WHERE id = ?1",
            params![id, now_rfc3339()],
        )?;
        if n == 0 {
            return Err(Error::Input(format!("agent session {id} not found")));
        }
        Ok(())
    }

    /// Mark a `codemark agent watch` as delivering `id`'s replies (`true`,
    /// refreshed as a heartbeat) or as gone (`false`).
    pub fn set_agent_session_watcher(&self, id: &str, active: bool) -> Result<()> {
        self.conn().execute(
            "UPDATE agent_sessions SET watcher_heartbeat_at = ?2 WHERE id = ?1",
            params![id, active.then(now_rfc3339)],
        )?;
        Ok(())
    }

    pub fn end_agent_session(&self, id: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE agent_sessions SET ended_at = ?2 WHERE id = ?1 AND ended_at IS NULL",
            params![id, now_rfc3339()],
        )?;
        tracing::debug!(target: "codemark::agent", id, "session ended");
        Ok(())
    }

    pub fn get_agent_session(&self, id: &str) -> Result<Option<AgentSession>> {
        Ok(self
            .conn()
            .query_row(
                &format!("SELECT {SESSION_COLS} FROM agent_sessions WHERE id = ?1"),
                [id],
                session_from_row,
            )
            .optional()?)
    }

    /// Sessions, newest first. `include_ended` keeps cleanly ended ones.
    pub fn list_agent_sessions(&self, include_ended: bool) -> Result<Vec<AgentSession>> {
        let sql = format!(
            "SELECT {SESSION_COLS} FROM agent_sessions {} ORDER BY started_at DESC",
            if include_ended { "" } else { "WHERE ended_at IS NULL" }
        );
        let mut stmt = self.conn().prepare(&sql)?;
        let rows = stmt.query_map([], session_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ── threads ─────────────────────────────────────────────────────────

    /// Bind the thread rooted at `root_id` to `session_id`, replacing any
    /// earlier binding: replies to it now go to that session.
    pub fn bind_agent_thread(&self, root_id: &str, session_id: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO agent_threads (comment_id, session_id, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(comment_id) DO UPDATE SET session_id = excluded.session_id",
            params![root_id, session_id, now_rfc3339()],
        )?;
        tracing::debug!(target: "codemark::agent", thread = root_id, session = session_id, "thread bound");
        Ok(())
    }

    /// The session that owns the thread rooted at `root_id`, if any.
    pub fn agent_session_for_thread(&self, root_id: &str) -> Result<Option<AgentSession>> {
        Ok(self
            .conn()
            .query_row(
                &format!(
                    "SELECT {} FROM agent_threads t JOIN agent_sessions s ON s.id = t.session_id
                     WHERE t.comment_id = ?1",
                    SESSION_COLS
                        .split(", ")
                        .map(|c| format!("s.{c}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                [root_id],
                session_from_row,
            )
            .optional()?)
    }

    /// Agent state for each bound thread among `root_ids`: its session and
    /// newest delivery. Threads with no binding are absent from the map.
    pub fn agent_thread_states(
        &self,
        root_ids: &[&str],
    ) -> Result<HashMap<String, ThreadAgentState>> {
        let mut out = HashMap::new();
        for root in root_ids {
            let Some(session) = self.agent_session_for_thread(root)? else { continue };
            let latest = self
                .conn()
                .query_row(
                    &format!(
                        "SELECT {DELIVERY_COLS} FROM agent_deliveries WHERE thread_id = ?1
                         ORDER BY created_at DESC LIMIT 1"
                    ),
                    [root],
                    delivery_from_row,
                )
                .optional()?;
            out.insert(root.to_string(), ThreadAgentState { session, latest });
        }
        Ok(out)
    }

    // ── deliveries ──────────────────────────────────────────────────────

    /// Insert a user's reply and, when its thread is bound to an agent
    /// session, a queued delivery for it, in one transaction. Returns the
    /// delivery, or `None` for a thread no agent owns.
    pub fn insert_reply_with_delivery(
        &self,
        reply: &BookmarkComment,
        root_id: &str,
    ) -> Result<Option<Delivery>> {
        let tx = self.conn().unchecked_transaction()?;
        self.insert_comment(reply)?;
        let session = self.agent_session_for_thread(root_id)?;
        let delivery = match session {
            Some(s) => {
                let d = Delivery::new(&reply.id, root_id, &s.id);
                self.insert_delivery(&d)?;
                Some(d)
            }
            None => None,
        };
        tx.commit()?;
        if let Some(d) = &delivery {
            tracing::debug!(target: "codemark::agent", delivery = %d.id, session = %d.session_id, "reply queued for agent");
        }
        Ok(delivery)
    }

    pub fn insert_delivery(&self, d: &Delivery) -> Result<()> {
        self.conn().execute(
            &format!(
                "INSERT INTO agent_deliveries ({DELIVERY_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)"
            ),
            params![
                d.id,
                d.comment_id,
                d.thread_id,
                d.session_id,
                d.status.as_str(),
                d.created_at,
                d.delivered_at,
                d.answered_at,
                d.answer_comment_id,
                d.error
            ],
        )?;
        Ok(())
    }

    pub fn get_delivery(&self, id: &str) -> Result<Option<Delivery>> {
        Ok(self
            .conn()
            .query_row(
                &format!("SELECT {DELIVERY_COLS} FROM agent_deliveries WHERE id = ?1"),
                [id],
                delivery_from_row,
            )
            .optional()?)
    }

    /// Deliveries for `session_id` in `status`, oldest first.
    pub fn deliveries_for_session(
        &self,
        session_id: &str,
        status: DeliveryStatus,
    ) -> Result<Vec<Delivery>> {
        let mut stmt = self.conn().prepare(&format!(
            "SELECT {DELIVERY_COLS} FROM agent_deliveries
             WHERE session_id = ?1 AND status = ?2 ORDER BY created_at ASC"
        ))?;
        let rows = stmt.query_map(params![session_id, status.as_str()], delivery_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Move a delivery from `from` to `to`, stamping the matching time.
    /// Returns `false` when it was no longer in `from` (someone else moved it).
    pub fn transition_delivery(
        &self,
        id: &str,
        from: DeliveryStatus,
        to: DeliveryStatus,
        error: Option<&str>,
    ) -> Result<bool> {
        let stamp = match to {
            DeliveryStatus::Delivered => "delivered_at",
            DeliveryStatus::Answered => "answered_at",
            _ => "created_at", // rewritten to itself below
        };
        let sql = if stamp == "created_at" {
            "UPDATE agent_deliveries SET status = ?3, error = COALESCE(?5, error)
             WHERE id = ?1 AND status = ?2"
                .to_string()
        } else {
            format!(
                "UPDATE agent_deliveries SET status = ?3, {stamp} = ?4, error = COALESCE(?5, error)
                 WHERE id = ?1 AND status = ?2"
            )
        };
        let n = self
            .conn()
            .execute(&sql, params![id, from.as_str(), to.as_str(), now_rfc3339(), error])?;
        tracing::debug!(target: "codemark::agent", delivery = id, from = from.as_str(), to = to.as_str(), moved = n > 0, "delivery transition");
        Ok(n > 0)
    }

    /// Insert an agent's answer to a thread and mark that session's open
    /// deliveries on the thread answered, in one transaction. Returns how
    /// many deliveries it closed.
    pub fn insert_agent_answer(
        &self,
        answer: &BookmarkComment,
        root_id: &str,
        session_id: &str,
    ) -> Result<usize> {
        let tx = self.conn().unchecked_transaction()?;
        self.insert_comment(answer)?;
        let n = self.conn().execute(
            "UPDATE agent_deliveries
             SET status = 'answered', answered_at = ?3, answer_comment_id = ?4
             WHERE thread_id = ?1 AND session_id = ?2 AND status IN ('queued', 'delivered')",
            params![root_id, session_id, now_rfc3339(), answer.id],
        )?;
        tx.commit()?;
        tracing::debug!(target: "codemark::agent", thread = root_id, session = session_id, closed = n, "agent answered");
        Ok(n)
    }

    /// Cancel the open deliveries on a thread (the user gave up waiting).
    pub fn cancel_thread_deliveries(&self, root_id: &str) -> Result<usize> {
        Ok(self.conn().execute(
            "UPDATE agent_deliveries SET status = 'cancelled'
             WHERE thread_id = ?1 AND status IN ('queued', 'delivered')",
            [root_id],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::bookmark::{Bookmark, BookmarkHealth};

    fn bookmark(db: &Database) -> Bookmark {
        let bm = Bookmark {
            id: "bm-1".into(),
            query: "(function_item) @target".into(),
            language: "rust".into(),
            grammar_source: None,
            forked_from: None,
            branch: None,
            file_path: "src/lib.rs".into(),
            content_hash: None,
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            created_by: None,
            current_resolution_id: None,
            repo_id: None,
            tags: Vec::new(),
            annotations: Vec::new(),
            comments: vec![],
        };
        db.insert_bookmark(&bm).unwrap();
        bm
    }

    #[test]
    fn reply_on_bound_thread_queues_and_answer_closes_it() {
        let db = Database::open_in_memory().unwrap();
        let bm = bookmark(&db);
        let session = AgentSession::new("claude-code", Some("claude".into()));
        db.insert_agent_session(&session).unwrap();

        let root = BookmarkComment::new(&bm.id, "claude", "This arm swallows EOF");
        db.insert_comment(&root).unwrap();
        db.bind_agent_thread(&root.id, &session.id).unwrap();

        let reply = BookmarkComment {
            parent_id: Some(root.id.clone()),
            ..BookmarkComment::new(&bm.id, "daniel", "why not in the lexer?")
        };
        let d = db.insert_reply_with_delivery(&reply, &root.id).unwrap().expect("queued");
        assert_eq!(d.status, DeliveryStatus::Queued);
        assert_eq!(
            db.deliveries_for_session(&session.id, DeliveryStatus::Queued).unwrap().len(),
            1
        );

        // Only one claimer wins.
        assert!(
            db.transition_delivery(&d.id, DeliveryStatus::Queued, DeliveryStatus::Delivered, None)
                .unwrap()
        );
        assert!(
            !db.transition_delivery(&d.id, DeliveryStatus::Queued, DeliveryStatus::Delivered, None)
                .unwrap()
        );

        let answer = BookmarkComment {
            parent_id: Some(root.id.clone()),
            ..BookmarkComment::new(&bm.id, "claude", "the lexer can't see the parser state")
        };
        assert_eq!(db.insert_agent_answer(&answer, &root.id, &session.id).unwrap(), 1);
        let d = db.get_delivery(&d.id).unwrap().unwrap();
        assert_eq!(d.status, DeliveryStatus::Answered);
        assert_eq!(d.answer_comment_id.as_deref(), Some(answer.id.as_str()));

        let states = db.agent_thread_states(&[root.id.as_str()]).unwrap();
        assert_eq!(states[&root.id].latest.as_ref().unwrap().status, DeliveryStatus::Answered);
    }

    #[test]
    fn reply_on_unbound_thread_has_no_delivery() {
        let db = Database::open_in_memory().unwrap();
        let bm = bookmark(&db);
        let root = BookmarkComment::new(&bm.id, "daniel", "note to self");
        db.insert_comment(&root).unwrap();
        let reply = BookmarkComment {
            parent_id: Some(root.id.clone()),
            ..BookmarkComment::new(&bm.id, "daniel", "still true")
        };
        assert!(db.insert_reply_with_delivery(&reply, &root.id).unwrap().is_none());
        assert!(db.agent_thread_states(&[root.id.as_str()]).unwrap().is_empty());
    }
}
