//! Building a ticket's Zellij session: worktree + layout preparation and the
//! DB commit that records it. Shared by the TUI engine and the `ticket create`
//! CLI so both paths produce identical sessions.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::Config;
use crate::db::Db;
use crate::models::{Agent, Project, Status, Ticket};
use crate::{agent, detect, git, layout, slug, zellij_config};

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
    let worktree = config.worktree_dir(&root, &name);
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
