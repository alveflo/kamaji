use anyhow::{bail, Result};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::app::{App, FormField, Modal, TicketForm};
use crate::config::Config;
use crate::db::Db;
use crate::detect::{self, SignalLevel};
use crate::models::{Agent, Status, Ticket};
use crate::{agent, git, layout, slug, zellij, zellij_config};

/// Side effect the main loop must run by releasing the terminal.
#[derive(Debug, PartialEq)]
pub enum Effect {
    None,
    RunSession {
        name: String,
        layout_path: PathBuf,
    },
    Attach {
        name: String,
    },
    /// Leave the board and return to the project picker.
    SwitchProject,
}

pub struct Engine {
    pub db: Db,
    pub config: Config,
    pub app: App,
    /// Last observed signal level per ticket id (in-memory; re-baselined on restart).
    pub last_level: HashMap<i64, SignalLevel>,
    /// Tickets kamaji auto-moved to Review (provenance gate for the move back).
    pub auto_review_ids: HashSet<i64>,
    /// Per-ticket scrape screen hash for the stability guard.
    pub scrape_hash: HashMap<i64, Option<u64>>,
    /// Where per-session idle markers live.
    pub state_dir: std::path::PathBuf,
}

impl Engine {
    pub fn new(db: Db, config: Config, app: App) -> Self {
        Engine {
            db,
            config,
            app,
            last_level: HashMap::new(),
            auto_review_ids: HashSet::new(),
            scrape_hash: HashMap::new(),
            state_dir: detect::default_state_dir(),
        }
    }

    pub fn reload(&mut self) -> Result<()> {
        self.app.tickets = self.db.list_tickets(self.app.project.id)?;
        self.app.reclamp();
        Ok(())
    }

    fn layout_file(&self, name: &str, contents: &str) -> Result<PathBuf> {
        static LAYOUT_COUNTER: AtomicU64 = AtomicU64::new(0);

        let dir = std::env::temp_dir().join("kamaji-layouts");
        std::fs::create_dir_all(&dir)?;
        let counter = LAYOUT_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("{name}-{}-{counter}.kdl", std::process::id()));
        std::fs::write(&path, contents)?;
        Ok(path)
    }

    /// Create the worktree + layout for a ticket and return the RunSession effect.
    fn start_session(&mut self, ticket: &Ticket) -> Result<Effect> {
        let root = self.app.project.root_dir.clone();
        if !git::is_git_repo(&root) {
            bail!("project root is not a git repository: {}", root.display());
        }
        let name = slug::ticket_name(ticket.id, &ticket.title);
        let base = if self.config.base_branch == "auto" {
            git::default_branch(&root)?
        } else {
            self.config.base_branch.clone()
        };
        let worktree = self.config.worktree_dir(&root, &name);
        if !worktree.exists() {
            git::add_worktree(&root, &worktree, &name, &base)?;
        }
        let argv = agent::build_command(
            self.config.commands_for(ticket.agent),
            ticket.initial_prompt.as_deref(),
        );
        // For Claude, inject hook settings that maintain the idle marker, and
        // clear any stale marker so the session baselines as Active.
        let argv = if self.config.auto_review.enabled && ticket.agent == Agent::Claude {
            let marker = detect::marker_path(&self.state_dir, &name);
            let _ = std::fs::create_dir_all(&self.state_dir);
            let _ = std::fs::remove_file(&marker);
            detect::inject_claude_settings(argv, &marker.to_string_lossy())
        } else {
            argv
        };
        // Resolve the bar style: config override (compact/default/none) else
        // auto-detect from the user's zellij default_layout.
        let bar = zellij_config::resolve_bar_style(
            &self.config.zellij_bar,
            zellij_config::detect_default_layout().as_deref(),
        );
        let kdl = layout::render_layout(&worktree.to_string_lossy(), &argv, bar);
        let layout_path = self.layout_file(&name, &kdl)?;
        self.db
            .set_ticket_session(ticket.id, &name, &worktree.to_string_lossy(), &name)?;
        self.db.set_ticket_status(ticket.id, Status::InProgress)?;
        self.reload()?;
        Ok(Effect::RunSession { name, layout_path })
    }

    /// Apply a column move to the currently-selected ticket.
    #[allow(dead_code)]
    pub fn move_selected(&mut self, target: Status) -> Result<Effect> {
        let Some(ticket) = self.app.selected_ticket().cloned() else {
            return Ok(Effect::None);
        };
        self.apply_move(ticket, target)
    }

    /// Apply a column move to a ticket identified by id (used by the Move modal
    /// so the move targets the ticket the modal was opened for, independent of
    /// the current cursor).
    fn move_ticket(&mut self, ticket_id: i64, target: Status) -> Result<Effect> {
        let Some(ticket) = self.app.tickets.iter().find(|t| t.id == ticket_id).cloned() else {
            return Ok(Effect::None);
        };
        self.apply_move(ticket, target)
    }

    fn apply_move(&mut self, ticket: Ticket, target: Status) -> Result<Effect> {
        self.auto_review_ids.remove(&ticket.id);
        if target == Status::InProgress {
            return match ticket.session_name.clone() {
                Some(name) => {
                    self.db.set_ticket_status(ticket.id, Status::InProgress)?;
                    self.reload()?;
                    Ok(Effect::Attach { name })
                }
                None => self.start_session(&ticket),
            };
        }
        self.db.set_ticket_status(ticket.id, target)?;
        self.reload()?;
        Ok(Effect::None)
    }

    /// Terminate session + remove worktree + delete branch for a ticket, then
    /// clear the recorded session columns so the ticket no longer shows as live.
    pub fn cleanup_ticket(&mut self, ticket_id: i64) -> Result<()> {
        if let Some(t) = self.db.get_ticket(ticket_id)? {
            if let Some(name) = &t.session_name {
                zellij::terminate_session(name);
                let _ = std::fs::remove_file(detect::marker_path(&self.state_dir, name));
            }
            let root = &self.app.project.root_dir;
            if let Some(wt) = &t.worktree_path {
                let _ = git::remove_worktree(root, wt);
            }
            if let Some(b) = &t.branch {
                let _ = git::delete_branch(root, b);
            }
            self.db.clear_ticket_session(ticket_id)?;
            self.forget_ticket_state(ticket_id);
        }
        Ok(())
    }

    /// Reconcile recorded sessions against zellij: if a successful
    /// `zellij list-sessions` does not contain a ticket's session (including as
    /// a resurrectable/exited entry), the session is gone, so clear its columns.
    /// Does nothing if zellij can't be queried, so a transient failure never
    /// wipes valid state.
    pub fn reconcile(&mut self) -> Result<()> {
        let Some(list) = zellij::list_sessions() else {
            return Ok(());
        };
        let stale: Vec<(i64, String)> = self
            .app
            .tickets
            .iter()
            .filter_map(|t| {
                t.session_name
                    .as_deref()
                    .filter(|n| !zellij::session_in_list(&list, n))
                    .map(|n| (t.id, n.to_string()))
            })
            .collect();
        for (id, name) in stale {
            self.db.clear_ticket_session(id)?;
            let _ = std::fs::remove_file(detect::marker_path(&self.state_dir, &name));
            self.forget_ticket_state(id);
        }
        self.reload()
    }

    /// Forget all in-memory detection state for a ticket (on teardown/vanish).
    fn forget_ticket_state(&mut self, id: i64) {
        self.last_level.remove(&id);
        self.auto_review_ids.remove(&id);
        self.scrape_hash.remove(&id);
    }

    /// Apply move decisions given already-gathered signal levels. Split out from
    /// the IO so it can be unit-tested with crafted levels.
    fn detect_tick_with(&mut self, levels: &HashMap<i64, SignalLevel>) -> Result<()> {
        let mut changed = false;
        for (&id, &level) in levels {
            // Copy out the status so we don't hold an app borrow across the db write.
            let Some(status) = self
                .app
                .tickets
                .iter()
                .find(|t| t.id == id)
                .map(|t| t.status)
            else {
                continue;
            };
            let last = self.last_level.get(&id).copied();
            let was_auto = self.auto_review_ids.contains(&id);
            if let Some(target) = detect::decide(last, level, status, was_auto) {
                self.db.set_ticket_status(id, target)?;
                match target {
                    Status::Review => {
                        self.auto_review_ids.insert(id);
                        self.app.status_message =
                            Some(format!("#{id} → Needs attention (agent idle)"));
                    }
                    Status::InProgress => {
                        self.auto_review_ids.remove(&id);
                        self.app.status_message =
                            Some(format!("#{id} → In Progress (agent active)"));
                    }
                    _ => {}
                }
                changed = true;
            }
            if level != SignalLevel::Unknown {
                self.last_level.insert(id, level);
            }
        }
        if changed {
            self.reload()?;
        }
        Ok(())
    }

    /// Read the current signal level for every live, in-progress/review ticket.
    fn gather_levels(&mut self) -> HashMap<i64, SignalLevel> {
        // Snapshot first so we don't borrow `app` while mutating scrape state.
        let live: Vec<(i64, Agent, String)> = self
            .app
            .tickets
            .iter()
            .filter(|t| matches!(t.status, Status::InProgress | Status::Review))
            .filter_map(|t| t.session_name.clone().map(|s| (t.id, t.agent, s)))
            .collect();

        let mut out = HashMap::new();
        for (id, agent, session) in live {
            let level = match agent {
                Agent::Claude => {
                    detect::marker_level(&detect::marker_path(&self.state_dir, &session))
                }
                Agent::Codex | Agent::Copilot => {
                    let patterns: Vec<String> = self.config.auto_review_patterns(agent).to_vec();
                    if patterns.is_empty() {
                        continue; // detector disabled for this agent
                    }
                    let screen = zellij::dump_screen(&session);
                    let hash = self.scrape_hash.entry(id).or_insert(None);
                    detect::scrape_level(screen.as_deref(), &patterns, hash)
                }
            };
            out.insert(id, level);
        }
        out
    }

    /// One detection pass: gather levels, then apply move decisions.
    pub fn detect_tick(&mut self) -> Result<()> {
        let levels = self.gather_levels();
        self.detect_tick_with(&levels)
    }

    fn submit_form(&mut self, form: &TicketForm) -> Result<()> {
        match form.editing_id {
            Some(id) => self
                .db
                .update_ticket_fields(id, &form.title, &form.description)?,
            None => {
                self.db.create_ticket(
                    self.app.project.id,
                    &form.title,
                    &form.description,
                    form.prompt_opt().as_deref(),
                    form.agent,
                )?;
            }
        }
        self.reload()
    }

    /// Single entry point for key handling. Returns an Effect for the main loop.
    pub fn on_key(&mut self, key: KeyEvent) -> Result<Effect> {
        // Take ownership of the modal to avoid borrow conflicts.
        let modal = std::mem::replace(&mut self.app.modal, Modal::None);
        match modal {
            Modal::None => self.on_board_key(key),
            Modal::Form(mut form) => {
                match key.code {
                    KeyCode::Esc => {} // close (modal already None)
                    KeyCode::Enter => {
                        if !form.title.trim().is_empty() {
                            self.submit_form(&form)?;
                        } else {
                            self.app.modal = Modal::Form(form);
                            self.app.status_message = Some("Title is required".into());
                        }
                    }
                    KeyCode::Tab => {
                        form.next_field();
                        self.app.modal = Modal::Form(form);
                    }
                    KeyCode::BackTab => {
                        form.prev_field();
                        self.app.modal = Modal::Form(form);
                    }
                    KeyCode::Left if form.field == FormField::Agent => {
                        form.cycle_agent(false);
                        self.app.modal = Modal::Form(form);
                    }
                    KeyCode::Right if form.field == FormField::Agent => {
                        form.cycle_agent(true);
                        self.app.modal = Modal::Form(form);
                    }
                    KeyCode::Backspace => {
                        form.backspace();
                        self.app.modal = Modal::Form(form);
                    }
                    KeyCode::Char(c) => {
                        form.input_char(c);
                        self.app.modal = Modal::Form(form);
                    }
                    _ => {
                        self.app.modal = Modal::Form(form);
                    }
                }
                Ok(Effect::None)
            }
            Modal::Move {
                ticket_id,
                mut target,
            } => match key.code {
                KeyCode::Esc => Ok(Effect::None),
                KeyCode::Left => {
                    let i = Status::all().iter().position(|s| *s == target).unwrap();
                    target = Status::all()[i.saturating_sub(1)];
                    self.app.modal = Modal::Move { ticket_id, target };
                    Ok(Effect::None)
                }
                KeyCode::Right => {
                    let i = Status::all().iter().position(|s| *s == target).unwrap();
                    target = Status::all()[(i + 1).min(3)];
                    self.app.modal = Modal::Move { ticket_id, target };
                    Ok(Effect::None)
                }
                KeyCode::Enter => {
                    if target == Status::Done {
                        self.app.modal = Modal::ConfirmDone { ticket_id };
                        Ok(Effect::None)
                    } else {
                        self.move_ticket(ticket_id, target)
                    }
                }
                _ => {
                    self.app.modal = Modal::Move { ticket_id, target };
                    Ok(Effect::None)
                }
            },
            Modal::ConfirmDone { ticket_id } => match key.code {
                KeyCode::Char('y') => {
                    self.cleanup_ticket(ticket_id)?;
                    self.db.set_ticket_status(ticket_id, Status::Done)?;
                    self.reload()?;
                    Ok(Effect::None)
                }
                KeyCode::Char('n') => {
                    self.db.set_ticket_status(ticket_id, Status::Done)?;
                    self.reload()?;
                    Ok(Effect::None)
                }
                _ => Ok(Effect::None), // Esc cancels
            },
            Modal::ConfirmDelete { ticket_id } => match key.code {
                KeyCode::Char('y') => {
                    self.cleanup_ticket(ticket_id)?;
                    self.db.delete_ticket(ticket_id)?;
                    self.reload()?;
                    Ok(Effect::None)
                }
                _ => Ok(Effect::None),
            },
            Modal::Help => Ok(Effect::None), // any key closes
        }
    }

    fn on_board_key(&mut self, key: KeyEvent) -> Result<Effect> {
        self.app.status_message = None;
        match key.code {
            KeyCode::Char('q') => self.app.should_quit = true,
            KeyCode::Char('p') => return Ok(Effect::SwitchProject),
            KeyCode::Char('?') => self.app.modal = Modal::Help,
            KeyCode::Left | KeyCode::Char('h') => self.app.left(),
            KeyCode::Right | KeyCode::Char('l') => self.app.right(),
            KeyCode::Up | KeyCode::Char('k') => self.app.up(),
            KeyCode::Down | KeyCode::Char('j') => self.app.down(),
            KeyCode::Char('c') => {
                // Resolve the default agent: project override, else global config.
                let default_agent = self
                    .app
                    .project
                    .default_agent
                    .unwrap_or_else(|| self.config.default_agent());
                self.app.modal = Modal::Form(TicketForm::new_create(default_agent));
            }
            KeyCode::Char('e') => {
                if let Some(t) = self.app.selected_ticket() {
                    self.app.modal = Modal::Form(TicketForm::from_ticket(t));
                }
            }
            KeyCode::Char('m') => {
                if let Some(t) = self.app.selected_ticket() {
                    self.app.modal = Modal::Move {
                        ticket_id: t.id,
                        target: t.status,
                    };
                }
            }
            KeyCode::Char('d') => {
                if let Some(t) = self.app.selected_ticket() {
                    self.app.modal = Modal::ConfirmDelete { ticket_id: t.id };
                }
            }
            // Enter "enters" the ticket: attach to its session if one exists,
            // otherwise start one (creating the worktree + session and moving
            // the ticket to In Progress).
            KeyCode::Enter => {
                if let Some(t) = self.app.selected_ticket().cloned() {
                    return match t.session_name.clone() {
                        Some(name) => Ok(Effect::Attach { name }),
                        None => self.start_session(&t),
                    };
                }
            }
            _ => {}
        }
        Ok(Effect::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::SignalLevel;
    use crate::models::Agent;
    use ratatui::crossterm::event::{KeyEvent, KeyModifiers};
    use std::collections::HashMap;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn engine_with_project(root: std::path::PathBuf) -> Engine {
        let db = Db::open_in_memory().unwrap();
        let project = db.create_project("p", &root, None).unwrap();
        let app = App::new(project, vec![]);
        Engine::new(db, Config::default(), app)
    }

    /// Initialize a real git repo with one commit at `root` so `start_session`
    /// can add a worktree against it.
    fn init_repo(root: &std::path::Path) {
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

    #[test]
    fn create_ticket_via_form() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.on_key(key('c')).unwrap();
        for ch in "Add login".chars() {
            e.on_key(key(ch)).unwrap();
        }
        e.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(e.app.tickets.len(), 1);
        assert_eq!(e.app.tickets[0].title, "Add login");
        assert_eq!(e.app.tickets[0].status, Status::Todo);
    }

    #[test]
    fn pressing_p_requests_project_switch() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        assert_eq!(e.on_key(key('p')).unwrap(), Effect::SwitchProject);
    }

    #[test]
    fn move_to_review_then_done_without_session() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.db.create_ticket(e.app.project.id, "t", "", None, Agent::Claude)
            .unwrap();
        e.reload().unwrap();
        // Move to Review (col index 2).
        assert_eq!(e.move_selected(Status::Review).unwrap(), Effect::None);
        assert_eq!(
            e.db.list_tickets(e.app.project.id).unwrap()[0].status,
            Status::Review
        );
    }

    #[test]
    fn start_session_creates_worktree_and_effect() {
        // Build a real repo so start_session can add a worktree.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        init_repo(&root);

        let mut e = engine_with_project(root.clone());
        // Point worktrees somewhere isolated under tempdir.
        e.config.worktree_base = dir.path().join("wts").to_string_lossy().to_string();
        e.state_dir = dir.path().join("state");
        let t =
            e.db.create_ticket(e.app.project.id, "Add login", "", Some("go"), Agent::Claude)
                .unwrap();
        e.reload().unwrap();

        let effect = e.move_selected(Status::InProgress).unwrap();
        let name = slug::ticket_name(t.id, "Add login");
        match effect {
            Effect::RunSession {
                name: n,
                layout_path,
            } => {
                assert_eq!(n, name);
                assert!(layout_path.exists());
                let layout = std::fs::read_to_string(&layout_path).unwrap();
                assert!(
                    layout.contains("--settings"),
                    "claude layout must inject --settings: {layout}"
                );
            }
            other => panic!("expected RunSession, got {other:?}"),
        }
        let stored = e.db.get_ticket(t.id).unwrap().unwrap();
        assert_eq!(stored.status, Status::InProgress);
        assert_eq!(stored.session_name.as_deref(), Some(name.as_str()));
        assert!(dir.path().join("wts").join(&name).join("f").exists());

        // Cleanup removes the worktree and clears the recorded session columns.
        e.cleanup_ticket(t.id).unwrap();
        let cleaned = e.db.get_ticket(t.id).unwrap().unwrap();
        assert_eq!(cleaned.session_name, None);
        assert_eq!(cleaned.worktree_path, None);
        assert_eq!(cleaned.branch, None);
        assert!(!dir.path().join("wts").join(&name).join("f").exists());
    }

    /// Helper: an in-progress ticket with a recorded session, returns its id.
    fn in_progress_ticket(e: &mut Engine) -> i64 {
        let t =
            e.db.create_ticket(e.app.project.id, "t", "", None, Agent::Claude)
                .unwrap();
        e.db.set_ticket_session(t.id, "kamaji-x", "/wt", "kamaji-x")
            .unwrap();
        e.db.set_ticket_status(t.id, Status::InProgress).unwrap();
        e.reload().unwrap();
        t.id
    }

    fn levels(id: i64, level: SignalLevel) -> HashMap<i64, SignalLevel> {
        let mut m = HashMap::new();
        m.insert(id, level);
        m
    }

    #[test]
    fn idle_after_active_moves_in_progress_to_review() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let id = in_progress_ticket(&mut e);
        e.detect_tick_with(&levels(id, SignalLevel::Active))
            .unwrap();
        e.detect_tick_with(&levels(id, SignalLevel::Idle)).unwrap();
        assert_eq!(e.db.get_ticket(id).unwrap().unwrap().status, Status::Review);
        assert!(e.auto_review_ids.contains(&id));
        // The toast names the column as the user sees it ("Needs attention").
        assert!(e
            .app
            .status_message
            .as_deref()
            .unwrap()
            .contains("Needs attention"));
    }

    #[test]
    fn resumed_auto_reviewed_card_returns_to_in_progress() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let id = in_progress_ticket(&mut e);
        e.detect_tick_with(&levels(id, SignalLevel::Active))
            .unwrap();
        e.detect_tick_with(&levels(id, SignalLevel::Idle)).unwrap();
        e.detect_tick_with(&levels(id, SignalLevel::Active))
            .unwrap();
        assert_eq!(
            e.db.get_ticket(id).unwrap().unwrap().status,
            Status::InProgress
        );
        assert!(!e.auto_review_ids.contains(&id));
    }

    #[test]
    fn manual_drag_back_is_not_re_moved() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let id = in_progress_ticket(&mut e);
        e.detect_tick_with(&levels(id, SignalLevel::Active))
            .unwrap();
        e.detect_tick_with(&levels(id, SignalLevel::Idle)).unwrap();
        e.move_ticket(id, Status::InProgress).unwrap();
        e.detect_tick_with(&levels(id, SignalLevel::Idle)).unwrap();
        assert_eq!(
            e.db.get_ticket(id).unwrap().unwrap().status,
            Status::InProgress
        );
    }

    #[test]
    fn never_drags_manually_placed_review_card() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let id = in_progress_ticket(&mut e);
        e.move_ticket(id, Status::Review).unwrap();
        e.detect_tick_with(&levels(id, SignalLevel::Idle)).unwrap();
        e.detect_tick_with(&levels(id, SignalLevel::Active))
            .unwrap();
        assert_eq!(e.db.get_ticket(id).unwrap().unwrap().status, Status::Review);
    }

    #[test]
    fn cleanup_removes_marker_and_state() {
        let tmp = tempfile::tempdir().unwrap();
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.state_dir = tmp.path().to_path_buf();
        std::fs::create_dir_all(&e.state_dir).unwrap();

        let t =
            e.db.create_ticket(e.app.project.id, "t", "", None, Agent::Claude)
                .unwrap();
        e.db.set_ticket_session(t.id, "kamaji-sess", "/wt", "kamaji-sess")
            .unwrap();
        e.reload().unwrap();
        let marker = crate::detect::marker_path(&e.state_dir, "kamaji-sess");
        std::fs::write(&marker, "").unwrap();
        e.auto_review_ids.insert(t.id);
        e.last_level.insert(t.id, SignalLevel::Idle);

        e.cleanup_ticket(t.id).unwrap();

        assert!(!marker.exists());
        assert!(!e.auto_review_ids.contains(&t.id));
        assert!(!e.last_level.contains_key(&t.id));
    }

    #[test]
    fn detect_tick_reads_claude_marker_and_moves_to_review() {
        let tmp = tempfile::tempdir().unwrap();
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.state_dir = tmp.path().to_path_buf();
        std::fs::create_dir_all(&e.state_dir).unwrap();

        let t =
            e.db.create_ticket(e.app.project.id, "t", "", None, Agent::Claude)
                .unwrap();
        e.db.set_ticket_session(t.id, "kamaji-sess", "/wt", "kamaji-sess")
            .unwrap();
        e.db.set_ticket_status(t.id, Status::InProgress).unwrap();
        e.reload().unwrap();

        // No marker yet => Active baseline; no move.
        e.detect_tick().unwrap();
        assert_eq!(
            e.db.get_ticket(t.id).unwrap().unwrap().status,
            Status::InProgress
        );

        // Agent's Stop hook would create the marker => Idle => Review.
        std::fs::write(crate::detect::marker_path(&e.state_dir, "kamaji-sess"), "").unwrap();
        e.detect_tick().unwrap();
        assert_eq!(
            e.db.get_ticket(t.id).unwrap().unwrap().status,
            Status::Review
        );
    }

    /// A `zellij_bar = "compact"` override must produce a compact-bar layout
    /// regardless of the user's zellij config.
    #[test]
    fn start_session_honors_compact_bar_override() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        init_repo(&root);

        let mut e = engine_with_project(root.clone());
        e.config.worktree_base = dir.path().join("wts").to_string_lossy().to_string();
        e.config.zellij_bar = "compact".to_string();
        e.db.create_ticket(e.app.project.id, "Add login", "", Some("go"), Agent::Claude)
            .unwrap();
        e.reload().unwrap();

        let effect = e.move_selected(Status::InProgress).unwrap();
        let Effect::RunSession { layout_path, .. } = effect else {
            panic!("expected RunSession, got {effect:?}");
        };
        let kdl = std::fs::read_to_string(&layout_path).unwrap();
        assert!(
            kdl.contains("compact-bar"),
            "compact override should render compact-bar:\n{kdl}"
        );
        assert!(
            !kdl.contains("status-bar"),
            "compact override must drop the status-bar:\n{kdl}"
        );
    }

    fn enter() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
    }

    /// Enter on a ticket that already has a session attaches to it without
    /// changing its status (the old `a` behavior, now on Enter).
    #[test]
    fn enter_attaches_to_existing_session() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let t =
            e.db.create_ticket(e.app.project.id, "t", "", None, Agent::Claude)
                .unwrap();
        e.db.set_ticket_session(t.id, "sess", "/tmp/wt", "branch")
            .unwrap();
        e.db.set_ticket_status(t.id, Status::Review).unwrap();
        e.reload().unwrap();
        // Move the cursor to the Review column so the ticket is selected.
        e.on_key(key('l')).unwrap();
        e.on_key(key('l')).unwrap();

        assert_eq!(
            e.on_key(enter()).unwrap(),
            Effect::Attach {
                name: "sess".into()
            }
        );
        // Attaching to an existing session leaves the column untouched.
        assert_eq!(
            e.db.get_ticket(t.id).unwrap().unwrap().status,
            Status::Review
        );
    }

    /// Enter on a Todo ticket without a session creates the session/worktree
    /// and moves the ticket to In Progress.
    #[test]
    fn enter_starts_session_for_todo_ticket() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        init_repo(&root);

        let mut e = engine_with_project(root.clone());
        e.config.worktree_base = dir.path().join("wts").to_string_lossy().to_string();
        let t =
            e.db.create_ticket(e.app.project.id, "Add login", "", Some("go"), Agent::Claude)
                .unwrap();
        e.reload().unwrap();
        assert_eq!(e.db.get_ticket(t.id).unwrap().unwrap().status, Status::Todo);

        let effect = e.on_key(enter()).unwrap();
        let name = slug::ticket_name(t.id, "Add login");
        match effect {
            Effect::RunSession { name: n, .. } => assert_eq!(n, name),
            other => panic!("expected RunSession, got {other:?}"),
        }
        let stored = e.db.get_ticket(t.id).unwrap().unwrap();
        assert_eq!(stored.status, Status::InProgress);
        assert_eq!(stored.session_name.as_deref(), Some(name.as_str()));

        e.cleanup_ticket(t.id).unwrap();
    }

    /// `e` opens the edit form for the selected ticket (edit moved off Enter).
    #[test]
    fn e_opens_edit_form() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let t =
            e.db.create_ticket(e.app.project.id, "t", "", None, Agent::Claude)
                .unwrap();
        e.reload().unwrap();

        e.on_key(key('e')).unwrap();
        match &e.app.modal {
            Modal::Form(form) => assert_eq!(form.editing_id, Some(t.id)),
            other => panic!("expected edit form, got {other:?}"),
        }
    }
}
