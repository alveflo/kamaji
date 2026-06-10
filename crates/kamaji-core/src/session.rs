//! Building a ticket's Zellij session: worktree + layout preparation and the
//! DB commit that records it. Shared by the TUI engine and the `ticket create`
//! CLI so both paths produce identical sessions.

use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::Config;
use crate::models::{Agent, Project, Status, Ticket};
use crate::{agent, db::Db, detect};
use crate::{git, layout, slug, zellij, zellij_config};

/// Everything needed to launch a session, produced by `prepare_session` before
/// any DB session/status columns are written.
pub struct Prepared {
    pub name: String,
    pub layout_path: PathBuf,
    pub worktree: PathBuf,
    pub instrumented: bool,
}

/// Build the worktree + layout for a ticket without writing any DB
/// session/status columns. Shared by foreground and background start.
pub fn prepare_session(
    project: &Project,
    config: &Config,
    state_dir: &Path,
    ticket: &Ticket,
) -> Result<Prepared> {
    let argv = agent::build_command(
        config.commands_for(ticket.agent),
        ticket.initial_prompt.as_deref(),
    );
    prepare_with_argv(project, config, state_dir, ticket, argv)
}

/// Build the worktree + layout to *resume* a ticket whose persisted session was
/// resurrected (e.g. after a reboot). Identical to [`prepare_session`] except
/// the agent runs its resume command — continuing the prior conversation —
/// instead of replaying the initial prompt, and the existing worktree is reused.
pub fn prepare_resume_session(
    project: &Project,
    config: &Config,
    state_dir: &Path,
    ticket: &Ticket,
    resume_argv: Vec<String>,
) -> Result<Prepared> {
    prepare_with_argv(project, config, state_dir, ticket, resume_argv)
}

/// Shared core: resolve the worktree (creating it only if absent), instrument
/// Claude with idle hooks, render the layout, and return the [`Prepared`]. The
/// only thing that differs between a fresh start and a resume is `argv`.
fn prepare_with_argv(
    project: &Project,
    config: &Config,
    state_dir: &Path,
    ticket: &Ticket,
    argv: Vec<String>,
) -> Result<Prepared> {
    ensure_agent_command(&argv)?;

    let root = project.root_dir.clone();
    if !git::is_git_repo(&root) {
        bail!("project root is not a git repository: {}", root.display());
    }
    let name = slug::ticket_name(ticket.id, &ticket.title);
    let base = if config.base_branch == "auto" {
        git::default_branch(&root)?
    } else {
        config.base_branch.clone()
    };
    let Some(worktree) = config.worktree_dir(&root, &name) else {
        bail!("no worktree location configured; set one in the TUI (press w) or in config.toml");
    };
    if !worktree.exists() {
        git::add_worktree(&root, &worktree, &name, &base)?;
    }
    let instrumented =
        config.auto_review.enabled && matches!(ticket.agent, Agent::Claude | Agent::Codex);
    let argv = if instrumented {
        let marker = detect::marker_path(state_dir, &name);
        let _ = std::fs::create_dir_all(state_dir);
        let _ = std::fs::remove_file(&marker); // start "active": no marker
        match ticket.agent {
            // Claude takes per-invocation hooks via --settings.
            Agent::Claude => detect::inject_claude_settings(argv, &marker.to_string_lossy()),
            // Codex has no per-invocation hook flag; install its global
            // hooks.json (idempotent). The hook derives the same marker path
            // from $ZELLIJ_SESSION_NAME, so argv is unchanged.
            Agent::Codex => {
                detect::install_codex_hooks(state_dir)?;
                argv
            }
            _ => argv,
        }
    } else {
        argv
    };
    let bar = zellij_config::resolve_bar_style(
        &config.zellij_bar,
        zellij_config::detect_default_layout().as_deref(),
    );
    let kdl = layout::render_layout(&worktree.to_string_lossy(), &argv, bar)?;
    let layout_path = layout_file(&name, &kdl)?;
    Ok(Prepared {
        name,
        layout_path,
        worktree,
        instrumented,
    })
}

fn ensure_agent_command(argv: &[String]) -> Result<()> {
    if argv.is_empty() {
        bail!("{}", layout::EMPTY_COMMAND_CONFIG_ERROR);
    }
    Ok(())
}

/// Everything needed to launch a project's "main" session — a workspace not
/// tied to any ticket. There is no worktree and no DB state, so unlike
/// [`Prepared`] this carries only the session name and its layout.
pub struct MainPrepared {
    pub name: String,
    pub layout_path: PathBuf,
}

/// Build the layout for a project's "main" session: a bare workspace — a plain
/// shell in the project root, with no agent spawned. Creates no worktree and
/// writes no DB state, so it works in any directory and is fully derived from
/// the project + config — its existence is tracked only by zellij itself.
pub fn prepare_main_session(project: &Project, config: &Config) -> Result<MainPrepared> {
    let name = slug::main_session_name(project.id);
    let bar = zellij_config::resolve_bar_style(
        &config.zellij_bar,
        zellij_config::detect_default_layout().as_deref(),
    );
    let kdl = layout::render_shell_layout(&project.root_dir.to_string_lossy(), bar);
    let layout_path = layout_file(&name, &kdl)?;
    Ok(MainPrepared { name, layout_path })
}

/// Record a prepared session on the ticket: session columns, the instrumented
/// flag, and a move to In Progress.
pub fn commit_session(db: &Db, ticket_id: i64, p: &Prepared) -> Result<()> {
    db.set_ticket_session(ticket_id, &p.name, &p.worktree.to_string_lossy(), &p.name)?;
    db.set_ticket_instrumented(ticket_id, p.instrumented)?;
    db.set_ticket_status(ticket_id, Status::InProgress)?;
    Ok(())
}

/// Tear down a ticket's session: kill the zellij session, remove its worktree
/// and branch, delete the idle marker, and clear the ticket's session columns.
/// Best-effort on the external steps (a session/worktree may already be gone);
/// only the DB clear is required to succeed. Mirrors the TUI's
/// `Engine::cleanup_ticket` so the daemon and TUI tear down identically.
pub fn cleanup_ticket(db: &Db, root_dir: &Path, state_dir: &Path, ticket_id: i64) -> Result<()> {
    if let Some(t) = db.get_ticket(ticket_id)? {
        if let Some(name) = &t.session_name {
            zellij::terminate_session(name);
            let _ = std::fs::remove_file(detect::marker_path(state_dir, name));
        }
        if let Some(wt) = &t.worktree_path {
            let _ = git::remove_worktree(root_dir, wt);
        }
        if let Some(b) = &t.branch {
            let _ = git::delete_branch(root_dir, b);
        }
        db.clear_ticket_session(ticket_id)?;
    }
    Ok(())
}

/// Reconcile recorded sessions against the live `zellij list-sessions` output.
/// For every ticket whose `session_name` is NOT present in `sessions`, clear its
/// session columns and remove its idle marker, and collect `(id, name)`. When
/// `sessions` is `None` (zellij couldn't be queried) this is a no-op, so a
/// transient failure never wipes valid state. The session list is a parameter
/// (not fetched here) so callers control it and tests can inject a crafted list.
pub fn reconcile(
    db: &Db,
    tickets: &[Ticket],
    state_dir: &Path,
    sessions: Option<&str>,
) -> Result<Vec<(i64, String)>> {
    let Some(list) = sessions else {
        return Ok(Vec::new());
    };
    let vanished: Vec<(i64, String)> = tickets
        .iter()
        .filter_map(|t| {
            t.session_name
                .as_deref()
                .filter(|n| !zellij::session_in_list(list, n))
                .map(|n| (t.id, n.to_string()))
        })
        .collect();
    for (id, name) in &vanished {
        db.clear_ticket_session(*id)?;
        let _ = std::fs::remove_file(detect::marker_path(state_dir, name));
    }
    Ok(vanished)
}

/// How a live `kamaji-*` zellij session relates to the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionKind {
    /// Bound to a ticket via its recorded `session_name`.
    Ticket {
        id: i64,
        title: String,
        status: Status,
    },
    /// A project's bare "main" workspace (`kamaji-main-<project-id>`).
    Main,
    /// A `kamaji-*` session with no matching ticket and not a main session.
    Orphan,
}

/// One classified zellij session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    pub name: String,
    pub kind: SessionKind,
}

/// Classify the `kamaji-*` sessions in raw `zellij list-sessions` output against
/// the DB. The first whitespace-delimited token of each line is the session
/// name; non-`kamaji-` names are dropped, and duplicate names (zellij lists a
/// session and its EXITED/resurrectable stub separately) collapse to the first
/// occurrence, preserving list order. A name matching a ticket's recorded
/// `session_name` is `Ticket`; one matching `kamaji-main-<project-id>` is `Main`;
/// anything else is `Orphan`.
pub fn classify_sessions(db: &Db, list_output: &str) -> Result<Vec<SessionEntry>> {
    let projects = db.list_projects()?;
    let main_names: HashSet<String> = projects
        .iter()
        .map(|p| slug::main_session_name(p.id))
        .collect();
    let mut ticket_by_session: HashMap<String, (i64, String, Status)> = HashMap::new();
    for p in &projects {
        for t in db.list_tickets(p.id)? {
            if let Some(name) = t.session_name {
                ticket_by_session.insert(name, (t.id, t.title, t.status));
            }
        }
    }

    let mut seen: HashSet<&str> = HashSet::new();
    let mut entries = Vec::new();
    for line in list_output.lines() {
        let Some(name) = line.split_whitespace().next() else {
            continue;
        };
        if !name.starts_with("kamaji-") || !seen.insert(name) {
            continue;
        }
        let kind = if let Some((id, title, status)) = ticket_by_session.get(name) {
            SessionKind::Ticket {
                id: *id,
                title: title.clone(),
                status: *status,
            }
        } else if main_names.contains(name) {
            SessionKind::Main
        } else {
            SessionKind::Orphan
        };
        entries.push(SessionEntry {
            name: name.to_string(),
            kind,
        });
    }
    Ok(entries)
}

/// Write a rendered layout to a uniquely-named temp file and return its path.
fn layout_file(name: &str, contents: &str) -> Result<PathBuf> {
    static LAYOUT_COUNTER: AtomicU64 = AtomicU64::new(0);

    let dir = std::env::temp_dir().join("kamaji-layouts");
    std::fs::create_dir_all(&dir)?;
    let counter = LAYOUT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("{name}-{}-{counter}.kdl", std::process::id()));
    std::fs::write(&path, contents)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconcile_clears_vanished_sessions_and_returns_them() {
        let db = Db::open_in_memory().unwrap();
        let p = db
            .create_project("p", std::path::Path::new("/tmp/p"), None)
            .unwrap();
        // t1 has a session that is STILL in the list; t2's session has vanished.
        let t1 = db
            .create_ticket(p.id, "a", "", None, Agent::Claude)
            .unwrap();
        db.set_ticket_session(t1.id, "kamaji-1-a", "/wt1", "kamaji-1-a")
            .unwrap();
        let t2 = db
            .create_ticket(p.id, "b", "", None, Agent::Claude)
            .unwrap();
        db.set_ticket_session(t2.id, "kamaji-2-b", "/wt2", "kamaji-2-b")
            .unwrap();
        // t3 has no session at all — it must be skipped entirely.
        let t3 = db
            .create_ticket(p.id, "c", "", None, Agent::Claude)
            .unwrap();

        let state_dir = tempfile::tempdir().unwrap();
        let marker2 = crate::detect::marker_path(state_dir.path(), "kamaji-2-b");
        std::fs::write(&marker2, "").unwrap();

        let tickets = db.list_tickets(p.id).unwrap();
        // The session list contains only t1's session — t2's has vanished.
        let sessions = "kamaji-1-a [Created 1h ago]\n";
        let vanished = reconcile(&db, &tickets, state_dir.path(), Some(sessions)).unwrap();

        // Only t2 vanished; t3 (no session) is not even considered.
        assert_eq!(vanished, vec![(t2.id, "kamaji-2-b".to_string())]);
        assert_eq!(db.get_ticket(t3.id).unwrap().unwrap().session_name, None);
        // t2's columns cleared + marker removed; t1 untouched.
        assert_eq!(db.get_ticket(t2.id).unwrap().unwrap().session_name, None);
        assert!(!marker2.exists());
        assert_eq!(
            db.get_ticket(t1.id)
                .unwrap()
                .unwrap()
                .session_name
                .as_deref(),
            Some("kamaji-1-a")
        );
    }

    #[test]
    fn reconcile_is_a_noop_when_session_list_is_unavailable() {
        let db = Db::open_in_memory().unwrap();
        let p = db
            .create_project("p", std::path::Path::new("/tmp/p"), None)
            .unwrap();
        let t = db
            .create_ticket(p.id, "a", "", None, Agent::Claude)
            .unwrap();
        db.set_ticket_session(t.id, "kamaji-1-a", "/wt", "kamaji-1-a")
            .unwrap();
        let tickets = db.list_tickets(p.id).unwrap();
        let state_dir = tempfile::tempdir().unwrap();

        // `None` means zellij couldn't be queried — never clear anything.
        let vanished = reconcile(&db, &tickets, state_dir.path(), None).unwrap();
        assert!(vanished.is_empty());
        assert_eq!(
            db.get_ticket(t.id)
                .unwrap()
                .unwrap()
                .session_name
                .as_deref(),
            Some("kamaji-1-a")
        );
    }

    #[test]
    fn prepare_session_rejects_empty_agent_command_as_config_error() {
        let mut config = Config::default();
        config.agents.claude.no_prompt = Vec::new();

        let project = Project {
            id: 1,
            name: "proj".into(),
            root_dir: PathBuf::from("/definitely/not/a/git/repo"),
            default_agent: None,
            created_at: String::new(),
        };
        let ticket = Ticket {
            id: 1,
            project_id: project.id,
            title: "ticket".into(),
            description: String::new(),
            initial_prompt: None,
            agent: Agent::Claude,
            status: Status::Todo,
            session_name: None,
            worktree_path: None,
            branch: None,
            auto_reviewed: false,
            instrumented: false,
            created_at: String::new(),
            updated_at: String::new(),
        };

        let err = match prepare_session(
            &project,
            &config,
            std::path::Path::new("/tmp/kamaji-state"),
            &ticket,
        ) {
            Ok(_) => panic!("empty agent command should be rejected"),
            Err(e) => e.to_string(),
        };

        assert!(err.contains("config error: agent command must not be empty"));
        assert!(err.contains("no_prompt"));
    }

    fn project(id: i64, root: PathBuf, default_agent: Option<Agent>) -> Project {
        Project {
            id,
            name: "proj".into(),
            root_dir: root,
            default_agent,
            created_at: String::new(),
        }
    }

    /// The main session is a bare workspace: a plain shell in the project root
    /// (no worktree, no agent), under the project's main-session name.
    #[test]
    fn prepare_main_session_opens_bare_shell_in_project_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let p = prepare_main_session(&project(7, root.clone(), None), &Config::default()).unwrap();

        assert_eq!(p.name, slug::main_session_name(7));
        assert!(p.layout_path.exists());
        let kdl = std::fs::read_to_string(&p.layout_path).unwrap();
        // No agent (or any other) command is spawned: the pane is a plain shell.
        assert!(
            !kdl.contains("command="),
            "main session must launch no agent:\n{kdl}"
        );
        // Escape the path the same way `render_shell_layout` does: on Windows the
        // root contains backslashes, which KDL rendering doubles, so a raw
        // substring check would fail there.
        let cwd_esc = layout::kdl_escape(&root.to_string_lossy());
        assert!(
            kdl.contains(&format!("cwd=\"{cwd_esc}\"")),
            "shell should open in the project root, not a worktree:\n{kdl}"
        );
    }

    /// Even with a project-level default agent configured, the main session
    /// never launches it — it is an empty workspace by design.
    #[test]
    fn prepare_main_session_never_launches_an_agent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let p = prepare_main_session(&project(1, root, Some(Agent::Codex)), &Config::default())
            .unwrap();
        let kdl = std::fs::read_to_string(&p.layout_path).unwrap();
        assert!(
            !kdl.contains("command="),
            "main session must not spawn the project default agent:\n{kdl}"
        );
    }

    /// Unlike ticket sessions, the main session must not require the project
    /// root to be a git repository (there is no worktree to create).
    #[test]
    fn prepare_main_session_works_outside_a_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert!(prepare_main_session(
            &project(2, dir.path().to_path_buf(), None),
            &Config::default()
        )
        .is_ok());
    }

    #[test]
    fn classify_sessions_labels_ticket_main_and_orphan() {
        let db = Db::open_in_memory().unwrap();
        let p = db
            .create_project("p", std::path::Path::new("/tmp/p"), None)
            .unwrap();
        // A ticket with a live session.
        let t = db
            .create_ticket(p.id, "Fix auth", "", None, Agent::Claude)
            .unwrap();
        db.set_ticket_session(t.id, "kamaji-1-fix-auth", "/wt", "kamaji-1-fix-auth")
            .unwrap();

        let main = crate::slug::main_session_name(p.id);
        let list = format!(
            "kamaji-1-fix-auth [Created 2h ago]\n\
             {main} [Created 1h ago]\n\
             kamaji-7-old [Created 3h ago]\n\
             other-session (current)\n\
             kamaji-1-fix-auth [Created 2h ago] (EXITED - attach to resurrect)\n"
        );

        let entries = classify_sessions(&db, &list).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["kamaji-1-fix-auth", main.as_str(), "kamaji-7-old"]
        );

        match &entries[0].kind {
            SessionKind::Ticket { id, title, status } => {
                assert_eq!(*id, t.id);
                assert_eq!(title, "Fix auth");
                assert_eq!(*status, Status::Todo);
            }
            other => panic!("expected ticket kind, got {other:?}"),
        }
        assert!(matches!(entries[1].kind, SessionKind::Main));
        assert!(matches!(entries[2].kind, SessionKind::Orphan));
    }

    #[test]
    fn classify_sessions_empty_list_is_empty() {
        let db = Db::open_in_memory().unwrap();
        assert!(classify_sessions(&db, "").unwrap().is_empty());
    }

    /// cleanup_ticket removes the worktree + branch and clears the ticket's
    /// session columns. Uses a real temp git repo (git is available in tests).
    #[test]
    fn prepare_session_instruments_codex_and_installs_hooks() {
        // Real git repo so `worktree add` has a base.
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        let run = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.email", "t@t.t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("README.md"), "hi").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "init"]);

        let hooks_dir = tempfile::tempdir().unwrap();
        let hooks_path = hooks_dir.path().join("hooks.json");
        std::env::set_var("KAMAJI_CODEX_HOOKS_PATH", &hooks_path);

        let wt = tempfile::tempdir().unwrap();
        let config = Config {
            worktree_base: Some(wt.path().join("wt").to_string_lossy().to_string()),
            ..Config::default()
        };

        let db = Db::open_in_memory().unwrap();
        let project = db.create_project("p", root, None).unwrap();
        let ticket = db
            .create_ticket(project.id, "codex task", "", None, Agent::Codex)
            .unwrap();

        let state_dir = tempfile::tempdir().unwrap();
        let prepared = prepare_session(&project, &config, state_dir.path(), &ticket).unwrap();

        assert!(prepared.instrumented, "codex sessions must be instrumented");
        assert!(hooks_path.exists(), "codex hooks file should be installed");
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&hooks_path).unwrap()).unwrap();
        assert!(v["hooks"]["Stop"].is_array());

        std::env::remove_var("KAMAJI_CODEX_HOOKS_PATH");
    }

    #[test]
    fn cleanup_ticket_removes_worktree_and_clears_session() {
        // A committed git repo so `worktree add` has a base.
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        let run = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.email", "t@t.t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("README.md"), "hi").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "init"]);

        let worktree = repo.path().join("..").join("kamaji-wt-cleanup");
        let _ = crate::git::remove_worktree(root, &worktree); // ignore if absent
        crate::git::add_worktree(root, &worktree, "kamaji-1-x", "main").unwrap();
        assert!(worktree.exists());

        let state_dir = tempfile::tempdir().unwrap();
        let marker = crate::detect::marker_path(state_dir.path(), "kamaji-1-x");
        std::fs::write(&marker, "").unwrap();

        let db = Db::open_in_memory().unwrap();
        let p = db.create_project("p", root, None).unwrap();
        let t = db
            .create_ticket(p.id, "x", "", None, Agent::Claude)
            .unwrap();
        db.set_ticket_session(
            t.id,
            "kamaji-1-x",
            &worktree.to_string_lossy(),
            "kamaji-1-x",
        )
        .unwrap();

        cleanup_ticket(&db, root, state_dir.path(), t.id).unwrap();

        assert!(!worktree.exists(), "worktree should be removed");
        assert!(!marker.exists(), "idle marker should be removed");
        // The branch must be deleted too (otherwise a retry can't recreate it).
        let branches = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["branch", "--list", "kamaji-1-x"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&branches.stdout).trim().is_empty(),
            "branch kamaji-1-x should be deleted"
        );
        let got = db.get_ticket(t.id).unwrap().unwrap();
        assert_eq!(got.session_name, None);
        assert_eq!(got.worktree_path, None);
        assert_eq!(got.branch, None);
    }
}
