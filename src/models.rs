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
            Status::Review => "Review",
            Status::Done => "Done",
        }
    }
    pub fn all() -> [Status; 4] {
        [Status::Todo, Status::InProgress, Status::Review, Status::Done]
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
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct Ticket {
    pub id: i64,
    pub project_id: i64,
    pub title: String,
    pub description: String,
    pub initial_prompt: Option<String>,
    pub agent: Agent,
    pub status: Status,
    pub position: i64,
    pub session_name: Option<String>,
    pub worktree_path: Option<PathBuf>,
    pub branch: Option<String>,
    pub created_at: String,
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
    fn status_roundtrips_and_has_titles() {
        assert_eq!(Status::from_str("in_progress").unwrap(), Status::InProgress);
        assert_eq!(Status::InProgress.title(), "In Progress");
        assert!(Status::from_str("bogus").is_err());
    }
}
