//! Live agent bridge: sessions, the threads they own, and deliveries.
//!
//! The database is the bus. An adapter process registers an
//! [`AgentSession`] for a running agent (Claude Code, pi, Codex...). Threads
//! the agent starts are bound to that session. When a user replies to a bound
//! thread, the reply is written together with a [`Delivery`] row; the
//! session's adapter picks it up, pushes it into the agent, and the agent's
//! answer lands back in the thread. The TUI watches the same rows to show a
//! pending indicator and the answer, live.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A session counts as live while its heartbeat is younger than this.
pub const SESSION_LIVE_SECS: i64 = 15;

/// A delivered reply with no answer after this long is shown as expired.
pub const DELIVERY_EXPIRY_SECS: i64 = 5 * 60;

/// Timestamp format used for every bridge row.
pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Seconds between `iso` and now; `None` when `iso` does not parse.
pub fn age_secs(iso: &str) -> Option<i64> {
    let at = DateTime::parse_from_rfc3339(iso).ok()?;
    Some((Utc::now() - at.with_timezone(&Utc)).num_seconds())
}

/// A running (or past) agent session that can take pushed replies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSession {
    pub id: String,
    /// `claude-code`, `codex`, `pi`, `pull`...
    pub kind: String,
    /// Short name shown in the TUI (`claude`); falls back to `kind`.
    pub label: Option<String>,
    /// The agent's own session or thread id, for resuming it later.
    pub external_ref: Option<String>,
    pub pid: Option<i64>,
    pub started_at: String,
    pub heartbeat_at: String,
    pub ended_at: Option<String>,
    /// Heartbeat of a `codemark agent watch` delivering this session's
    /// replies (for example under Claude Code's Monitor tool). While it is
    /// fresh, the channel adapter leaves deliveries to the watcher.
    #[serde(default)]
    pub watcher_heartbeat_at: Option<String>,
    /// Whether replies are pushed to the agent (a channel, an extension).
    /// `false` for agents that fetch them on their next turn, such as a
    /// plain MCP client calling `codemark_inbox`.
    #[serde(default = "default_push")]
    pub push: bool,
}

fn default_push() -> bool {
    true
}

impl AgentSession {
    /// A session for `kind`. Replies are pushed unless the kind is `pull`;
    /// see [`AgentSession::with_push`].
    pub fn new(kind: impl Into<String>, label: Option<String>) -> Self {
        let now = now_rfc3339();
        let kind = kind.into();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            push: kind != "pull",
            kind,
            label,
            external_ref: None,
            pid: Some(std::process::id() as i64),
            started_at: now.clone(),
            heartbeat_at: now,
            ended_at: None,
            watcher_heartbeat_at: None,
        }
    }

    /// This session with replies pushed (`true`) or pulled (`false`).
    pub fn with_push(self, push: bool) -> Self {
        Self { push, ..self }
    }

    /// Name to show for the agent: its label, else its kind.
    pub fn display_name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.kind)
    }

    /// Whether the session is running: not ended, heartbeat recent.
    pub fn is_live(&self) -> bool {
        self.ended_at.is_none()
            && age_secs(&self.heartbeat_at).is_some_and(|age| age <= SESSION_LIVE_SECS)
    }

    /// Whether a `codemark agent watch` is currently delivering this
    /// session's replies.
    pub fn has_live_watcher(&self) -> bool {
        self.watcher_heartbeat_at
            .as_deref()
            .and_then(age_secs)
            .is_some_and(|age| age <= SESSION_LIVE_SECS)
    }

    /// Whether the session's agent only pulls replies on its next turn
    /// (no push, and no `codemark agent watch` delivering them), so the TUI
    /// says "waiting for its next turn" instead of showing a spinner.
    pub fn is_pull_only(&self) -> bool {
        !self.push && !self.has_live_watcher()
    }
}

/// Agent names `@mentioned` in `body`, lowercased, in order, without
/// duplicates. A mention is `@` followed by letters, digits, `_` or `-`, not
/// preceded by a word character (so `me@example.com` is not one), and not
/// inside a backtick code span or fence.
pub fn mentions(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut in_code = false;
    let mut prev: Option<char> = None;
    for (i, c) in body.char_indices() {
        if c == '`' {
            in_code = !in_code;
        } else if c == '@'
            && !in_code
            && !prev.is_some_and(|p| p.is_alphanumeric() || matches!(p, '_' | '.' | '-'))
        {
            let rest = &body[i + 1..];
            let len = rest
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
                .unwrap_or(rest.len());
            let name = rest[..len].trim_end_matches('-').to_ascii_lowercase();
            if !name.is_empty() && !out.contains(&name) {
                out.push(name);
            }
        }
        prev = Some(c);
    }
    out
}

/// Whether `session` answers to `@name`: its label or its kind, ignoring case.
pub fn answers_to(session: &AgentSession, name: &str) -> bool {
    session.label.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(name))
        || session.kind.eq_ignore_ascii_case(name)
}

/// Where a delivery is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryStatus {
    /// Written with the reply; no adapter has picked it up yet.
    Queued,
    /// Pushed into the agent; waiting for its answer.
    Delivered,
    /// The agent answered in the thread.
    Answered,
    Failed,
    Expired,
    Cancelled,
}

impl DeliveryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Delivered => "delivered",
            Self::Answered => "answered",
            Self::Failed => "failed",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => Self::Queued,
            "delivered" => Self::Delivered,
            "answered" => Self::Answered,
            "failed" => Self::Failed,
            "expired" => Self::Expired,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    /// Whether the user is still waiting on the agent.
    pub fn is_open(self) -> bool {
        matches!(self, Self::Queued | Self::Delivered)
    }
}

/// One user reply that must reach an agent session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub id: String,
    /// The user's reply.
    pub comment_id: String,
    /// Root comment of the thread.
    pub thread_id: String,
    pub session_id: String,
    pub status: DeliveryStatus,
    pub created_at: String,
    pub delivered_at: Option<String>,
    pub answered_at: Option<String>,
    pub answer_comment_id: Option<String>,
    pub error: Option<String>,
}

impl Delivery {
    pub fn new(comment_id: &str, thread_id: &str, session_id: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            comment_id: comment_id.to_string(),
            thread_id: thread_id.to_string(),
            session_id: session_id.to_string(),
            status: DeliveryStatus::Queued,
            created_at: now_rfc3339(),
            delivered_at: None,
            answered_at: None,
            answer_comment_id: None,
            error: None,
        }
    }

    /// Whether a delivered reply has gone unanswered past the expiry.
    pub fn is_overdue(&self) -> bool {
        self.status == DeliveryStatus::Delivered
            && self
                .delivered_at
                .as_deref()
                .and_then(age_secs)
                .is_some_and(|age| age > DELIVERY_EXPIRY_SECS)
    }
}

/// The agent side of one thread, as the TUI shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadAgentState {
    /// The session the thread is waiting on: the one its newest delivery
    /// went to (an `@mention` can go to another agent than the owner), else
    /// the owner.
    pub session: AgentSession,
    /// The newest delivery on the thread, if any.
    pub latest: Option<Delivery>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_are_names_after_at_outside_code_and_emails() {
        assert_eq!(mentions("@claude what is this?"), ["claude"]);
        assert_eq!(mentions("ask @Codex, then @claude-code."), ["codex", "claude-code"]);
        assert_eq!(mentions("@claude and again @CLAUDE"), ["claude"]);
        assert!(mentions("mail me@example.com").is_empty());
        assert!(mentions("see `@types/node` and\n```\n@decorator\n```").is_empty());
        assert!(mentions("just an @ sign, and @- too").is_empty());
        assert_eq!(mentions("(@pi)"), ["pi"]);
    }

    #[test]
    fn a_session_answers_to_its_label_or_kind() {
        let s = AgentSession::new("claude-code", Some("claude".into()));
        assert!(answers_to(&s, "claude") && answers_to(&s, "Claude-Code"));
        assert!(!answers_to(&s, "codex"));
        assert!(answers_to(&AgentSession::new("codex", None), "codex"));
    }

    #[test]
    fn a_session_waits_for_its_next_turn_only_without_push_or_watcher() {
        assert!(!AgentSession::new("claude-code", None).is_pull_only());
        // `pull` (`codemark agent register`) defaults to pulling.
        assert!(AgentSession::new("pull", None).is_pull_only());
        let codex = AgentSession::new("codex", None).with_push(false);
        assert!(codex.is_pull_only());
        // A live `codemark agent watch` delivers its replies as they come.
        let watched = AgentSession { watcher_heartbeat_at: Some(now_rfc3339()), ..codex };
        assert!(!watched.is_pull_only());
    }

    #[test]
    fn sessions_serialized_before_push_existed_still_push() {
        let mut v = serde_json::to_value(AgentSession::new("pi", None)).unwrap();
        v.as_object_mut().unwrap().remove("push");
        let s: AgentSession = serde_json::from_value(v).unwrap();
        assert!(s.push);
    }

    #[test]
    fn status_round_trips() {
        for s in [
            DeliveryStatus::Queued,
            DeliveryStatus::Delivered,
            DeliveryStatus::Answered,
            DeliveryStatus::Failed,
            DeliveryStatus::Expired,
            DeliveryStatus::Cancelled,
        ] {
            assert_eq!(DeliveryStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(DeliveryStatus::parse("nope"), None);
    }

    #[test]
    fn session_liveness_follows_heartbeat_and_end() {
        let mut s = AgentSession::new("claude-code", Some("claude".into()));
        assert!(s.is_live());
        s.heartbeat_at = "2020-01-01T00:00:00.000Z".into();
        assert!(!s.is_live());
        s.heartbeat_at = now_rfc3339();
        s.ended_at = Some(now_rfc3339());
        assert!(!s.is_live());
    }

    #[test]
    fn overdue_only_when_delivered_long_ago() {
        let mut d = Delivery::new("c", "t", "s");
        assert!(!d.is_overdue());
        d.status = DeliveryStatus::Delivered;
        d.delivered_at = Some(now_rfc3339());
        assert!(!d.is_overdue());
        d.delivered_at = Some("2020-01-01T00:00:00.000Z".into());
        assert!(d.is_overdue());
    }
}
