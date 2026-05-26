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

fn default_true() -> bool {
    true
}
fn default_poll_interval() -> u64 {
    5
}

fn default_zellij_bar() -> String {
    "auto".to_string()
}

fn default_theme() -> String {
    "catppuccin".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScrapePatterns {
    #[serde(default)]
    pub codex: Vec<String>,
    #[serde(default)]
    pub copilot: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoReview {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default)]
    pub patterns: ScrapePatterns,
}

impl Default for AutoReview {
    fn default() -> Self {
        AutoReview {
            enabled: true,
            poll_interval_secs: 5,
            patterns: ScrapePatterns::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub default_agent: String,
    pub worktree_base: String,
    pub base_branch: String,
    /// Bar style for spawned zellij sessions: `auto` (match the user's zellij
    /// `default_layout`), `compact`, `default`, or `none`. Defaults to `auto`,
    /// and tolerates older configs that omit the key.
    #[serde(default = "default_zellij_bar")]
    pub zellij_bar: String,
    /// Active colorscheme name. One of the built-in theme keys (see
    /// `crate::theme::Theme::ALL`), e.g. "catppuccin" or "default". Tolerates
    /// older configs that omit the key.
    #[serde(default = "default_theme")]
    pub theme: String,
    pub agents: Agents,
    #[serde(default)]
    pub auto_review: AutoReview,
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
            zellij_bar: default_zellij_bar(),
            theme: default_theme(),
            agents: Agents {
                claude: cmd("claude"),
                codex: cmd("codex"),
                // copilot rejects a bare positional prompt ("Invalid command
                // format"); it needs `-i` to start an interactive session,
                // optionally seeded with a prompt.
                copilot: AgentCommands {
                    with_prompt: vec![
                        "copilot".to_string(),
                        "-i".to_string(),
                        "{prompt}".to_string(),
                    ],
                    no_prompt: vec!["copilot".to_string(), "-i".to_string()],
                },
            },
            auto_review: AutoReview::default(),
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

    /// Scrape idle-substrings for `agent`. Claude uses launch-injected hooks
    /// instead of scraping, so it has none.
    pub fn auto_review_patterns(&self, agent: Agent) -> &[String] {
        match agent {
            Agent::Codex => &self.auto_review.patterns.codex,
            Agent::Copilot => &self.auto_review.patterns.copilot,
            Agent::Claude => &[],
        }
    }

    /// Detection cadence; clamped to at least 1s so it can never busy-loop.
    pub fn poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.auto_review.poll_interval_secs.max(1))
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
        assert_eq!(c.zellij_bar, "auto");
        assert_eq!(
            c.commands_for(Agent::Codex).with_prompt,
            vec!["codex", "{prompt}"]
        );
    }

    /// copilot needs `-i` for an interactive session; a bare positional prompt
    /// is rejected with "Invalid command format". See GitHub issue #44.
    #[test]
    fn copilot_default_uses_interactive_flag() {
        let c = Config::default();
        let copilot = c.commands_for(Agent::Copilot);
        assert_eq!(copilot.with_prompt, vec!["copilot", "-i", "{prompt}"]);
        assert_eq!(copilot.no_prompt, vec!["copilot", "-i"]);
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
        assert_eq!(loaded.zellij_bar, c.zellij_bar);
    }

    /// A config.toml written before `zellij_bar` existed must still load,
    /// defaulting the missing field to "auto" rather than erroring.
    #[test]
    fn missing_zellij_bar_defaults_to_auto() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut text = toml::to_string_pretty(&Config::default()).unwrap();
        text = text
            .lines()
            .filter(|l| !l.trim_start().starts_with("zellij_bar"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("zellij_bar"));
        fs::write(&path, text).unwrap();
        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded.zellij_bar, "auto");
    }

    #[test]
    fn auto_review_defaults_on() {
        let c = Config::default();
        assert!(c.auto_review.enabled);
        assert_eq!(c.auto_review.poll_interval_secs, 5);
        assert!(c.auto_review.patterns.codex.is_empty());
        assert!(c.auto_review.patterns.copilot.is_empty());
        assert_eq!(c.poll_interval(), std::time::Duration::from_secs(5));
    }

    #[test]
    fn patterns_lookup_by_agent() {
        let mut c = Config::default();
        c.auto_review.patterns.codex = vec!["▌".into()];
        assert_eq!(c.auto_review_patterns(Agent::Codex), &["▌".to_string()]);
        assert!(c.auto_review_patterns(Agent::Claude).is_empty());
        assert!(c.auto_review_patterns(Agent::Copilot).is_empty());
    }

    #[test]
    fn missing_theme_defaults_to_catppuccin() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // Write a config that predates the theme key by stripping it out.
        let text = toml::to_string_pretty(&Config::default())
            .unwrap()
            .lines()
            .filter(|l| !l.trim_start().starts_with("theme"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("theme"));
        fs::write(&path, text).unwrap();
        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded.theme, "catppuccin");
    }

    #[test]
    fn default_config_theme_is_catppuccin() {
        assert_eq!(Config::default().theme, "catppuccin");
    }

    #[test]
    fn config_without_auto_review_section_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "default_agent = \"claude\"\nworktree_base = \"{root}/../wt\"\nbase_branch = \"auto\"\n\
             [agents.claude]\nwith_prompt = [\"claude\", \"{prompt}\"]\nno_prompt = [\"claude\"]\n\
             [agents.codex]\nwith_prompt = [\"codex\", \"{prompt}\"]\nno_prompt = [\"codex\"]\n\
             [agents.copilot]\nwith_prompt = [\"copilot\", \"{prompt}\"]\nno_prompt = [\"copilot\"]\n",
        )
        .unwrap();
        let loaded = load_from(&path).unwrap();
        assert!(loaded.auto_review.enabled);
        assert_eq!(loaded.auto_review.poll_interval_secs, 5);
    }
}
