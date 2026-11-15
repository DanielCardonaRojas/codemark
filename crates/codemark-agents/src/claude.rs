//! The Claude Code channel: a stdio MCP server Claude Code spawns.
//!
//! It declares the `claude/channel` capability, registers an agent session,
//! and pushes each reply to a thread this session owns as a
//! `notifications/claude/channel` event. The tools and instructions are the
//! shared [`AgentServer`]'s; Claude answers with `codemark_reply`. See
//! <https://code.claude.com/docs/en/channels-reference>.
//!
//! Channels are a research preview: Claude Code must be started with
//! `--dangerously-load-development-channels server:codemark` (or `--channels`
//! once allowlisted). Without it Claude Code still spawns this server as a
//! plain MCP server and drops the events, so `codemark_inbox` offers the same
//! replies on demand.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

use codemark_core::storage::db::Database;

use crate::mcp::{self, Outbound};
use crate::runtime::{self, DeliveryEvent};
use crate::server::{AgentServer, Delivery};

/// Session kind recorded for Claude Code.
pub const KIND: &str = "claude-code";

/// The `notifications/claude/channel` params for `event`. `meta` keys must
/// be identifiers (letters, digits, `_`): others are dropped by Claude Code.
pub fn channel_params(event: &DeliveryEvent) -> Value {
    let content = runtime::describe_event(event);
    json!({
        "content": content,
        "meta": {
            "thread_id": event.thread_id,
            "delivery_id": event.delivery_id,
            "bookmark_id": event.bookmark.id,
            "file": event.bookmark.file,
            "author": event.author,
        }
    })
}

/// Run the channel on stdio until Claude Code closes stdin.
pub fn run(
    db_path: &Path,
    label: Option<String>,
    codemark_exe: Option<PathBuf>,
) -> codemark_core::error::Result<()> {
    let db = Database::open(db_path)?;
    let session = runtime::register_session(
        &db,
        KIND,
        Some(label.unwrap_or_else(|| "claude".into())),
        std::env::var("CLAUDE_SESSION_ID").ok(),
        true,
    )?;
    drop(db);
    tracing::info!(target: "codemark::agent", session = %session.id, db = %db_path.display(), "claude channel started");

    let out = Outbound::new(std::io::stdout());
    let stop = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (out, stop, path, sid) =
            (out.clone(), stop.clone(), db_path.to_path_buf(), session.id.clone());
        std::thread::spawn(move || {
            let res = runtime::watch_loop(&path, &sid, runtime::WatchRole::Owner, stop, |events| {
                for e in &events {
                    out.notify("notifications/claude/channel", channel_params(e))
                        .map_err(|e| codemark_core::error::Error::Operation(e.to_string()))?;
                }
                Ok(())
            });
            if let Err(e) = res {
                tracing::error!(target: "codemark::agent", error = %e, "channel watcher stopped");
            }
        })
    };

    let server = AgentServer::new(db_path, session.clone(), codemark_exe, Delivery::ClaudeChannel)?;
    let served = mcp::serve(std::io::stdin().lock(), &out, &server);
    stop.store(true, Ordering::Relaxed);
    let _ = watcher.join();
    server.db.end_agent_session(&session.id)?;
    served.map_err(|e| codemark_core::error::Error::Operation(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::tests::seeded;
    use codemark_core::engine::bookmark::BookmarkComment;

    #[test]
    fn channel_params_carry_routing_meta_and_thread() {
        let (db, session, root) = seeded();
        let reply = BookmarkComment {
            parent_id: Some(root.id.clone()),
            ..BookmarkComment::new(&root.bookmark_id, "daniel", "Why not the lexer?")
        };
        db.insert_reply_with_delivery(&reply, &root.id).unwrap();
        let e = runtime::claim_pending(&db, &session.id).unwrap().remove(0);
        let p = channel_params(&e);
        assert_eq!(p["meta"]["thread_id"], root.id);
        let content = p["content"].as_str().unwrap();
        assert!(content.contains("daniel replied"), "{content}");
        assert!(content.contains("src/parser.rs"), "{content}");
        assert!(content.contains("> claude: This arm swallows EOF."), "{content}");
        for key in p["meta"].as_object().unwrap().keys() {
            assert!(key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'), "{key}");
        }
    }
}
