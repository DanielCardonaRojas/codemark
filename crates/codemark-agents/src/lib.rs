//! Live agent bridge adapters.
//!
//! The bus itself (sessions, thread bindings, deliveries) lives in
//! `codemark-core` (`engine::agent`, `storage::agent_repo`) so the TUI can use
//! it without linking this crate. This crate holds what talks to agents:
//!
//! - [`runtime`]: what every adapter shares: register a session, claim
//!   queued deliveries, build the event an agent sees, write its answer.
//! - [`mcp`]: a minimal stdio MCP (JSON-RPC) server.
//! - [`claude`]: the Claude Code channel, an MCP server that declares
//!   `claude/channel` and pushes replies with `notifications/claude/channel`.
//!
//! Adapters in other languages (the pi extension) wrap the CLI's
//! `codemark agent watch`, which streams [`runtime::DeliveryEvent`]s.

pub mod claude;
pub mod mcp;
pub mod runtime;
