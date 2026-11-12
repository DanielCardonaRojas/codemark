//! The Claude Code channel: a stdio MCP server Claude Code spawns.
//!
//! It declares the `claude/channel` capability, registers an agent session,
//! and pushes each reply to a thread this session owns as a
//! `notifications/claude/channel` event. Claude answers with the
//! `codemark_reply` tool. See
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

use codemark_core::engine::agent::AgentSession;
use codemark_core::storage::db::Database;

use crate::mcp::{self, Handler, Outbound, arg_str};
use crate::runtime::{self, DeliveryEvent};

/// Session kind recorded for Claude Code.
pub const KIND: &str = "claude-code";

/// The `notifications/claude/channel` params for `event`. `meta` keys must
/// be identifiers (letters, digits, `_`): others are dropped by Claude Code.
pub fn channel_params(event: &DeliveryEvent) -> Value {
    let mut content = format!(
        "{} replied in a codemark thread you own on `{}`",
        event.author, event.bookmark.file
    );
    if let Some(hint) = &event.anchor_hint {
        content.push_str(&format!(" ({hint})"));
    }
    content.push_str(":\n\n");
    content.push_str(&event.body);
    if !event.thread.is_empty() {
        content.push_str("\n\nThread so far:");
        for e in &event.thread {
            content.push_str(&format!("\n> {}: {}", e.author, e.body.replace('\n', "\n>   ")));
        }
    }
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

/// The shell command that streams this session's replies one per line, for
/// Claude Code's Monitor tool. Paths are single-quoted for the shell.
pub fn watch_command(exe: Option<&Path>, db_path: &Path, session_id: &str) -> String {
    let quote = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', r"'\''"));
    let exe = exe.map(quote).unwrap_or_else(|| "codemark".into());
    format!("{exe} agent watch --session {session_id} --oneline --db {}", quote(db_path))
}

fn instructions(session: &AgentSession, watch: &str) -> String {
    format!(
        "Codemark delivers replies to code comments you wrote in this session. Each one \
is a person answering a comment you left on bookmarked code. Read the code at that \
location if you need to, then answer with the codemark_reply tool, passing the \
thread_id. Keep answers short, like a code-review reply. If you change code because \
of a reply, say so in your answer.\n\
\n\
Receiving replies: as soon as the session starts, before anything else, start a \
background Monitor (timeout_ms 1800000, description \"codemark replies\") running \
exactly:\n  {watch}\n\
Each line it prints is one reply (`codemark reply thread_id=... | author: text`). \
When the monitor expires, start it again with the same command: replies that arrived \
meanwhile are delivered on restart. Do this even if replies also arrive as \
<channel source=\"codemark\"> events: while the monitor runs, the channel stays quiet, \
so you never get a reply twice. If you cannot start a Monitor, call codemark_inbox \
whenever the user mentions a reply.\n\
\n\
To start a thread the user can answer live, use codemark_comment (bookmark_id, body, \
optional range). Threads you create with the codemark CLI are live too when you pass \
--session {id} to `codemark comment add`; codemark_bind_thread binds an existing thread.",
        id = session.id
    )
}

/// The channel's MCP handler.
pub struct Channel {
    db: Database,
    db_path: PathBuf,
    session: AgentSession,
    /// The codemark binary, for `codemark_comment` (anchoring lives in the
    /// CLI). `None` disables the tool.
    codemark_exe: Option<PathBuf>,
}

impl Channel {
    pub fn new(
        db_path: &Path,
        session: AgentSession,
        codemark_exe: Option<PathBuf>,
    ) -> codemark_core::error::Result<Self> {
        Ok(Self {
            db: Database::open(db_path)?,
            db_path: db_path.to_path_buf(),
            session,
            codemark_exe,
        })
    }

    fn reply(&self, args: &Value) -> Result<String, String> {
        let thread = arg_str(args, "thread_id")?;
        let body = arg_str(args, "body")?;
        let c =
            runtime::answer(&self.db, &self.session, thread, body).map_err(|e| e.to_string())?;
        Ok(format!("Replied in thread {} (comment {}).", c.parent_id.unwrap_or_default(), c.id))
    }

    fn bind(&self, args: &Value) -> Result<String, String> {
        let id = arg_str(args, "comment_id")?;
        let root = runtime::thread_root(&self.db, id).map_err(|e| e.to_string())?;
        self.db.bind_agent_thread(&root.id, &self.session.id).map_err(|e| e.to_string())?;
        Ok(format!("Thread {} is now live: replies to it reach this session.", root.id))
    }

    fn inbox(&self) -> Result<String, String> {
        let events = runtime::open_events(&self.db, &self.session.id).map_err(|e| e.to_string())?;
        if events.is_empty() {
            return Ok("No pending replies.".into());
        }
        let parts: Vec<String> = events
            .iter()
            .map(|e| {
                let p = channel_params(e);
                format!(
                    "[thread_id={}]\n{}",
                    e.thread_id,
                    p["content"].as_str().unwrap_or_default()
                )
            })
            .collect();
        Ok(parts.join("\n\n---\n\n"))
    }

    fn comment(&self, args: &Value) -> Result<String, String> {
        let exe = self.codemark_exe.as_ref().ok_or("codemark_comment is unavailable")?;
        let bookmark = arg_str(args, "bookmark_id")?;
        let body = arg_str(args, "body")?;
        let mut cmd = std::process::Command::new(exe);
        cmd.args(["comment", "add", bookmark, "--body", body])
            .arg("--created-by")
            .arg(self.session.display_name())
            .arg("--session")
            .arg(&self.session.id)
            .arg("--db")
            .arg(&self.db_path);
        if let Some(range) = args.get("range").and_then(Value::as_str).filter(|r| !r.is_empty()) {
            cmd.args(["--range", range]);
        }
        let out = cmd.output().map_err(|e| format!("failed to run codemark: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        if out.status.success() { Ok(text) } else { Err(text) }
    }
}

impl Handler for Channel {
    fn capabilities(&self) -> Value {
        json!({"experimental": {"claude/channel": {}}, "tools": {}})
    }

    fn instructions(&self) -> Option<String> {
        let watch = watch_command(self.codemark_exe.as_deref(), &self.db_path, &self.session.id);
        Some(instructions(&self.session, &watch))
    }

    fn server_name(&self) -> &str {
        "codemark"
    }

    fn tools(&self) -> Vec<Value> {
        let mut tools = vec![
            json!({
                "name": "codemark_reply",
                "description": "Answer a codemark comment thread. Use the thread_id from a <channel source=\"codemark\"> event.",
                "inputSchema": {"type": "object", "properties": {
                    "thread_id": {"type": "string", "description": "Thread (root comment) id from the channel tag"},
                    "body": {"type": "string", "description": "Markdown answer"}
                }, "required": ["thread_id", "body"]}
            }),
            json!({
                "name": "codemark_bind_thread",
                "description": "Make an existing codemark comment thread live: replies to it will be pushed to this session.",
                "inputSchema": {"type": "object", "properties": {
                    "comment_id": {"type": "string", "description": "Any comment id in the thread (full or prefix)"}
                }, "required": ["comment_id"]}
            }),
            json!({
                "name": "codemark_inbox",
                "description": "Fetch replies to your codemark threads that you have not answered yet.",
                "inputSchema": {"type": "object", "properties": {}}
            }),
        ];
        if self.codemark_exe.is_some() {
            tools.push(json!({
                "name": "codemark_comment",
                "description": "Comment on a bookmark, optionally on one node inside it, as a live thread the user can answer.",
                "inputSchema": {"type": "object", "properties": {
                    "bookmark_id": {"type": "string"},
                    "body": {"type": "string", "description": "Markdown comment"},
                    "range": {"type": "string", "description": "Optional line range in the bookmark's file: 42, 42-67 or 10:5-12:20"}
                }, "required": ["bookmark_id", "body"]}
            }));
        }
        tools
    }

    fn call_tool(&self, name: &str, args: &Value) -> Result<String, String> {
        tracing::debug!(target: "codemark::agent", tool = name, "channel tool call");
        match name {
            "codemark_reply" => self.reply(args),
            "codemark_bind_thread" => self.bind(args),
            "codemark_inbox" => self.inbox(),
            "codemark_comment" => self.comment(args),
            _ => Err(format!("unknown tool: {name}")),
        }
    }
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

    let channel = Channel::new(db_path, session.clone(), codemark_exe)?;
    let served = mcp::serve(std::io::stdin().lock(), &out, &channel);
    stop.store(true, Ordering::Relaxed);
    let _ = watcher.join();
    channel.db.end_agent_session(&session.id)?;
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

    #[test]
    fn instructions_name_the_monitor_command() {
        let (db, session, _) = seeded();
        let ch = Channel {
            db,
            db_path: PathBuf::from("/repo it/.codemark/codemark.db"),
            session: session.clone(),
            codemark_exe: Some(PathBuf::from("/bin/codemark")),
        };
        let text = ch.instructions().unwrap();
        let cmd = format!(
            "'/bin/codemark' agent watch --session {} --oneline --db '/repo it/.codemark/codemark.db'",
            session.id
        );
        assert!(text.contains(&cmd), "{text}");
        assert!(text.contains("Monitor"), "{text}");
    }

    #[test]
    fn declares_channel_capability() {
        let (db, session, _) = seeded();
        let ch = Channel { db, db_path: PathBuf::from(":memory:"), session, codemark_exe: None };
        assert!(ch.capabilities()["experimental"]["claude/channel"].is_object());
        assert!(ch.tools().iter().all(|t| t["name"] != "codemark_comment"));
    }
}
