use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::config::Config;
use crate::db::Db;
use crate::models::{Agent, Project};
use crate::session;

const USAGE: &str = "\
Usage:
  kamaji
  kamaji ticket create --prompt <prompt> [--title <title>] [--description <text>] [--agent <agent>] [--project <id-or-name>] [--background]
  kamaji ticket create <prompt> [--title <title>] [--description <text>] [--agent <agent>] [--project <id-or-name>] [--background]

Agents: claude, codex, copilot

  --background, -b   also start the ticket's agent in a detached zellij session
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Tui,
    Help,
    Version,
    CreateTicket(CreateTicketArgs),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTicketArgs {
    pub project: Option<String>,
    pub title: Option<String>,
    pub description: String,
    pub prompt: Option<String>,
    pub agent: Option<Agent>,
    /// Opt-in: also start the ticket's detached zellij session.
    pub background: bool,
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

/// A prepared session the caller must launch (detached) after `run_create_ticket`
/// has recorded it. Carries what teardown needs if the launch fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub ticket_id: i64,
    pub name: String,
    pub layout_path: PathBuf,
    pub cwd: PathBuf,
}

/// Result of `run_create_ticket`: the ticket is always created; `launch` is set
/// only when a background session was prepared and recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOutcome {
    /// Summary to print to stdout (always the "Created ticket #N" line).
    pub message: String,
    /// Reason to print to stderr when `--background` could not be honored.
    pub warning: Option<String>,
    /// Some => the caller must launch this detached zellij session.
    pub launch: Option<LaunchSpec>,
    /// True when `--background` was requested but the session could not be
    /// prepared; the ticket stays in Todo and the caller should exit non-zero.
    pub background_failed: bool,
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
    if args == ["--version"] || args == ["-V"] || args == ["version"] {
        return Ok(Command::Version);
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
    let mut background = false;
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
            "--background" | "-b" => {
                background = true;
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
        background,
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
    state_dir: &Path,
) -> Result<CreateOutcome> {
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
    let message = format!(
        "Created ticket #{} in project {}: {}",
        ticket.number, project.name, ticket.title
    );

    if !args.background {
        return Ok(CreateOutcome {
            message,
            warning: None,
            launch: None,
            background_failed: false,
        });
    }

    // --background: prepare + record the session here; the caller performs the
    // detached launch. On any preparation failure the ticket is left in Todo and
    // the caller is told to exit non-zero.
    match session::prepare_session(&project, config, state_dir, &ticket) {
        Ok(p) => {
            session::commit_session(db, ticket.id, &p)?;
            Ok(CreateOutcome {
                message,
                warning: None,
                launch: Some(LaunchSpec {
                    ticket_id: ticket.id,
                    name: p.name,
                    layout_path: p.layout_path,
                    cwd: p.worktree,
                }),
                background_failed: false,
            })
        }
        Err(e) => Ok(CreateOutcome {
            message,
            warning: Some(format!("could not start session: {e}")),
            launch: None,
            background_failed: true,
        }),
    }
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
    fn parses_version_flag() {
        assert_eq!(parse(["--version"]).unwrap(), Command::Version);
        assert_eq!(parse(["-V"]).unwrap(), Command::Version);
        assert_eq!(parse(["version"]).unwrap(), Command::Version);
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
                background: false,
            })
        );
    }

    #[test]
    fn background_flag_sets_background() {
        for flag in ["-b", "--background"] {
            let Command::CreateTicket(args) =
                parse(["ticket", "create", flag, "--prompt", "go"]).unwrap()
            else {
                panic!("expected CreateTicket for {flag}");
            };
            assert!(args.background, "{flag} should set background");
        }
        // Absent: defaults off.
        let Command::CreateTicket(args) = parse(["ticket", "create", "--prompt", "go"]).unwrap()
        else {
            panic!("expected CreateTicket");
        };
        assert!(!args.background, "background defaults off");
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
                background: false,
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
            background: false,
        };

        let out =
            run_create_ticket(&db, &Config::default(), &args, dir.path(), dir.path()).unwrap();
        assert!(out.message.contains("Created ticket #1"));
        assert!(out.launch.is_none(), "no --background => no session");
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
            background: false,
        };

        run_create_ticket(&db, &Config::default(), &args, &worktree, dir.path()).unwrap();
        let tickets = db.list_tickets(project.id).unwrap();
        assert_eq!(tickets.len(), 2);
        assert!(tickets.iter().any(|t| t.title == "Next"));
    }

    /// Initialize a real git repo with one commit at `root`.
    fn init_repo(root: &Path) {
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.email", "t@t.t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("f"), "x").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "i"]);
    }

    fn background_args() -> CreateTicketArgs {
        CreateTicketArgs {
            project: None,
            title: Some("Add login".into()),
            description: String::new(),
            prompt: Some("go".into()),
            agent: None,
            background: true,
        }
    }

    #[test]
    fn background_flag_starts_session_in_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        init_repo(&root);

        let db = Db::open_in_memory().unwrap();
        db.create_project("kamaji", &root, Some(Agent::Claude))
            .unwrap();
        let config = Config {
            worktree_base: Some(dir.path().join("wts").to_string_lossy().to_string()),
            ..Config::default()
        };
        let state_dir = dir.path().join("state");

        let out = run_create_ticket(&db, &config, &background_args(), &root, &state_dir).unwrap();

        let spec = out.launch.expect("background should produce a launch spec");
        assert!(!out.background_failed);
        assert!(out.warning.is_none());

        let project = db.list_projects().unwrap().remove(0);
        let ticket = db.list_tickets(project.id).unwrap().remove(0);
        assert_eq!(ticket.status, crate::models::Status::InProgress);
        assert_eq!(ticket.session_name.as_deref(), Some(spec.name.as_str()));
        assert_eq!(spec.ticket_id, ticket.id);

        assert!(spec.layout_path.exists());
        let layout = std::fs::read_to_string(&spec.layout_path).unwrap();
        assert!(
            layout.contains("--settings"),
            "claude layout must inject --settings: {layout}"
        );
        assert!(spec.cwd.ends_with(&spec.name));
    }

    #[test]
    fn background_flag_in_non_git_root_stays_todo() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root"); // created but not a git repo
        std::fs::create_dir_all(&root).unwrap();

        let db = Db::open_in_memory().unwrap();
        db.create_project("kamaji", &root, Some(Agent::Claude))
            .unwrap();

        let out = run_create_ticket(
            &db,
            &Config::default(),
            &background_args(),
            &root,
            dir.path(),
        )
        .unwrap();

        assert!(out.launch.is_none(), "no session launched");
        assert!(out.background_failed, "background failure is signaled");
        assert!(out.warning.is_some(), "a reason is provided for stderr");

        let project = db.list_projects().unwrap().remove(0);
        let ticket = db.list_tickets(project.id).unwrap().remove(0);
        assert_eq!(ticket.status, crate::models::Status::Todo);
        assert!(ticket.session_name.is_none());
    }
}
