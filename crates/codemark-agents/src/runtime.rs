//! What every adapter shares: register a session, claim deliveries, build
//! the event the agent sees, and write the agent's answer back.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use codemark_core::engine::agent::{AgentSession, Delivery, DeliveryStatus};
use codemark_core::engine::bookmark::BookmarkComment;
use codemark_core::error::{Error, Result};
use codemark_core::storage::db::Database;

/// How often adapters look for queued deliveries.
pub const POLL_INTERVAL: Duration = Duration::from_millis(300);
/// How often adapters refresh their session's heartbeat.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// Thread history sent with a delivery is capped at this many bytes, newest
/// kept, so a long thread can't flood the agent's context.
const THREAD_HISTORY_BYTES: usize = 4 * 1024;

/// One reply pushed to an agent: what `codemark agent watch` prints as a JSON
/// line, and what the Claude channel turns into a `<channel>` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub delivery_id: String,
    pub session_id: String,
    /// Root comment of the thread; pass it back to answer.
    pub thread_id: String,
    /// The user's reply.
    pub comment_id: String,
    pub author: String,
    pub body: String,
    pub bookmark: BookmarkRef,
    /// Where the thread's anchor pointed when it was written
    /// (`L43-L47: match tokens.peek() {`), if it is anchored.
    pub anchor_hint: Option<String>,
    /// The thread before this reply, oldest first.
    pub thread: Vec<ThreadEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkRef {
    pub id: String,
    pub file: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadEntry {
    pub author: String,
    pub body: String,
}

/// Register a new session for an agent of `kind`.
pub fn register_session(
    db: &Database,
    kind: &str,
    label: Option<String>,
    external_ref: Option<String>,
) -> Result<AgentSession> {
    let mut session = AgentSession::new(kind, label);
    session.external_ref = external_ref;
    db.insert_agent_session(&session)?;
    Ok(session)
}

/// Build the event for `delivery` from the rows it points at.
pub fn build_event(db: &Database, delivery: &Delivery) -> Result<DeliveryEvent> {
    let reply = db
        .get_comment(&delivery.comment_id)?
        .ok_or_else(|| Error::Operation(format!("reply {} is gone", delivery.comment_id)))?;
    let root = db
        .get_comment(&delivery.thread_id)?
        .ok_or_else(|| Error::Operation(format!("thread {} is gone", delivery.thread_id)))?;
    let bookmark = db.get_bookmark(&root.bookmark_id)?;
    let file = bookmark.map(|b| b.file_path).unwrap_or_default();

    let comments = db.list_comments_for_bookmark(&root.bookmark_id)?;
    let mut thread: Vec<ThreadEntry> = std::iter::once(&root)
        .chain(comments.iter().filter(|c| c.parent_id.as_deref() == Some(root.id.as_str())))
        .filter(|c| c.id != reply.id && c.created_at <= reply.created_at)
        .map(|c| ThreadEntry { author: c.author.clone(), body: c.body.clone() })
        .collect();
    // Keep the newest entries within the byte budget, but always the root.
    let mut used = 0;
    let mut keep_from = thread.len();
    for (i, e) in thread.iter().enumerate().rev() {
        used += e.body.len();
        if used > THREAD_HISTORY_BYTES && i > 0 {
            break;
        }
        keep_from = i;
    }
    if keep_from > 1 {
        thread.drain(1..keep_from);
    }

    Ok(DeliveryEvent {
        kind: "reply".into(),
        delivery_id: delivery.id.clone(),
        session_id: delivery.session_id.clone(),
        thread_id: root.id.clone(),
        comment_id: reply.id,
        author: reply.author,
        body: reply.body,
        bookmark: BookmarkRef { id: root.bookmark_id.clone(), file },
        anchor_hint: root.anchor_hint.clone(),
        thread,
    })
}

/// Claim every queued delivery for `session_id` (queued → delivered) and
/// return their events. A delivery another process claimed first is skipped;
/// one whose rows are gone is marked failed.
pub fn claim_pending(db: &Database, session_id: &str) -> Result<Vec<DeliveryEvent>> {
    let mut events = Vec::new();
    for d in db.deliveries_for_session(session_id, DeliveryStatus::Queued)? {
        match build_event(db, &d) {
            Ok(event) => {
                if db.transition_delivery(
                    &d.id,
                    DeliveryStatus::Queued,
                    DeliveryStatus::Delivered,
                    None,
                )? {
                    events.push(event);
                }
            }
            Err(e) => {
                tracing::warn!(target: "codemark::agent", delivery = %d.id, error = %e, "delivery failed");
                db.transition_delivery(
                    &d.id,
                    DeliveryStatus::Queued,
                    DeliveryStatus::Failed,
                    Some(&e.to_string()),
                )?;
            }
        }
    }
    Ok(events)
}

/// Every reply still waiting on `session_id`: queued ones are claimed, and
/// ones already pushed but never answered are included again. A push can be
/// dropped silently (Claude Code ignores channel events when channels are
/// unavailable), so an explicit inbox read must not trust "delivered".
pub fn open_events(db: &Database, session_id: &str) -> Result<Vec<DeliveryEvent>> {
    let mut events = Vec::new();
    for d in db.deliveries_for_session(session_id, DeliveryStatus::Delivered)? {
        match build_event(db, &d) {
            Ok(e) => events.push(e),
            Err(e) => {
                tracing::warn!(target: "codemark::agent", delivery = %d.id, error = %e, "unreadable delivery");
            }
        }
    }
    events.extend(claim_pending(db, session_id)?);
    Ok(events)
}

/// Resolve a thread id (full or unambiguous prefix, root or any reply) to
/// its root comment.
pub fn thread_root(db: &Database, id: &str) -> Result<BookmarkComment> {
    let mut current = db
        .get_comment_by_prefix(id)?
        .ok_or_else(|| Error::Input(format!("no comment with id {id}")))?;
    for _ in 0..1000 {
        match current.parent_id.as_deref().map(|p| db.get_comment(p)).transpose()?.flatten() {
            Some(parent) => current = parent,
            None => return Ok(current),
        }
    }
    Err(Error::Operation(format!("comment {id} has a reply cycle")))
}

/// Write `session`'s answer to the thread holding `thread_id` and close its
/// open deliveries there. Returns the new comment.
pub fn answer(
    db: &Database,
    session: &AgentSession,
    thread_id: &str,
    body: &str,
) -> Result<BookmarkComment> {
    let root = thread_root(db, thread_id)?;
    let comment = BookmarkComment {
        parent_id: Some(root.id.clone()),
        ..BookmarkComment::new(&root.bookmark_id, session.display_name(), body)
    };
    db.insert_agent_answer(&comment, &root.id, &session.id)?;
    Ok(comment)
}

/// Who is running a [`watch_loop`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchRole {
    /// The adapter that owns the session (the Claude channel server). It
    /// keeps the session alive and yields deliveries to a live
    /// [`WatchRole::Watcher`].
    Owner,
    /// A `codemark agent watch` attached to the session (for example under
    /// Claude Code's Monitor). It refreshes the session's watcher heartbeat so
    /// the owner yields, and clears it when it stops.
    Watcher,
}

/// Run `on_events` with each batch of newly claimed deliveries until `stop`
/// is set, refreshing the session's heartbeats as it goes. Opens its own
/// connection, so it can run on a background thread.
pub fn watch_loop(
    db_path: &Path,
    session_id: &str,
    role: WatchRole,
    stop: Arc<AtomicBool>,
    mut on_events: impl FnMut(Vec<DeliveryEvent>) -> Result<()>,
) -> Result<()> {
    let db = Database::open(db_path)?;
    let beat = |db: &Database| -> Result<()> {
        db.heartbeat_agent_session(session_id)?;
        if role == WatchRole::Watcher {
            db.set_agent_session_watcher(session_id, true)?;
        }
        Ok(())
    };
    beat(&db)?;
    let mut last_beat = Instant::now();
    let result = (|| {
        while !stop.load(Ordering::Relaxed) {
            if last_beat.elapsed() >= HEARTBEAT_INTERVAL {
                beat(&db)?;
                last_beat = Instant::now();
            }
            // The owner leaves deliveries to a live watcher: a channel push
            // can be dropped silently, a watcher's line can't.
            let yield_to_watcher = role == WatchRole::Owner
                && db.get_agent_session(session_id)?.is_some_and(|s| s.has_live_watcher());
            if !yield_to_watcher {
                let events = claim_pending(&db, session_id)?;
                if !events.is_empty() {
                    tracing::debug!(target: "codemark::agent", session = session_id, n = events.len(), ?role, "deliveries claimed");
                    on_events(events)?;
                }
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        Ok(())
    })();
    if role == WatchRole::Watcher {
        let _ = db.set_agent_session_watcher(session_id, false);
    }
    result
}

/// One line for `event`, for line-oriented consumers such as Claude Code's
/// Monitor tool, where every stdout line becomes a notification. Newlines in
/// the body are flattened so one reply is one line.
pub fn format_oneline(event: &DeliveryEvent) -> String {
    let flat = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut line =
        format!("codemark reply thread_id={} file={}", event.thread_id, event.bookmark.file);
    if let Some(hint) = &event.anchor_hint {
        line.push_str(&format!(" at=\"{}\"", flat(hint).replace('"', "'")));
    }
    line.push_str(&format!(" | {}: {}", event.author, flat(&event.body)));
    if let Some(root) = event.thread.first() {
        let mut quoted = flat(&root.body);
        if quoted.chars().count() > 160 {
            quoted = quoted.chars().take(159).collect::<String>() + "…";
        }
        line.push_str(&format!(" | thread started by {}: {}", root.author, quoted));
    }
    line.push_str(" | answer with codemark_reply");
    line
}

/// The database the current directory's repo uses.
pub fn current_db_path(explicit: Option<&Path>) -> Result<PathBuf> {
    let db = codemark_core::storage::Workspace::open_primary(explicit)?;
    Ok(db.path().to_path_buf())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use codemark_core::engine::bookmark::{Bookmark, BookmarkHealth};

    pub(crate) fn seeded() -> (Database, AgentSession, BookmarkComment) {
        let db = Database::open_in_memory().unwrap();
        let bm = Bookmark {
            id: "bm-1".into(),
            query: "(function_item) @target".into(),
            language: "rust".into(),
            grammar_source: None,
            forked_from: None,
            branch: None,
            file_path: "src/parser.rs".into(),
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
        let session = register_session(&db, "claude-code", Some("claude".into()), None).unwrap();
        let root = BookmarkComment {
            anchor_hint: Some("L43-L47: match tokens.peek() {".into()),
            ..BookmarkComment::new(&bm.id, "claude", "This arm swallows EOF.")
        };
        db.insert_comment(&root).unwrap();
        db.bind_agent_thread(&root.id, &session.id).unwrap();
        (db, session, root)
    }

    #[test]
    fn claim_builds_event_and_answer_closes_delivery() {
        let (db, session, root) = seeded();
        let reply = BookmarkComment {
            parent_id: Some(root.id.clone()),
            ..BookmarkComment::new(&root.bookmark_id, "daniel", "Why not fix it in the lexer?")
        };
        let d = db.insert_reply_with_delivery(&reply, &root.id).unwrap().unwrap();

        let events = claim_pending(&db, &session.id).unwrap();
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.thread_id, root.id);
        assert_eq!(e.body, "Why not fix it in the lexer?");
        assert_eq!(e.bookmark.file, "src/parser.rs");
        assert_eq!(e.anchor_hint.as_deref(), Some("L43-L47: match tokens.peek() {"));
        assert_eq!(
            e.thread,
            vec![ThreadEntry { author: "claude".into(), body: "This arm swallows EOF.".into() }]
        );
        // Claimed once only.
        assert!(claim_pending(&db, &session.id).unwrap().is_empty());

        // Answer by a prefix of the reply id: resolves to the root.
        let ans = answer(&db, &session, &reply.id[..8], "The lexer has no parser state.").unwrap();
        assert_eq!(ans.parent_id.as_deref(), Some(root.id.as_str()));
        assert_eq!(ans.author, "claude");
        assert_eq!(db.get_delivery(&d.id).unwrap().unwrap().status, DeliveryStatus::Answered);
    }

    #[test]
    fn oneline_is_one_line_with_the_thread_id() {
        let (db, session, root) = seeded();
        let reply = BookmarkComment {
            parent_id: Some(root.id.clone()),
            ..BookmarkComment::new(&root.bookmark_id, "daniel", "two\nlines")
        };
        db.insert_reply_with_delivery(&reply, &root.id).unwrap();
        let e = claim_pending(&db, &session.id).unwrap().remove(0);
        let line = format_oneline(&e);
        assert!(!line.contains('\n'), "{line}");
        assert!(line.contains(&format!("thread_id={}", root.id)), "{line}");
        assert!(line.contains("daniel: two lines"), "{line}");
    }

    #[test]
    fn owner_yields_to_a_live_watcher() {
        let (db, session, root) = seeded();
        db.set_agent_session_watcher(&session.id, true).unwrap();
        assert!(db.get_agent_session(&session.id).unwrap().unwrap().has_live_watcher());
        db.set_agent_session_watcher(&session.id, false).unwrap();
        assert!(!db.get_agent_session(&session.id).unwrap().unwrap().has_live_watcher());
        let _ = root;
    }

    #[test]
    fn open_events_resurface_pushed_but_unanswered_replies() {
        let (db, session, root) = seeded();
        let reply = BookmarkComment {
            parent_id: Some(root.id.clone()),
            ..BookmarkComment::new(&root.bookmark_id, "daniel", "still there?")
        };
        db.insert_reply_with_delivery(&reply, &root.id).unwrap();
        // The push "happened" but the agent never saw it.
        assert_eq!(claim_pending(&db, &session.id).unwrap().len(), 1);
        assert!(claim_pending(&db, &session.id).unwrap().is_empty());
        let open = open_events(&db, &session.id).unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].body, "still there?");
        // Answered: gone from the inbox.
        answer(&db, &session, &root.id, "yes").unwrap();
        assert!(open_events(&db, &session.id).unwrap().is_empty());
    }
}
