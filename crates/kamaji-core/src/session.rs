//! Building a ticket's Zellij session: worktree + layout preparation and the
//! DB commit that records it. Shared by the TUI engine and the `ticket create`
//! CLI so both paths produce identical sessions.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::Config;
use crate::models::{Agent, Project, Status, Ticket};
use crate::{agent, db::Db, detect};
use crate::{git, layout, slug, zellij_config};

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
    let instrumented = config.auto_review.enabled && ticket.agent == Agent::Claude;
    let argv = if instrumented {
        let marker = detect::marker_path(state_dir, &name);
        let _ = std::fs::create_dir_all(state_dir);
        let _ = std::fs::remove_file(&marker);
        detect::inject_claude_settings(argv, &marker.to_string_lossy())
    } else {
        argv
    };
    let bar = zellij_config::resolve_bar_style(
        &config.zellij_bar,
        zellij_config::detect_default_layout().as_deref(),
    );
    let kdl = layout::render_layout(&worktree.to_string_lossy(), &argv, bar);
    let layout_path = layout_file(&name, &kdl)?;
    Ok(Prepared {
        name,
        layout_path,
        worktree,
        instrumented,
    })
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
}
