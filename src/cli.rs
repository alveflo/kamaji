use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::config::Config;
use crate::db::Db;
use crate::models::{Agent, Project};

const USAGE: &str = "\
Usage:
  kamaji
  kamaji ticket create --prompt <prompt> [--title <title>] [--description <text>] [--agent <agent>] [--project <id-or-name>]
  kamaji ticket create <prompt> [--title <title>] [--description <text>] [--agent <agent>] [--project <id-or-name>]

Agents: claude, codex, copilot
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Tui,
    Help,
    CreateTicket(CreateTicketArgs),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTicketArgs {
    pub project: Option<String>,
    pub title: Option<String>,
    pub description: String,
    pub prompt: Option<String>,
    pub agent: Option<Agent>,
}

impl CreateTicketArgs {
    fn title_or_prompt(&self) -> Result<String> {
        if let Some(title) = self
            .title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Ok(title.to_string());
        }
        let prompt = self
            .prompt
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                anyhow!("ticket create requires --prompt, a prompt argument, or --title")
            })?;
        Ok(prompt.lines().next().unwrap_or(prompt).trim().to_string())
    }
}

pub fn usage() -> &'static str {
    USAGE
}

pub fn parse<I, S>(args: I) -> Result<Command>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    if args.is_empty() {
        return Ok(Command::Tui);
    }
    if args == ["--help"] || args == ["-h"] || args == ["help"] {
        return Ok(Command::Help);
    }
    match args.as_slice() {
        [scope, action, rest @ ..] if scope == "ticket" || scope == "tickets" => {
            match action.as_str() {
                "create" | "new" => parse_ticket_create(rest),
                _ => bail!("unknown ticket command: {action}\n\n{USAGE}"),
            }
        }
        [other, ..] => bail!("unknown command: {other}\n\n{USAGE}"),
        [] => Ok(Command::Tui),
    }
}

fn parse_ticket_create(args: &[String]) -> Result<Command> {
    let mut project = None;
    let mut title = None;
    let mut description = String::new();
    let mut prompt = None;
    let mut agent = None;
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--project" | "-p" => {
                project = Some(take_value(args, &mut i, "--project")?);
            }
            "--title" | "-t" => {
                title = Some(take_value(args, &mut i, "--title")?);
            }
            "--description" | "--desc" | "-d" => {
                description = take_value(args, &mut i, "--description")?;
            }
            "--prompt" => {
                prompt = Some(take_value(args, &mut i, "--prompt")?);
            }
            "--agent" | "-a" => {
                let value = take_value(args, &mut i, "--agent")?;
                agent = Some(Agent::from_str(&value).map_err(|e| anyhow!(e))?);
            }
            "--" => {
                positional.extend(args[i + 1..].iter().cloned());
                break;
            }
            flag if flag.starts_with('-') => bail!("unknown option: {flag}\n\n{USAGE}"),
            value => positional.push(value.to_string()),
        }
        i += 1;
    }
    if prompt.is_none() && !positional.is_empty() {
        prompt = Some(positional.join(" "));
    } else if !positional.is_empty() {
        bail!("unexpected positional argument: {}", positional.join(" "));
    }

    let parsed = CreateTicketArgs {
        project,
        title,
        description,
        prompt,
        agent,
    };
    parsed.title_or_prompt()?;
    Ok(Command::CreateTicket(parsed))
}

fn take_value(args: &[String], i: &mut usize, name: &str) -> Result<String> {
    *i += 1;
    args.get(*i)
        .filter(|value| !value.is_empty())
        .cloned()
        .ok_or_else(|| anyhow!("{name} requires a value"))
}

pub fn run_create_ticket(
    db: &Db,
    config: &Config,
    args: &CreateTicketArgs,
    cwd: &Path,
) -> Result<String> {
    let project = match args.project.as_deref() {
        Some(selector) => select_project(db, selector)?,
        None => infer_project(db, cwd)?,
    };
    let agent = args
        .agent
        .or(project.default_agent)
        .unwrap_or_else(|| config.default_agent());
    let title = args.title_or_prompt()?;
    let prompt = args
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let ticket = db.create_ticket(project.id, &title, args.description.trim(), prompt, agent)?;
    Ok(format!(
        "Created ticket #{} in project {}: {}",
        ticket.id, project.name, ticket.title
    ))
}

fn select_project(db: &Db, selector: &str) -> Result<Project> {
    if let Ok(id) = selector.parse::<i64>() {
        return db
            .get_project(id)?
            .ok_or_else(|| anyhow!("no project with id {id}"));
    }
    let matches: Vec<Project> = db
        .list_projects()?
        .into_iter()
        .filter(|p| p.name == selector)
        .collect();
    match matches.as_slice() {
        [project] => Ok(project.clone()),
        [] => bail!("no project named {selector:?}"),
        _ => bail!("more than one project named {selector:?}; use --project <id>"),
    }
}

fn infer_project(db: &Db, cwd: &Path) -> Result<Project> {
    let cwd = normalize_path(cwd);
    let projects = db.list_projects()?;
    if projects.is_empty() {
        bail!("no kamaji projects exist; create one in the TUI first");
    }

    let mut matches = Vec::new();
    for project in &projects {
        let root = normalize_path(&project.root_dir);
        if cwd.starts_with(&root) {
            matches.push((root.components().count(), project.clone()));
        }
        for ticket in db.list_tickets(project.id)? {
            if let Some(worktree) = ticket.worktree_path {
                let worktree = normalize_path(&worktree);
                if cwd.starts_with(&worktree) {
                    matches.push((worktree.components().count(), project.clone()));
                }
            }
        }
    }

    matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    if let Some((score, project)) = matches.first() {
        let ambiguous = matches
            .iter()
            .any(|(other_score, other)| other_score == score && other.id != project.id);
        if ambiguous {
            bail!("current directory matches multiple projects; pass --project <id-or-name>");
        }
        return Ok(project.clone());
    }

    if projects.len() == 1 {
        return Ok(projects[0].clone());
    }

    let names = projects
        .iter()
        .map(|p| format!("{} ({})", p.id, p.name))
        .collect::<Vec<_>>()
        .join(", ");
    bail!(
        "could not infer project from {}; pass --project <id-or-name>. Projects: {names}",
        cwd.display()
    )
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with_project(root: &Path) -> Db {
        let db = Db::open_in_memory().unwrap();
        db.create_project("kamaji", root, Some(Agent::Codex))
            .unwrap();
        db
    }

    #[test]
    fn no_args_runs_tui() {
        assert_eq!(parse(Vec::<String>::new()).unwrap(), Command::Tui);
    }

    #[test]
    fn parses_ticket_create_with_prompt() {
        let parsed = parse([
            "ticket",
            "create",
            "--prompt",
            "Start working on GitHub issue #12",
        ])
        .unwrap();
        assert_eq!(
            parsed,
            Command::CreateTicket(CreateTicketArgs {
                project: None,
                title: None,
                description: String::new(),
                prompt: Some("Start working on GitHub issue #12".into()),
                agent: None,
            })
        );
    }

    #[test]
    fn parses_positional_prompt_and_options() {
        let parsed = parse([
            "tickets",
            "new",
            "--project",
            "kamaji",
            "--agent",
            "claude",
            "--title",
            "Issue 12",
            "Start working",
            "on issue 12",
        ])
        .unwrap();
        assert_eq!(
            parsed,
            Command::CreateTicket(CreateTicketArgs {
                project: Some("kamaji".into()),
                title: Some("Issue 12".into()),
                description: String::new(),
                prompt: Some("Start working on issue 12".into()),
                agent: Some(Agent::Claude),
            })
        );
    }

    #[test]
    fn create_ticket_infers_project_from_registered_root() {
        let dir = tempfile::tempdir().unwrap();
        let db = db_with_project(dir.path());
        let args = CreateTicketArgs {
            project: None,
            title: None,
            description: String::new(),
            prompt: Some("Start working on issue 12".into()),
            agent: None,
        };

        let out = run_create_ticket(&db, &Config::default(), &args, dir.path()).unwrap();
        assert!(out.contains("Created ticket #1"));
        let project = db.list_projects().unwrap().remove(0);
        let tickets = db.list_tickets(project.id).unwrap();
        assert_eq!(tickets[0].title, "Start working on issue 12");
        assert_eq!(
            tickets[0].initial_prompt.as_deref(),
            Some("Start working on issue 12")
        );
        assert_eq!(tickets[0].agent, Agent::Codex);
    }

    #[test]
    fn create_ticket_infers_project_from_recorded_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let worktree = dir.path().join("worktrees").join("kamaji-1-existing");
        std::fs::create_dir_all(&worktree).unwrap();

        let db = db_with_project(&dir.path().join("root"));
        let project = db.list_projects().unwrap().remove(0);
        let existing = db
            .create_ticket(project.id, "Existing", "", None, Agent::Claude)
            .unwrap();
        db.set_ticket_session(
            existing.id,
            "kamaji-1-existing",
            &worktree.to_string_lossy(),
            "kamaji-1-existing",
        )
        .unwrap();

        let args = CreateTicketArgs {
            project: None,
            title: Some("Next".into()),
            description: String::new(),
            prompt: Some("Do the next thing".into()),
            agent: Some(Agent::Claude),
        };

        run_create_ticket(&db, &Config::default(), &args, &worktree).unwrap();
        let tickets = db.list_tickets(project.id).unwrap();
        assert_eq!(tickets.len(), 2);
        assert!(tickets.iter().any(|t| t.title == "Next"));
    }
}
