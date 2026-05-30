use std::path::PathBuf;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Copilot,
}

impl Agent {
    pub fn as_str(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Copilot => "copilot",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Copilot => "Copilot",
        }
    }
    pub fn all() -> [Agent; 3] {
        [Agent::Claude, Agent::Codex, Agent::Copilot]
    }
    /// Position of `self` within `Agent::all()`; the inverse of indexing it.
    pub fn index(self) -> usize {
        Agent::all().iter().position(|a| *a == self).unwrap_or(0)
    }
}

impl FromStr for Agent {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "claude" => Ok(Agent::Claude),
            "codex" => Ok(Agent::Codex),
            "copilot" => Ok(Agent::Copilot),
            other => Err(format!("unknown agent: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Todo,
    InProgress,
    Review,
    Done,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Todo => "todo",
            Status::InProgress => "in_progress",
            Status::Review => "review",
            Status::Done => "done",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Status::Todo => "Todo",
            Status::InProgress => "In Progress",
            Status::Review => "Needs attention",
            Status::Done => "Done",
        }
    }
    pub fn all() -> [Status; 4] {
        [
            Status::Todo,
            Status::InProgress,
            Status::Review,
            Status::Done,
        ]
    }
}

impl FromStr for Status {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "todo" => Ok(Status::Todo),
            "in_progress" => Ok(Status::InProgress),
            "review" => Ok(Status::Review),
            "done" => Ok(Status::Done),
            other => Err(format!("unknown status: {other}")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub root_dir: PathBuf,
    pub default_agent: Option<Agent>,
    #[allow(dead_code)]
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct Ticket {
    pub id: i64,
    #[allow(dead_code)]
    pub project_id: i64,
    pub title: String,
    pub description: String,
    pub initial_prompt: Option<String>,
    pub agent: Agent,
    pub status: Status,
    #[allow(dead_code)]
    pub position: i64,
    pub session_name: Option<String>,
    pub worktree_path: Option<PathBuf>,
    pub branch: Option<String>,
    /// kamaji auto-moved this ticket to "Needs attention" (Review) because its
    /// agent went idle. Persisted so the move back to In Progress survives a
    /// restart; cleared on a manual move or when the session is torn down.
    pub auto_reviewed: bool,
    /// The session was started with the idle-detection hooks (Claude
    /// `--settings`). Without it, an absent marker does not imply "active", so
    /// the agent's activity (and the green bullet) must not be trusted.
    pub instrumented: bool,
    #[allow(dead_code)]
    pub created_at: String,
    #[allow(dead_code)]
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_roundtrips_through_str() {
        for a in Agent::all() {
            assert_eq!(Agent::from_str(a.as_str()).unwrap(), a);
        }
        assert!(Agent::from_str("nope").is_err());
    }

    #[test]
    fn agent_index_inverts_all() {
        for (i, a) in Agent::all().into_iter().enumerate() {
            assert_eq!(a.index(), i);
            assert_eq!(Agent::all()[a.index()], a);
        }
    }

    #[test]
    fn status_roundtrips_and_has_titles() {
        assert_eq!(Status::from_str("in_progress").unwrap(), Status::InProgress);
        assert_eq!(Status::InProgress.title(), "In Progress");
        // The Review column is displayed as "Needs attention" but its DB key
        // stays "review" so persisted tickets keep round-tripping.
        assert_eq!(Status::Review.title(), "Needs attention");
        assert_eq!(Status::Review.as_str(), "review");
        assert_eq!(Status::from_str("review").unwrap(), Status::Review);
        assert!(Status::from_str("bogus").is_err());
    }
}
