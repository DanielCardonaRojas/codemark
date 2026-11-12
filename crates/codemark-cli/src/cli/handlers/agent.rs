//! `codemark agent`: live agent sessions that answer comment replies.
//!
//! The bus is the repo database (see `codemark_core::engine::agent`). These
//! commands are the shared core adapters wrap: `watch` streams replies as
//! JSON lines, `inbox` hands them over once, `channel claude` is the Claude
//! Code adapter itself. Experimental while Claude channels are a preview.

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::Serialize;

use crate::cli::output::{OutputMode, write_json_success};
use crate::cli::*;
use codemark_agents::{claude, runtime};
use codemark_core::engine::agent::AgentSession;
use codemark_core::error::{Error, Result};
use codemark_core::storage::Workspace;

use super::open_db;

pub async fn handle_agent(cli: &Cli, mode: &OutputMode, args: &AgentArgs) -> Result<()> {
    match &args.command {
        AgentCommand::Sessions(a) => sessions(cli, mode, a),
        AgentCommand::Register(a) => register(cli, a),
        AgentCommand::Watch(a) => watch(cli, a),
        AgentCommand::Inbox(a) => inbox(cli, a),
        AgentCommand::Channel(a) => channel(cli, a),
        AgentCommand::Setup(a) => setup(cli, a),
    }
}

#[derive(Serialize)]
struct SessionView<'a> {
    #[serde(flatten)]
    session: &'a AgentSession,
    live: bool,
}

fn sessions(cli: &Cli, mode: &OutputMode, args: &AgentSessionsArgs) -> Result<()> {
    let db = open_db(cli)?;
    let sessions = db.list_agent_sessions(args.all)?;
    let views: Vec<SessionView> =
        sessions.iter().map(|s| SessionView { session: s, live: s.is_live() }).collect();
    match mode {
        OutputMode::Json => write_json_success(&views)?,
        _ => {
            for v in &views {
                println!(
                    "{}  {:<12} {:<10} {}",
                    v.session.id,
                    v.session.kind,
                    v.session.display_name(),
                    if v.live { "live" } else { "offline" }
                );
            }
        }
    }
    Ok(())
}

fn register(cli: &Cli, args: &AgentRegisterArgs) -> Result<()> {
    let db = open_db(cli)?;
    let session = runtime::register_session(&db, &args.kind, args.label.clone(), None)?;
    println!("{}", session.id);
    Ok(())
}

/// Stream replies for a session as JSON lines until stdout closes. With no
/// `--session`, registers one and prints `{"type":"session",...}` first.
fn watch(cli: &Cli, args: &AgentWatchArgs) -> Result<()> {
    let db = open_db(cli)?;
    // Attaching to an existing session (e.g. Claude Code's Monitor watching
    // its channel session) makes this a watcher: the session's own adapter
    // yields to it, and the session outlives it. Without --session this
    // process registers, owns and ends the session.
    let (session, role) = match &args.session {
        Some(id) => (
            db.get_agent_session(id)?
                .ok_or_else(|| Error::Input(format!("no agent session {id}")))?,
            runtime::WatchRole::Watcher,
        ),
        None => (
            runtime::register_session(&db, &args.kind, args.label.clone(), None)?,
            runtime::WatchRole::Owner,
        ),
    };
    let path = db.path().to_path_buf();

    let mut out = std::io::stdout().lock();
    let emit = |out: &mut std::io::StdoutLock, e: &runtime::DeliveryEvent| -> Result<()> {
        let line = if args.oneline {
            runtime::format_oneline(e)
        } else {
            serde_json::to_string(e).map_err(|e| Error::Operation(e.to_string()))?
        };
        // A closed stdout means the consumer went away: stop watching.
        writeln!(out, "{line}").and_then(|_| out.flush())?;
        Ok(())
    };

    // One-line mode feeds notifications (every stdout line wakes the agent),
    // so the session greeting goes to stderr there.
    let hello = serde_json::json!({"type": "session", "session": &session});
    if args.oneline {
        eprintln!("{hello}");
    } else {
        writeln!(out, "{hello}")?;
        out.flush()?;
    }

    // A watcher (re)starting re-sends replies still waiting on the session,
    // including ones pushed while no watcher ran, which a channel may have
    // dropped.
    if role == runtime::WatchRole::Watcher {
        db.set_agent_session_watcher(&session.id, true)?;
        for e in runtime::open_events(&db, &session.id)? {
            emit(&mut out, &e)?;
        }
    }
    drop(db);

    let stop = Arc::new(AtomicBool::new(false));
    let res = runtime::watch_loop(&path, &session.id, role, stop, |events| {
        for e in &events {
            emit(&mut out, e)?;
        }
        Ok(())
    });
    if role == runtime::WatchRole::Owner
        && let Ok(db) = codemark_core::storage::db::Database::open(&path)
    {
        let _ = db.end_agent_session(&session.id);
    }
    res
}

fn inbox(cli: &Cli, args: &AgentInboxArgs) -> Result<()> {
    let db = open_db(cli)?;
    db.get_agent_session(&args.session)?
        .ok_or_else(|| Error::Input(format!("no agent session {}", args.session)))?;
    // Pulling counts as being alive.
    db.heartbeat_agent_session(&args.session)?;
    let events = runtime::claim_pending(&db, &args.session)?;
    write_json_success(&events)?;
    Ok(())
}

fn channel(cli: &Cli, args: &AgentChannelArgs) -> Result<()> {
    let path = runtime::current_db_path(cli.db.first().map(|p| p.as_path()))?;
    match args.agent {
        ChannelAgent::Claude => {
            claude::run(&path, args.label.clone(), std::env::current_exe().ok())
        }
    }
}

/// The `.mcp.json` server entry that runs the Claude channel.
fn claude_mcp_entry() -> serde_json::Value {
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "codemark".into());
    serde_json::json!({"command": exe, "args": ["agent", "channel", "claude"]})
}

fn setup(cli: &Cli, args: &AgentSetupArgs) -> Result<()> {
    match args.agent {
        ChannelAgent::Claude => {
            let entry = claude_mcp_entry();
            let launch = "claude --dangerously-load-development-channels server:codemark";
            if args.print {
                let cfg = serde_json::json!({"mcpServers": {"codemark": entry}});
                println!("{}", serde_json::to_string_pretty(&cfg).unwrap_or_default());
                eprintln!("\nThen start Claude Code with:\n  {launch}");
                return Ok(());
            }
            // Claude Code reads .mcp.json from the directory it starts in: the
            // worktree you're in, which may not hold the (shared) database.
            let cwd = std::env::current_dir()?;
            let root = match codemark_core::git::context::worktree_root(&cwd) {
                Some(root) => root,
                None => Workspace::project_root(&open_db(cli)?),
            };
            let path = root.join(".mcp.json");
            let mut cfg: serde_json::Value = match std::fs::read_to_string(&path) {
                Ok(text) => serde_json::from_str(&text).map_err(|e| {
                    Error::Input(format!("{} is not valid JSON: {e}", path.display()))
                })?,
                Err(_) => serde_json::json!({}),
            };
            if !cfg.is_object() {
                return Err(Error::Input(format!("{} is not a JSON object", path.display())));
            }
            if !cfg["mcpServers"].is_object() {
                cfg["mcpServers"] = serde_json::json!({});
            }
            cfg["mcpServers"]["codemark"] = entry;
            let text = serde_json::to_string_pretty(&cfg).unwrap_or_default();
            std::fs::write(&path, text + "\n")?;
            println!("Wrote the codemark channel to {}", path.display());
            println!(
                "Start Claude Code with:\n  {launch}\n\
                 Channels are a research preview; the flag is needed until codemark is allowlisted."
            );
            Ok(())
        }
    }
}
