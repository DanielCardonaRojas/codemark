//! The codemark MCP server an agent spawns: its tools, and how the agent
//! learns about replies.
//!
//! Every agent gets the same tools (`codemark_comment`, `codemark_reply`,
//! `codemark_bind_thread`, `codemark_inbox`). What differs is delivery:
//!
//! - [`Delivery::ClaudeChannel`]: Claude Code. The server declares
//!   `claude/channel` and pushes replies (see [`crate::claude`]), and the
//!   instructions have Claude run `codemark agent watch` under its Monitor
//!   tool, since channels can be inactive and drop pushes silently.
//! - [`Delivery::Pull`]: any other MCP client (Codex, Gemini CLI, Cursor…).
//!   Nothing is pushed: the agent calls `codemark_inbox`, and the TUI says
//!   "waiting for its next turn" until it does.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

use codemark_core::engine::agent::AgentSession;
use codemark_core::error::{Error, Result};
use codemark_core::storage::db::Database;

use crate::mcp::{self, Handler, Outbound, arg_str};
use crate::runtime;

/// How replies reach the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Pushed over Claude Code's channel, or streamed by a Monitor watcher.
    ClaudeChannel,
    /// Fetched by the agent with `codemark_inbox`.
    Pull,
}

/// The shell command that streams `session_id`'s replies one per line, for
/// tools such as Claude Code's Monitor. Paths are single-quoted for the shell.
pub fn watch_command(exe: Option<&Path>, db_path: &Path, session_id: &str) -> String {
    let quote = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', r"'\''"));
    let exe = exe.map(quote).unwrap_or_else(|| "codemark".into());
    format!("{exe} agent watch --session {session_id} --oneline --db {}", quote(db_path))
}

const ANSWERING: &str = "Each one is a person answering a comment you left on bookmarked \
code, or a comment that @mentions you by name. Read the code at that location if you need to, then answer with the codemark_reply \
tool, passing the thread_id. Keep answers short, like a code-review reply. If you change \
code because of a reply, say so in your answer.";

fn starting_threads(session_id: &str) -> String {
    format!(
        "To start a thread the user can answer, use codemark_comment (bookmark_id, body, \
optional range). Threads you create with the codemark CLI are yours too when you pass \
--session {session_id} to `codemark comment add`; codemark_bind_thread takes over an \
existing thread (say, one the user started for you)."
    )
}

fn instructions(delivery: Delivery, session: &AgentSession, watch: &str) -> String {
    let receiving = match delivery {
        Delivery::ClaudeChannel => format!(
            "Receiving replies: as soon as the session starts, before anything else, start a \
background Monitor (timeout_ms 1800000, description \"codemark replies\") running \
exactly:\n  {watch}\n\
Each line it prints is one comment for you (`codemark reply thread_id=... | author: text`, or `codemark mention ...` when the user @mentioned you). \
When the monitor expires, start it again with the same command: replies that arrived \
meanwhile are delivered on restart. Do this even if replies also arrive as \
<channel source=\"codemark\"> events: while the monitor runs, the channel stays quiet, \
so you never get a reply twice. If you cannot start a Monitor, call codemark_inbox \
whenever the user mentions a reply."
        ),
        Delivery::Pull => format!(
            "Receiving replies: replies are not pushed to you. Call codemark_inbox when you \
start working in this repository, and again whenever the user mentions a codemark comment, \
thread or reply; until you do, the user sees \"waiting for {name}'s next turn\". If you can \
run a long-lived background command whose output lines are shown to you as they arrive, you \
may run this instead, and restart it if it stops; each line is one reply:\n  {watch}",
            name = session.display_name()
        ),
    };
    format!(
        "Codemark delivers replies to code comments you wrote in this session. {ANSWERING}\n\n\
{receiving}\n\n{}",
        starting_threads(&session.id)
    )
}

/// The MCP handler: the codemark tools for one agent session.
pub struct AgentServer {
    pub(crate) db: Database,
    db_path: PathBuf,
    session: AgentSession,
    /// The codemark binary, for `codemark_comment` (anchoring lives in the
    /// CLI). `None` disables the tool.
    codemark_exe: Option<PathBuf>,
    delivery: Delivery,
}

impl AgentServer {
    pub fn new(
        db_path: &Path,
        session: AgentSession,
        codemark_exe: Option<PathBuf>,
        delivery: Delivery,
    ) -> Result<Self> {
        Ok(Self {
            db: Database::open(db_path)?,
            db_path: db_path.to_path_buf(),
            session,
            codemark_exe,
            delivery,
        })
    }

    pub fn session(&self) -> &AgentSession {
        &self.session
    }

    fn reply(&self, args: &Value) -> std::result::Result<String, String> {
        let thread = arg_str(args, "thread_id")?;
        let body = arg_str(args, "body")?;
        let c =
            runtime::answer(&self.db, &self.session, thread, body).map_err(|e| e.to_string())?;
        Ok(format!("Replied in thread {} (comment {}).", c.parent_id.unwrap_or_default(), c.id))
    }

    fn bind(&self, args: &Value) -> std::result::Result<String, String> {
        let id = arg_str(args, "comment_id")?;
        let root = runtime::thread_root(&self.db, id).map_err(|e| e.to_string())?;
        self.db.bind_agent_thread(&root.id, &self.session.id).map_err(|e| e.to_string())?;
        Ok(format!("Thread {} is now yours: replies to it reach this session.", root.id))
    }

    fn inbox(&self) -> std::result::Result<String, String> {
        // Asking for replies is a sign of life for a session nothing pushes to.
        let _ = self.db.heartbeat_agent_session(&self.session.id);
        let events = runtime::open_events(&self.db, &self.session.id).map_err(|e| e.to_string())?;
        if events.is_empty() {
            return Ok("No pending replies.".into());
        }
        let parts: Vec<String> = events
            .iter()
            .map(|e| format!("[thread_id={}]\n{}", e.thread_id, runtime::describe_event(e)))
            .collect();
        Ok(parts.join("\n\n---\n\n"))
    }

    fn comment(&self, args: &Value) -> std::result::Result<String, String> {
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

impl Handler for AgentServer {
    fn capabilities(&self) -> Value {
        match self.delivery {
            Delivery::ClaudeChannel => json!({"experimental": {"claude/channel": {}}, "tools": {}}),
            Delivery::Pull => json!({"tools": {}}),
        }
    }

    fn instructions(&self) -> Option<String> {
        let watch = watch_command(self.codemark_exe.as_deref(), &self.db_path, &self.session.id);
        Some(instructions(self.delivery, &self.session, &watch))
    }

    fn server_name(&self) -> &str {
        "codemark"
    }

    fn tools(&self) -> Vec<Value> {
        let mut tools = vec![
            json!({
                "name": "codemark_reply",
                "description": "Answer a codemark comment thread, using the thread_id of a reply you received (from codemark_inbox, a watcher line or a channel event).",
                "inputSchema": {"type": "object", "properties": {
                    "thread_id": {"type": "string", "description": "Thread (root comment) id"},
                    "body": {"type": "string", "description": "Markdown answer"}
                }, "required": ["thread_id", "body"]}
            }),
            json!({
                "name": "codemark_bind_thread",
                "description": "Take over an existing codemark comment thread: replies to it will reach this session.",
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
                "description": "Comment on a bookmark, optionally on one node inside it, as a thread the user can answer.",
                "inputSchema": {"type": "object", "properties": {
                    "bookmark_id": {"type": "string"},
                    "body": {"type": "string", "description": "Markdown comment"},
                    "range": {"type": "string", "description": "Optional line range in the bookmark's file: 42, 42-67 or 10:5-12:20"}
                }, "required": ["bookmark_id", "body"]}
            }));
        }
        tools
    }

    fn call_tool(&self, name: &str, args: &Value) -> std::result::Result<String, String> {
        tracing::debug!(target: "codemark::agent", tool = name, delivery = ?self.delivery, "tool call");
        match name {
            "codemark_reply" => self.reply(args),
            "codemark_bind_thread" => self.bind(args),
            "codemark_inbox" => self.inbox(),
            "codemark_comment" => self.comment(args),
            _ => Err(format!("unknown tool: {name}")),
        }
    }
}

/// Run a pull-only MCP server for an agent of `kind` on stdio until the
/// client closes stdin. The session stays live while the server runs, and
/// ends with it.
pub fn run_pull(
    db_path: &Path,
    kind: &str,
    label: Option<String>,
    codemark_exe: Option<PathBuf>,
) -> Result<()> {
    let db = Database::open(db_path)?;
    let session = runtime::register_session(&db, kind, label, None, false)?;
    drop(db);
    tracing::info!(target: "codemark::agent", session = %session.id, kind, db = %db_path.display(), "mcp server started");

    let stop = Arc::new(AtomicBool::new(false));
    let heartbeat = {
        let (stop, path, sid) = (stop.clone(), db_path.to_path_buf(), session.id.clone());
        std::thread::spawn(move || heartbeat_loop(&path, &sid, &stop))
    };

    let server = AgentServer::new(db_path, session, codemark_exe, Delivery::Pull)?;
    let out = Outbound::new(std::io::stdout());
    let served = mcp::serve(std::io::stdin().lock(), &out, &server);
    stop.store(true, Ordering::Relaxed);
    let _ = heartbeat.join();
    server.db.end_agent_session(&server.session.id)?;
    tracing::info!(target: "codemark::agent", session = %server.session.id, "mcp server stopped");
    served.map_err(|e| Error::Operation(e.to_string()))
}

/// Keep `session_id` live until `stop` is set.
fn heartbeat_loop(db_path: &Path, session_id: &str, stop: &AtomicBool) {
    let db = match Database::open(db_path) {
        Ok(db) => db,
        Err(e) => {
            tracing::error!(target: "codemark::agent", error = %e, "heartbeat could not open the database");
            return;
        }
    };
    let mut last = std::time::Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if last.elapsed() >= runtime::HEARTBEAT_INTERVAL {
            if let Err(e) = db.heartbeat_agent_session(session_id) {
                tracing::warn!(target: "codemark::agent", error = %e, "heartbeat failed");
            }
            last = std::time::Instant::now();
        }
        std::thread::sleep(runtime::POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::tests::seeded;
    use codemark_core::engine::bookmark::BookmarkComment;

    fn server(delivery: Delivery, exe: Option<&str>) -> (AgentServer, AgentSession) {
        let (db, session, _) = seeded();
        let s = AgentServer {
            db,
            db_path: PathBuf::from("/repo it/.codemark/codemark.db"),
            session: session.clone(),
            codemark_exe: exe.map(PathBuf::from),
            delivery,
        };
        (s, session)
    }

    #[test]
    fn claude_instructions_name_the_monitor_command() {
        let (s, session) = server(Delivery::ClaudeChannel, Some("/bin/codemark"));
        let text = s.instructions().unwrap();
        let cmd = format!(
            "'/bin/codemark' agent watch --session {} --oneline --db '/repo it/.codemark/codemark.db'",
            session.id
        );
        assert!(text.contains(&cmd), "{text}");
        assert!(text.contains("Monitor"), "{text}");
    }

    #[test]
    fn only_the_claude_server_declares_the_channel() {
        let (claude, _) = server(Delivery::ClaudeChannel, None);
        assert!(claude.capabilities()["experimental"]["claude/channel"].is_object());
        let (pull, _) = server(Delivery::Pull, None);
        assert!(pull.capabilities().get("experimental").is_none());
        assert!(pull.capabilities()["tools"].is_object());
    }

    #[test]
    fn pull_instructions_point_at_the_inbox_not_a_monitor() {
        let (s, _) = server(Delivery::Pull, Some("/bin/codemark"));
        let text = s.instructions().unwrap();
        assert!(text.contains("codemark_inbox"), "{text}");
        assert!(text.contains("not pushed"), "{text}");
        assert!(!text.contains("Monitor"), "{text}");
    }

    #[test]
    fn comment_tool_needs_the_binary() {
        let (s, _) = server(Delivery::Pull, None);
        assert!(s.tools().iter().all(|t| t["name"] != "codemark_comment"));
        let (s, _) = server(Delivery::Pull, Some("/bin/codemark"));
        assert!(s.tools().iter().any(|t| t["name"] == "codemark_comment"));
    }

    #[test]
    fn inbox_claims_a_reply_and_reply_answers_it() {
        let (s, session) = server(Delivery::Pull, None);
        let root = s.db.list_comments_for_bookmark("bm-1").unwrap().remove(0);
        let reply = BookmarkComment {
            parent_id: Some(root.id.clone()),
            ..BookmarkComment::new(&root.bookmark_id, "daniel", "Why not the lexer?")
        };
        s.db.insert_reply_with_delivery(&reply, &root.id).unwrap();

        let text = s.call_tool("codemark_inbox", &json!({})).unwrap();
        assert!(text.contains(&format!("[thread_id={}]", root.id)), "{text}");
        assert!(text.contains("Why not the lexer?"), "{text}");

        s.call_tool("codemark_reply", &json!({"thread_id": root.id, "body": "It can't see EOF."}))
            .unwrap();
        let state = s.db.agent_thread_states(&[root.id.as_str()]).unwrap();
        let latest = state[&root.id].latest.as_ref().unwrap();
        assert_eq!(latest.status, codemark_core::engine::agent::DeliveryStatus::Answered);
        assert_eq!(state[&root.id].session.id, session.id);
        assert_eq!(s.call_tool("codemark_inbox", &json!({})).unwrap(), "No pending replies.");
    }
}
