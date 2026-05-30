use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::client::{ClientError, DaemonClient};
use kamaji_core::config::Config;
use kamaji_core::models::{Agent, Project, Ticket};

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
    Tui(DaemonOpts),
    Help,
    Version,
    CreateTicket(CreateTicketArgs),
}

/// Escape-hatch daemon options for the TUI entrypoint.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DaemonOpts {
    /// `--daemon <ADDR>`: connect to this address, never spawn.
    pub forced_addr: Option<String>,
    /// `--no-spawn`: fail if no daemon is already running.
    pub no_spawn: bool,
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

/// Result of `run_create_ticket`: the ticket is always created via the daemon.
/// When `--background` is requested the daemon also starts the session, so the
/// caller never launches zellij itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOutcome {
    /// Summary to print to stdout (always the "Created ticket #N" line).
    pub message: String,
    /// Reason to print to stderr when `--background` could not be honored.
    pub warning: Option<String>,
    /// True when `--background` was requested but the daemon could not start the
    /// session; the ticket stays in Todo and the caller should exit non-zero.
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
        return Ok(Command::Tui(DaemonOpts::default()));
    }
    if args == ["--help"] || args == ["-h"] || args == ["help"] {
        return Ok(Command::Help);
    }
    if args == ["--version"] || args == ["-V"] || args == ["version"] {
        return Ok(Command::Version);
    }
    // Leading TUI escape-hatch flags: `--daemon <addr>` / `--no-spawn`. These are
    // only valid when no `ticket` subcommand follows.
    if matches!(args[0].as_str(), "--daemon" | "--no-spawn") {
        return parse_tui(&args);
    }
    match args.as_slice() {
        [scope, action, rest @ ..] if scope == "ticket" || scope == "tickets" => {
            match action.as_str() {
                "create" | "new" => parse_ticket_create(rest),
                _ => bail!("unknown ticket command: {action}\n\n{USAGE}"),
            }
        }
        [other, ..] => bail!("unknown command: {other}\n\n{USAGE}"),
        [] => Ok(Command::Tui(DaemonOpts::default())),
    }
}

/// Parse the leading TUI daemon escape-hatch flags into `Command::Tui`.
fn parse_tui(args: &[String]) -> Result<Command> {
    let mut opts = DaemonOpts::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--daemon" => opts.forced_addr = Some(take_value(args, &mut i, "--daemon")?),
            "--no-spawn" => opts.no_spawn = true,
            other => bail!("unexpected argument after TUI flags: {other}\n\n{USAGE}"),
        }
        i += 1;
    }
    Ok(Command::Tui(opts))
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
    client: &DaemonClient,
    config: &Config,
    args: &CreateTicketArgs,
    cwd: &Path,
) -> Result<CreateOutcome> {
    let project = match args.project.as_deref() {
        Some(selector) => select_project(client, selector)?,
        None => infer_project(client, cwd)?,
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
    let ticket = client
        .create_ticket(project.id, &title, args.description.trim(), prompt, agent)
        .map_err(|e| anyhow!("{e:?}"))?;
    let mut message = format!(
        "Created ticket #{} in project {}: {}",
        ticket.id, project.name, ticket.title
    );

    if !args.background {
        return Ok(CreateOutcome {
            message,
            warning: None,
            background_failed: false,
        });
    }

    // --background: the daemon owns the session start. A `BadRequest` (e.g. the
    // project root is not a git repo) leaves the ticket in Todo; report it as a
    // warning and tell the caller to exit non-zero.
    match client.start_ticket(ticket.id) {
        Ok(t) => {
            message.push_str(&format!(
                "\nStarted '{}' in the background",
                t.session_name.as_deref().unwrap_or("")
            ));
            Ok(CreateOutcome {
                message,
                warning: None,
                background_failed: false,
            })
        }
        Err(ClientError::BadRequest(m)) => Ok(CreateOutcome {
            message,
            warning: Some(m),
            background_failed: true,
        }),
        Err(e) => Ok(CreateOutcome {
            message,
            warning: Some(format!("{e:?}")),
            background_failed: true,
        }),
    }
}

fn select_project(client: &DaemonClient, selector: &str) -> Result<Project> {
    if let Ok(id) = selector.parse::<i64>() {
        return match client.get_project(id) {
            Ok(p) => Ok(p),
            Err(ClientError::NotFound) => bail!("no project with id {id}"),
            Err(e) => bail!("{e:?}"),
        };
    }
    let matches: Vec<Project> = client
        .list_projects()
        .map_err(|e| anyhow!("{e:?}"))?
        .into_iter()
        .filter(|p| p.name == selector)
        .collect();
    match matches.as_slice() {
        [project] => Ok(project.clone()),
        [] => bail!("no project named {selector:?}"),
        _ => bail!("more than one project named {selector:?}; use --project <id>"),
    }
}

fn infer_project(client: &DaemonClient, cwd: &Path) -> Result<Project> {
    let projects = client.list_projects().map_err(|e| anyhow!("{e:?}"))?;
    let mut candidates: Vec<(Project, Vec<Ticket>)> = Vec::with_capacity(projects.len());
    for p in projects {
        let tickets = client.list_tickets(p.id).map_err(|e| anyhow!("{e:?}"))?;
        candidates.push((p, tickets));
    }
    pick_project(&candidates, cwd)
}

/// Pure project-inference logic; no network I/O. `candidates` is a list of
/// `(project, its tickets)` pairs. Rules (in priority order):
///   1. Any project whose `root_dir` or whose ticket's `worktree_path` is a
///      prefix of `cwd` is a match, scored by the number of path components
///      (longer prefix wins).
///   2. Ties at the same score but different projects → ambiguous error.
///   3. No path match + exactly one project → return that project.
///   4. No path match + multiple projects → "could not infer" error.
fn pick_project(candidates: &[(Project, Vec<Ticket>)], cwd: &Path) -> Result<Project> {
    let cwd = normalize_path(cwd);
    if candidates.is_empty() {
        bail!("no kamaji projects exist; create one in the TUI first");
    }

    let mut matches: Vec<(usize, Project)> = Vec::new();
    for (project, tickets) in candidates {
        let root = normalize_path(&project.root_dir);
        if cwd.starts_with(&root) {
            matches.push((root.components().count(), project.clone()));
        }
        for ticket in tickets {
            if let Some(worktree) = &ticket.worktree_path {
                let worktree = normalize_path(worktree);
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

    if candidates.len() == 1 {
        return Ok(candidates[0].0.clone());
    }

    let names = candidates
        .iter()
        .map(|(p, _)| format!("{} ({})", p.id, p.name))
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
    use crate::test_support::spawn_test_daemon;

    /// A `DaemonClient` connected to a *dedicated* in-process kamajid (in-memory
    /// DB, default config — never touches the developer's real state). Each test
    /// gets its own daemon so the project/ticket inference assertions are
    /// isolated.
    fn test_client() -> DaemonClient {
        DaemonClient::connect(spawn_test_daemon()).unwrap()
    }

    /// A client whose daemon already has one "kamaji" project rooted at `root`.
    fn client_with_project(root: &Path) -> DaemonClient {
        let client = test_client();
        client
            .create_project("kamaji", root, Some(Agent::Codex))
            .unwrap();
        client
    }

    #[test]
    fn parses_version_flag() {
        assert_eq!(parse(["--version"]).unwrap(), Command::Version);
        assert_eq!(parse(["-V"]).unwrap(), Command::Version);
        assert_eq!(parse(["version"]).unwrap(), Command::Version);
    }

    #[test]
    fn no_args_runs_tui() {
        assert_eq!(
            parse(Vec::<String>::new()).unwrap(),
            Command::Tui(DaemonOpts::default())
        );
    }

    #[test]
    fn parses_daemon_and_no_spawn_flags() {
        assert_eq!(
            parse(["--no-spawn"]).unwrap(),
            Command::Tui(DaemonOpts {
                forced_addr: None,
                no_spawn: true
            })
        );
        assert_eq!(
            parse(["--daemon", "127.0.0.1:9000"]).unwrap(),
            Command::Tui(DaemonOpts {
                forced_addr: Some("127.0.0.1:9000".into()),
                no_spawn: false
            })
        );
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
    fn cli_create_ticket_via_daemon_infers_single_project() {
        let dir = tempfile::tempdir().unwrap();
        let client = client_with_project(dir.path());
        let args = CreateTicketArgs {
            project: None,
            title: None,
            description: String::new(),
            prompt: Some("Start working on issue 12".into()),
            agent: None,
            background: false,
        };

        // project=None + cwd under the only project root => inferred.
        let out = run_create_ticket(&client, &Config::default(), &args, dir.path()).unwrap();
        assert!(out.message.contains("Created ticket #1"));
        assert!(
            !out.background_failed,
            "no --background => no session start"
        );
        assert!(out.warning.is_none());

        // Read the ticket back through the client; assert exactly one exists.
        let project = client.list_projects().unwrap().remove(0);
        let tickets = client.list_tickets(project.id).unwrap();
        assert_eq!(tickets.len(), 1);
        assert_eq!(tickets[0].title, "Start working on issue 12");
        assert_eq!(
            tickets[0].initial_prompt.as_deref(),
            Some("Start working on issue 12")
        );
        // Falls back to the project's default agent (Codex) when --agent absent.
        assert_eq!(tickets[0].agent, Agent::Codex);
    }

    #[test]
    fn create_ticket_infers_single_project_regardless_of_cwd() {
        // With exactly one project, inference succeeds even when cwd is nowhere
        // near the project root (the single-project fallback).
        let dir = tempfile::tempdir().unwrap();
        let client = client_with_project(&dir.path().join("root"));
        let args = CreateTicketArgs {
            project: None,
            title: Some("Next".into()),
            description: String::new(),
            prompt: Some("Do the next thing".into()),
            agent: Some(Agent::Claude),
            background: false,
        };

        let elsewhere = tempfile::tempdir().unwrap();
        run_create_ticket(&client, &Config::default(), &args, elsewhere.path()).unwrap();
        let project = client.list_projects().unwrap().remove(0);
        let tickets = client.list_tickets(project.id).unwrap();
        assert_eq!(tickets.len(), 1);
        assert_eq!(tickets[0].title, "Next");
        // Explicit --agent overrides the project default.
        assert_eq!(tickets[0].agent, Agent::Claude);
    }

    #[test]
    fn create_ticket_ambiguous_projects_error() {
        // Two projects, neither containing cwd => inference must bail.
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let client = test_client();
        client.create_project("alpha", a.path(), None).unwrap();
        client.create_project("beta", b.path(), None).unwrap();

        let args = CreateTicketArgs {
            project: None,
            title: Some("x".into()),
            description: String::new(),
            prompt: Some("x".into()),
            agent: None,
            background: false,
        };
        let elsewhere = tempfile::tempdir().unwrap();
        let err = run_create_ticket(&client, &Config::default(), &args, elsewhere.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("could not infer project"), "{err}");
    }

    #[test]
    fn create_ticket_explicit_select_by_name() {
        // Even with multiple projects, an explicit --project name selects it.
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let client = test_client();
        client.create_project("alpha", a.path(), None).unwrap();
        let beta = client.create_project("beta", b.path(), None).unwrap();

        let args = CreateTicketArgs {
            project: Some("beta".into()),
            title: Some("x".into()),
            description: String::new(),
            prompt: Some("x".into()),
            agent: None,
            background: false,
        };
        let elsewhere = tempfile::tempdir().unwrap();
        run_create_ticket(&client, &Config::default(), &args, elsewhere.path()).unwrap();
        let tickets = client.list_tickets(beta.id).unwrap();
        assert_eq!(tickets.len(), 1);
        // The other project got nothing.
        let alpha = client
            .list_projects()
            .unwrap()
            .into_iter()
            .find(|p| p.name == "alpha")
            .unwrap();
        assert!(client.list_tickets(alpha.id).unwrap().is_empty());
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

    // ── pure pick_project tests (no daemon, no network) ──────────────────────

    fn sample_project(id: i64, name: &str, root: &Path) -> Project {
        Project {
            id,
            name: name.to_string(),
            root_dir: root.to_path_buf(),
            default_agent: None,
            created_at: "2026-05-30T00:00:00Z".to_string(),
        }
    }

    fn sample_ticket(id: i64, project_id: i64, worktree_path: Option<PathBuf>) -> Ticket {
        Ticket {
            id,
            project_id,
            title: "sample".to_string(),
            description: String::new(),
            initial_prompt: None,
            agent: Agent::Claude,
            status: kamaji_core::models::Status::InProgress,
            position: 0,
            session_name: None,
            worktree_path,
            branch: None,
            auto_reviewed: false,
            instrumented: false,
            created_at: "2026-05-30T00:00:00Z".to_string(),
            updated_at: "2026-05-30T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn pick_project_empty_candidates_errors() {
        let err = pick_project(&[], Path::new("/some/dir"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("no kamaji projects exist"), "{err}");
    }

    #[test]
    fn pick_project_single_project_fallback() {
        // cwd nowhere near the root → single-project fallback kicks in.
        let dir = tempfile::tempdir().unwrap();
        let p = sample_project(1, "solo", &dir.path().join("root"));
        let candidates = vec![(p.clone(), vec![])];
        let elsewhere = tempfile::tempdir().unwrap();
        let got = pick_project(&candidates, elsewhere.path()).unwrap();
        assert_eq!(got.id, p.id);
    }

    #[test]
    fn pick_project_matches_root_dir() {
        let dir = tempfile::tempdir().unwrap();
        let p = sample_project(1, "myproject", dir.path());
        let candidates = vec![(p.clone(), vec![])];
        // cwd is a subdirectory of the project root.
        let sub = dir.path().join("src");
        std::fs::create_dir_all(&sub).unwrap();
        let got = pick_project(&candidates, &sub).unwrap();
        assert_eq!(got.id, p.id);
    }

    #[test]
    fn pick_project_matches_worktree_path() {
        // Two projects, cwd is inside project B's ticket worktree.
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let worktree_dir = tempfile::tempdir().unwrap();

        let pa = sample_project(1, "alpha", dir_a.path());
        let pb = sample_project(2, "beta", dir_b.path());

        let ticket = sample_ticket(10, pb.id, Some(worktree_dir.path().to_path_buf()));

        // cwd is a subdirectory inside the worktree.
        let cwd = worktree_dir.path().join("subdir");
        std::fs::create_dir_all(&cwd).unwrap();

        let candidates = vec![(pa, vec![]), (pb.clone(), vec![ticket])];
        let got = pick_project(&candidates, &cwd).unwrap();
        assert_eq!(got.id, pb.id, "should infer beta via worktree_path");
    }

    #[test]
    fn pick_project_worktree_beats_shorter_root() {
        // project root is /tmp/proj, worktree is /tmp/proj/worktrees/feat.
        // cwd inside the worktree should prefer the worktree match (more components).
        let proj_dir = tempfile::tempdir().unwrap();
        let worktree_dir = proj_dir.path().join("worktrees").join("feat");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let p = sample_project(1, "myproject", proj_dir.path());
        let ticket = sample_ticket(1, p.id, Some(worktree_dir.clone()));
        // Even though there's only one project, both branches are covered.
        let candidates = vec![(p.clone(), vec![ticket])];
        let got = pick_project(&candidates, &worktree_dir).unwrap();
        assert_eq!(got.id, p.id);
    }

    #[test]
    fn pick_project_ambiguous_same_score_errors() {
        // Two projects whose roots both contain cwd (e.g. nested roots at same depth).
        // We simulate same-score ambiguity by using the exact same directory for both roots.
        let dir = tempfile::tempdir().unwrap();
        let pa = sample_project(1, "alpha", dir.path());
        let pb = sample_project(2, "beta", dir.path());
        let candidates = vec![(pa, vec![]), (pb, vec![])];
        let err = pick_project(&candidates, dir.path())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("matches multiple projects"),
            "expected ambiguity error, got: {err}"
        );
    }

    #[test]
    fn pick_project_no_match_multiple_projects_errors() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let pa = sample_project(1, "alpha", a.path());
        let pb = sample_project(2, "beta", b.path());
        let candidates = vec![(pa, vec![]), (pb, vec![])];
        let err = pick_project(&candidates, elsewhere.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("could not infer project"), "{err}");
        assert!(err.contains("alpha"), "{err}");
        assert!(err.contains("beta"), "{err}");
    }

    /// Needs a real git repo + zellij on PATH (the daemon spawns the detached
    /// session), so it is ignored by default.
    #[test]
    #[ignore]
    fn cli_create_ticket_background_starts_via_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        init_repo(&root);

        let client = client_with_project(&root);
        let out =
            run_create_ticket(&client, &Config::default(), &background_args(), &root).unwrap();
        assert!(!out.background_failed, "{:?}", out.warning);
        assert!(out.warning.is_none());

        let project = client.list_projects().unwrap().remove(0);
        let ticket = client.list_tickets(project.id).unwrap().remove(0);
        assert_eq!(ticket.status, kamaji_core::models::Status::InProgress);
        assert!(
            ticket.session_name.is_some(),
            "daemon should have started a session"
        );
    }
}
