use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::models::Agent;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCommands {
    pub with_prompt: Vec<String>,
    pub no_prompt: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agents {
    pub claude: AgentCommands,
    pub codex: AgentCommands,
    pub copilot: AgentCommands,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub default_agent: String,
    pub worktree_base: String,
    pub base_branch: String,
    pub agents: Agents,
}

impl Default for Config {
    fn default() -> Self {
        let cmd = |bin: &str| AgentCommands {
            with_prompt: vec![bin.to_string(), "{prompt}".to_string()],
            no_prompt: vec![bin.to_string()],
        };
        Config {
            default_agent: "claude".to_string(),
            worktree_base: "{root}/../kamaji-worktrees".to_string(),
            base_branch: "auto".to_string(),
            agents: Agents {
                claude: cmd("claude"),
                codex: cmd("codex"),
                copilot: cmd("copilot"),
            },
        }
    }
}

impl Config {
    pub fn commands_for(&self, agent: Agent) -> &AgentCommands {
        match agent {
            Agent::Claude => &self.agents.claude,
            Agent::Codex => &self.agents.codex,
            Agent::Copilot => &self.agents.copilot,
        }
    }

    pub fn default_agent(&self) -> Agent {
        self.default_agent.parse().unwrap_or(Agent::Claude)
    }

    /// Absolute worktree directory for `name`, with `{root}` expanded.
    pub fn worktree_dir(&self, root: &Path, name: &str) -> PathBuf {
        let base = self
            .worktree_base
            .replace("{root}", &root.to_string_lossy());
        PathBuf::from(base).join(name)
    }
}

pub fn config_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "kamaji").context("cannot determine config dir")?;
    Ok(dirs.config_dir().join("config.toml"))
}

pub fn load_from(path: &Path) -> Result<Config> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(toml::from_str(&text)?)
}

pub fn save_to(path: &Path, cfg: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, toml::to_string_pretty(cfg)?)?;
    Ok(())
}

pub fn load_or_init() -> Result<Config> {
    let path = config_path()?;
    if path.exists() {
        load_from(&path)
    } else {
        let cfg = Config::default();
        save_to(&path, &cfg)?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert_eq!(c.default_agent(), Agent::Claude);
        assert_eq!(
            c.commands_for(Agent::Codex).with_prompt,
            vec!["codex", "{prompt}"]
        );
    }

    #[test]
    fn worktree_dir_expands_root() {
        let c = Config::default();
        let p = c.worktree_dir(&PathBuf::from("/home/u/proj"), "kamaji-1-x");
        assert_eq!(
            p,
            PathBuf::from("/home/u/proj/../kamaji-worktrees/kamaji-1-x")
        );
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let c = Config::default();
        save_to(&path, &c).unwrap();
        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded.default_agent, c.default_agent);
        assert_eq!(loaded.worktree_base, c.worktree_base);
    }
}
