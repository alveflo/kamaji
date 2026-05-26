use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::app::{App, FormField, Modal, TicketForm};
use crate::config::Config;
use crate::db::Db;
use crate::detect::{self, SignalLevel};
use crate::models::{Agent, Status, Ticket};
use crate::session::{self, Prepared};
use crate::theme::Theme;
use crate::{git, zellij};

/// Side effect the main loop must run by releasing the terminal.
#[derive(Debug, PartialEq)]
pub enum Effect {
    None,
    RunSession {
        name: String,
        layout_path: PathBuf,
    },
    RunSessionBackground {
        name: String,
        layout_path: PathBuf,
        cwd: PathBuf,
    },
    Attach {
        name: String,
    },
    /// Leave the board and return to the project picker.
    SwitchProject,
    /// Download the latest release and replace the running binary.
    SelfUpdate {
        version: String,
    },
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
    /// Where the theme picker persists the chosen theme. Defaults to the real
    /// config path; tests override it.
    pub config_path: std::path::PathBuf,
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
            config_path: crate::config::config_path().unwrap_or_default(),
        }
    }

    pub fn reload(&mut self) -> Result<()> {
        self.app.tickets = self.db.list_tickets(self.app.project.id)?;
        // Rehydrate the auto-review provenance cache from the persisted column so
        // it survives restarts (the move back from Needs attention depends on it).
        self.auto_review_ids = self
            .app
            .tickets
            .iter()
            .filter(|t| t.auto_reviewed)
            .map(|t| t.id)
            .collect();
        self.app.reclamp();
        Ok(())
    }

    /// Build the worktree + layout for a ticket without writing any DB
    /// session/status columns. Shared by foreground and background start.
    fn prepare_session(&mut self, ticket: &Ticket) -> Result<Prepared> {
        session::prepare_session(&self.app.project, &self.config, &self.state_dir, ticket)
    }

    /// Create the worktree + layout for a ticket and return the RunSession effect.
    fn start_session(&mut self, ticket: &Ticket) -> Result<Effect> {
        let p = self.prepare_session(ticket)?;
        session::commit_session(&self.db, ticket.id, &p)?;
        self.reload()?;
        Ok(Effect::RunSession {
            name: p.name,
            layout_path: p.layout_path,
        })
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
        // A manual move overrides auto-review provenance (so a card a human
        // places in Needs attention is not dragged back when its agent resumes).
        self.db.set_ticket_auto_reviewed(ticket.id, false)?;
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
            // Copy out the status and per-project display number so we don't
            // hold an app borrow across the db write.
            let Some((status, number)) = self
                .app
                .tickets
                .iter()
                .find(|t| t.id == id)
                .map(|t| (t.status, t.number))
            else {
                continue;
            };
            let last = self.last_level.get(&id).copied();
            let was_auto = self.auto_review_ids.contains(&id);
            if let Some(target) = detect::decide(last, level, status, was_auto) {
                self.db.set_ticket_status(id, target)?;
                match target {
                    Status::Review => {
                        self.db.set_ticket_auto_reviewed(id, true)?;
                        self.auto_review_ids.insert(id);
                        self.app
                            .set_info(format!("#{number} → Needs attention (agent idle)"));
                    }
                    Status::InProgress => {
                        self.db.set_ticket_auto_reviewed(id, false)?;
                        self.auto_review_ids.remove(&id);
                        self.app
                            .set_info(format!("#{number} → In Progress (agent active)"));
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
        let live: Vec<(i64, Agent, String, bool)> = self
            .app
            .tickets
            .iter()
            .filter(|t| matches!(t.status, Status::InProgress | Status::Review))
            .filter_map(|t| {
                t.session_name
                    .clone()
                    .map(|s| (t.id, t.agent, s, t.instrumented))
            })
            .collect();

        // One session listing per tick, used to drop signals from exited
        // (resurrectable) sessions whose agent is no longer running. `None`
        // (couldn't ask) leaves detection untouched, like reconcile.
        let sessions = zellij::list_sessions();

        let mut out = HashMap::new();
        for (id, agent, session, instrumented) in live {
            // An exited session's agent is gone, so no signal is trustworthy.
            if let Some(list) = &sessions {
                if zellij::session_exited(list, &session) {
                    out.insert(id, SignalLevel::Unknown);
                    continue;
                }
            }
            let level = match agent {
                Agent::Claude => {
                    // The marker only means "active when absent" if kamaji
                    // installed the idle hooks; otherwise we can't tell.
                    if instrumented {
                        detect::marker_level(&detect::marker_path(&self.state_dir, &session))
                    } else {
                        SignalLevel::Unknown
                    }
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

    fn submit_form(&mut self, form: &TicketForm) -> Result<Effect> {
        match form.editing_id {
            Some(id) => {
                self.db
                    .update_ticket_fields(id, &form.title, &form.description)?;
                self.reload()?;
                Ok(Effect::None)
            }
            None => {
                let ticket = self.db.create_ticket(
                    self.app.project.id,
                    &form.title,
                    &form.description,
                    form.prompt_opt().as_deref(),
                    form.agent,
                )?;
                self.reload()?;
                if !form.start_in_background {
                    return Ok(Effect::None);
                }
                // Background start: prepare the session, then commit DB state.
                // On any preparation error, leave the card in Todo with a toast.
                match self.prepare_session(&ticket) {
                    Ok(p) => {
                        session::commit_session(&self.db, ticket.id, &p)?;
                        self.reload()?;
                        Ok(Effect::RunSessionBackground {
                            name: p.name,
                            layout_path: p.layout_path,
                            cwd: p.worktree,
                        })
                    }
                    Err(err) => {
                        self.app
                            .set_error(format!("could not start session: {err}"));
                        Ok(Effect::None)
                    }
                }
            }
        }
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
                            return self.submit_form(&form);
                        } else {
                            self.app.modal = Modal::Form(form);
                            self.app.set_error("Title is required");
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
                    KeyCode::Left | KeyCode::Right if form.field == FormField::Background => {
                        form.toggle_background();
                        self.app.modal = Modal::Form(form);
                    }
                    KeyCode::Char(' ') if form.field == FormField::Background => {
                        form.toggle_background();
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
            Modal::ThemePicker {
                mut selected,
                original,
            } => match key.code {
                KeyCode::Esc => {
                    self.app.theme = Theme::ALL[original]();
                    Ok(Effect::None)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = selected.saturating_sub(1);
                    self.app.theme = Theme::ALL[selected]();
                    self.app.modal = Modal::ThemePicker { selected, original };
                    Ok(Effect::None)
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = (selected + 1).min(Theme::ALL.len() - 1);
                    self.app.theme = Theme::ALL[selected]();
                    self.app.modal = Modal::ThemePicker { selected, original };
                    Ok(Effect::None)
                }
                KeyCode::Enter => {
                    let chosen = self.app.theme;
                    self.config.theme = chosen.name.to_string();
                    match crate::config::save_to(&self.config_path, &self.config) {
                        Ok(()) => {
                            self.app.set_info(format!("theme: {}", chosen.label));
                        }
                        Err(e) => {
                            // Persisting failed: revert live + config state to the original so
                            // what's shown matches what will load next launch.
                            let orig = Theme::ALL[original]();
                            self.app.theme = orig;
                            self.config.theme = orig.name.to_string();
                            self.app.set_error(format!("could not save theme: {e}"));
                        }
                    }
                    Ok(Effect::None)
                }
                _ => {
                    self.app.modal = Modal::ThemePicker { selected, original };
                    Ok(Effect::None)
                }
            },
            Modal::AgentPicker { mut selected } => match key.code {
                KeyCode::Esc => Ok(Effect::None), // close without saving
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = selected.saturating_sub(1);
                    self.app.modal = Modal::AgentPicker { selected };
                    Ok(Effect::None)
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = (selected + 1).min(Agent::all().len() - 1);
                    self.app.modal = Modal::AgentPicker { selected };
                    Ok(Effect::None)
                }
                KeyCode::Enter => {
                    let chosen = Agent::all()[selected];
                    let previous = std::mem::replace(
                        &mut self.config.default_agent,
                        chosen.as_str().to_string(),
                    );
                    match crate::config::save_to(&self.config_path, &self.config) {
                        Ok(()) => self
                            .app
                            .set_info(format!("default agent: {}", chosen.label())),
                        Err(e) => {
                            // Persisting failed: revert so config matches what loads next launch.
                            self.config.default_agent = previous;
                            self.app
                                .set_error(format!("could not save default agent: {e}"));
                        }
                    }
                    Ok(Effect::None)
                }
                _ => {
                    self.app.modal = Modal::AgentPicker { selected };
                    Ok(Effect::None)
                }
            },
        }
    }

    fn on_board_key(&mut self, key: KeyEvent) -> Result<Effect> {
        self.app.status_message = None;

        // While editing the search query, capture input before the board
        // hotkeys so typed characters edit the query (and 'q' doesn't quit).
        if self.app.search.editing {
            match key.code {
                KeyCode::Esc => self.app.search_clear(),
                KeyCode::Enter => self.app.search_commit(),
                KeyCode::Backspace => self.app.search_backspace(),
                KeyCode::Char(c) => self.app.search_push(c),
                _ => {}
            }
            return Ok(Effect::None);
        }

        match key.code {
            KeyCode::Char('q') => self.app.should_quit = true,
            KeyCode::Char('/') => self.app.search_start(),
            KeyCode::Esc if !self.app.search.is_empty() => self.app.search_clear(),
            KeyCode::Char('p') => return Ok(Effect::SwitchProject),
            KeyCode::Char('u') => {
                if let Some(version) = self.app.update.clone() {
                    return Ok(Effect::SelfUpdate { version });
                }
            }
            KeyCode::Char('?') => self.app.modal = Modal::Help,
            KeyCode::Char('t') => {
                let idx = Theme::index_of(&self.config.theme);
                self.app.modal = Modal::ThemePicker {
                    selected: idx,
                    original: idx,
                };
            }
            KeyCode::Char('a') => {
                self.app.modal = Modal::AgentPicker {
                    selected: self.config.default_agent().index(),
                };
            }
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
    use crate::slug;
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
        // The toast names the column as the user sees it ("Needs attention"),
        // and an automatic status transition is informational, not an error.
        let msg = e.app.status_message.as_ref().unwrap();
        assert!(msg.text.contains("Needs attention"));
        assert_eq!(msg.kind, crate::app::StatusKind::Info);
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

    /// The move back from Needs attention must survive a kamaji restart, which
    /// wipes all in-memory detection state. Provenance is persisted on the
    /// ticket, so after rehydrating from the DB the resumed agent still returns
    /// to In Progress.
    #[test]
    fn move_back_survives_lost_in_memory_provenance() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let id = in_progress_ticket(&mut e);
        // Auto-move to Needs attention (Active -> Idle).
        e.detect_tick_with(&levels(id, SignalLevel::Active))
            .unwrap();
        e.detect_tick_with(&levels(id, SignalLevel::Idle)).unwrap();
        assert_eq!(e.db.get_ticket(id).unwrap().unwrap().status, Status::Review);
        // Provenance is persisted, not just held in memory.
        assert!(e.db.get_ticket(id).unwrap().unwrap().auto_reviewed);

        // Simulate a restart: drop all in-memory detection state, reload from DB.
        e.auto_review_ids.clear();
        e.last_level.clear();
        e.reload().unwrap();

        // Resume the agent: re-baseline as Idle, then it becomes Active.
        e.detect_tick_with(&levels(id, SignalLevel::Idle)).unwrap();
        e.detect_tick_with(&levels(id, SignalLevel::Active))
            .unwrap();
        assert_eq!(
            e.db.get_ticket(id).unwrap().unwrap().status,
            Status::InProgress
        );
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
        // The session carries the idle hooks, so its marker signal is trusted.
        e.db.set_ticket_instrumented(t.id, true).unwrap();
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

    /// A Claude session started without the idle hooks (e.g. a session from
    /// before instrumentation existed) has no trustworthy marker: an absent
    /// marker must NOT be read as "active". Such a session never auto-moves and
    /// never reports an Active level (so its bullet is never shown as working).
    #[test]
    fn non_instrumented_claude_signal_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.state_dir = tmp.path().to_path_buf();
        std::fs::create_dir_all(&e.state_dir).unwrap();

        let t =
            e.db.create_ticket(e.app.project.id, "t", "", None, Agent::Claude)
                .unwrap();
        e.db.set_ticket_session(t.id, "kamaji-noinstr", "/wt", "kamaji-noinstr")
            .unwrap();
        e.db.set_ticket_status(t.id, Status::InProgress).unwrap();
        // instrumented stays false (no hooks were injected).
        e.reload().unwrap();

        // Baseline (marker absent), then the agent "stops" (marker present).
        e.detect_tick().unwrap();
        std::fs::write(
            crate::detect::marker_path(&e.state_dir, "kamaji-noinstr"),
            "",
        )
        .unwrap();
        e.detect_tick().unwrap();

        // An instrumented session would now be in Needs attention; this one must
        // stay In Progress, and must not have been recorded as Active/Idle.
        assert_eq!(
            e.db.get_ticket(t.id).unwrap().unwrap().status,
            Status::InProgress
        );
        assert!(!matches!(
            e.last_level.get(&t.id),
            Some(SignalLevel::Active) | Some(SignalLevel::Idle)
        ));
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

    /// Submitting the create form with the background toggle on (in a real git
    /// repo) prepares a session and returns RunSessionBackground; the ticket is
    /// moved to In Progress with a recorded session.
    #[test]
    fn create_with_background_toggle_starts_session() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        init_repo(&root);

        let mut e = engine_with_project(root.clone());
        e.config.worktree_base = dir.path().join("wts").to_string_lossy().to_string();
        e.state_dir = dir.path().join("state");

        e.on_key(key('c')).unwrap();
        for ch in "Add login".chars() {
            e.on_key(key(ch)).unwrap();
        }
        let effect = e
            .on_key(ratatui::crossterm::event::KeyEvent::new(
                ratatui::crossterm::event::KeyCode::Enter,
                ratatui::crossterm::event::KeyModifiers::NONE,
            ))
            .unwrap();

        let ticket_id = e.app.tickets[0].id;
        let name = slug::ticket_name(ticket_id, "Add login");
        match effect {
            Effect::RunSessionBackground {
                name: n,
                layout_path,
                cwd,
            } => {
                assert_eq!(n, name);
                assert!(layout_path.exists());
                assert!(cwd.ends_with(&name));
            }
            other => panic!("expected RunSessionBackground, got {other:?}"),
        }
        let t = &e.app.tickets[0];
        assert_eq!(t.status, Status::InProgress);
        assert_eq!(t.session_name.as_deref(), Some(name.as_str()));

        e.cleanup_ticket(ticket_id).unwrap();
    }

    /// With the toggle off, creation is the classic Todo card with no session.
    #[test]
    fn create_without_background_toggle_makes_todo_card() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        init_repo(&root);
        let mut e = engine_with_project(root.clone());
        e.config.worktree_base = dir.path().join("wts").to_string_lossy().to_string();

        e.on_key(key('c')).unwrap();
        for ch in "Plan only".chars() {
            e.on_key(key(ch)).unwrap();
        }
        // Tab to Background and turn it off.
        for _ in 0..4 {
            e.on_key(ratatui::crossterm::event::KeyEvent::new(
                ratatui::crossterm::event::KeyCode::Tab,
                ratatui::crossterm::event::KeyModifiers::NONE,
            ))
            .unwrap();
        }
        e.on_key(key(' ')).unwrap();
        let effect = e
            .on_key(ratatui::crossterm::event::KeyEvent::new(
                ratatui::crossterm::event::KeyCode::Enter,
                ratatui::crossterm::event::KeyModifiers::NONE,
            ))
            .unwrap();

        assert_eq!(effect, Effect::None);
        assert_eq!(e.app.tickets[0].status, Status::Todo);
        assert_eq!(e.app.tickets[0].session_name, None);
    }

    /// Toggle on but the project root is not a git repo: the ticket is still
    /// created, left in Todo, with an error toast (graceful failure).
    #[test]
    fn create_with_background_toggle_in_non_git_root_stays_todo() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.on_key(key('c')).unwrap();
        for ch in "No repo".chars() {
            e.on_key(key(ch)).unwrap();
        }
        let effect = e
            .on_key(ratatui::crossterm::event::KeyEvent::new(
                ratatui::crossterm::event::KeyCode::Enter,
                ratatui::crossterm::event::KeyModifiers::NONE,
            ))
            .unwrap();

        assert_eq!(effect, Effect::None);
        assert_eq!(e.app.tickets.len(), 1);
        assert_eq!(e.app.tickets[0].status, Status::Todo);
        assert_eq!(e.app.tickets[0].session_name, None);
        let msg = e
            .app
            .status_message
            .as_ref()
            .expect("an error toast is shown");
        assert_eq!(msg.kind, crate::app::StatusKind::Error);
    }

    #[test]
    fn t_opens_theme_picker_at_current_theme() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.config.theme = "nord".to_string();
        e.app.theme = crate::theme::Theme::by_name("nord");
        e.on_key(key('t')).unwrap();
        match e.app.modal {
            Modal::ThemePicker { selected, original } => {
                let idx = crate::theme::Theme::index_of("nord");
                assert_eq!(selected, idx);
                assert_eq!(original, idx);
            }
            ref other => panic!("expected ThemePicker, got {other:?}"),
        }
    }

    #[test]
    fn picker_down_previews_next_theme() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.app.modal = Modal::ThemePicker {
            selected: 0,
            original: 0,
        };
        e.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(e.app.theme.name, crate::theme::Theme::ALL[1]().name);
        match e.app.modal {
            Modal::ThemePicker { selected, .. } => assert_eq!(selected, 1),
            ref other => panic!("expected ThemePicker, got {other:?}"),
        }
    }

    #[test]
    fn picker_enter_persists_theme_to_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.config_path = dir.path().join("config.toml");
        let nord = crate::theme::Theme::index_of("nord");
        e.app.modal = Modal::ThemePicker {
            selected: nord,
            original: 0,
        };
        e.app.theme = crate::theme::Theme::ALL[nord]();
        e.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(e.app.modal, Modal::None));
        assert_eq!(e.config.theme, "nord");
        let saved = crate::config::load_from(&e.config_path).unwrap();
        assert_eq!(saved.theme, "nord");
    }

    #[test]
    fn picker_esc_reverts_to_original_theme() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let nord = crate::theme::Theme::index_of("nord");
        e.app.modal = Modal::ThemePicker {
            selected: nord,
            original: 0,
        };
        e.app.theme = crate::theme::Theme::ALL[nord]();
        e.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(e.app.modal, Modal::None));
        assert_eq!(e.app.theme.name, crate::theme::Theme::ALL[0]().name);
    }

    #[test]
    fn picker_up_clamps_at_first_theme() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.app.modal = Modal::ThemePicker {
            selected: 0,
            original: 0,
        };
        e.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
            .unwrap();
        match e.app.modal {
            Modal::ThemePicker { selected, .. } => {
                assert_eq!(selected, 0, "up at index 0 stays at 0")
            }
            ref other => panic!("expected ThemePicker, got {other:?}"),
        }
        assert_eq!(e.app.theme.name, crate::theme::Theme::ALL[0]().name);
    }

    #[test]
    fn picker_enter_persists_even_with_no_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.config_path = dir.path().join("config.toml");
        // Open at the current (default) theme and confirm without moving.
        e.app.theme = crate::theme::Theme::ALL[0]();
        e.config.theme = crate::theme::Theme::ALL[0]().name.to_string();
        e.app.modal = Modal::ThemePicker {
            selected: 0,
            original: 0,
        };
        e.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(e.app.modal, Modal::None));
        let saved = crate::config::load_from(&e.config_path).unwrap();
        assert_eq!(saved.theme, crate::theme::Theme::ALL[0]().name);
    }

    #[test]
    fn a_opens_agent_picker_at_current_default() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.config.default_agent = "codex".to_string();
        e.on_key(key('a')).unwrap();
        match e.app.modal {
            Modal::AgentPicker { selected } => {
                assert_eq!(selected, Agent::Codex.index());
            }
            ref other => panic!("expected AgentPicker, got {other:?}"),
        }
    }

    #[test]
    fn agent_picker_down_moves_selection() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.app.modal = Modal::AgentPicker { selected: 0 };
        e.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
            .unwrap();
        match e.app.modal {
            Modal::AgentPicker { selected } => assert_eq!(selected, 1),
            ref other => panic!("expected AgentPicker, got {other:?}"),
        }
    }

    #[test]
    fn agent_picker_down_clamps_at_last() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let last = Agent::all().len() - 1;
        e.app.modal = Modal::AgentPicker { selected: last };
        e.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
            .unwrap();
        match e.app.modal {
            Modal::AgentPicker { selected } => assert_eq!(selected, last),
            ref other => panic!("expected AgentPicker, got {other:?}"),
        }
    }

    #[test]
    fn agent_picker_enter_persists_default_to_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.config_path = dir.path().join("config.toml");
        e.app.modal = Modal::AgentPicker {
            selected: Agent::Copilot.index(),
        };
        e.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(e.app.modal, Modal::None));
        assert_eq!(e.config.default_agent, "copilot");
        let saved = crate::config::load_from(&e.config_path).unwrap();
        assert_eq!(saved.default_agent, "copilot");
        assert_eq!(saved.default_agent().index(), Agent::Copilot.index());
    }

    #[test]
    fn agent_picker_esc_leaves_config_unchanged() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.config.default_agent = "claude".to_string();
        e.app.modal = Modal::AgentPicker {
            selected: Agent::Codex.index(),
        };
        e.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(e.app.modal, Modal::None));
        assert_eq!(e.config.default_agent, "claude");
    }

    #[test]
    fn space_toggles_background_field_in_form() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.on_key(key('c')).unwrap();
        // Walk to the Background field via Tab (Title→Desc→Prompt→Agent→Background).
        for _ in 0..4 {
            e.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
                .unwrap();
        }
        match &e.app.modal {
            Modal::Form(f) => assert_eq!(f.field, FormField::Background),
            other => panic!("expected form, got {other:?}"),
        }
        // Space flips it off.
        e.on_key(key(' ')).unwrap();
        match &e.app.modal {
            Modal::Form(f) => assert!(!f.start_in_background),
            other => panic!("expected form, got {other:?}"),
        }
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

    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    #[test]
    fn slash_enters_search_and_typing_does_not_trigger_hotkeys() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.on_key(key('/')).unwrap();
        assert!(e.app.search.editing, "/ starts search editing");
        // 'q' is captured as query text, not treated as quit.
        e.on_key(key('q')).unwrap();
        assert!(
            !e.app.should_quit,
            "q is typed into the query while editing"
        );
        assert_eq!(e.app.search.query, "q");
    }

    #[test]
    fn enter_commits_search_filter() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.on_key(key('/')).unwrap();
        for c in "log".chars() {
            e.on_key(key(c)).unwrap();
        }
        e.on_key(enter()).unwrap();
        assert!(!e.app.search.editing, "Enter commits and stops editing");
        assert_eq!(
            e.app.search.query, "log",
            "the filter persists after commit"
        );
    }

    #[test]
    fn esc_clears_query_while_editing_then_clears_filter_when_applied() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        // While editing, Esc clears the query and exits search.
        e.on_key(key('/')).unwrap();
        for c in "log".chars() {
            e.on_key(key(c)).unwrap();
        }
        e.on_key(esc()).unwrap();
        assert!(!e.app.search.editing);
        assert!(e.app.search.query.is_empty());

        // Apply and commit a filter, then Esc on the board clears it.
        e.on_key(key('/')).unwrap();
        e.on_key(key('x')).unwrap();
        e.on_key(enter()).unwrap();
        assert_eq!(e.app.search.query, "x");
        e.on_key(esc()).unwrap();
        assert!(
            e.app.search.query.is_empty(),
            "Esc clears the applied filter"
        );
    }

    #[test]
    fn filter_does_not_stop_detection() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let id = in_progress_ticket(&mut e); // title "t", In Progress, has a session
                                             // Apply a filter that hides the in-progress card.
        e.app.search.query = "zzz".into();
        assert!(
            e.app.column_tickets(Status::InProgress).is_empty(),
            "the filter hides the in-progress card"
        );
        // Detection still sees the hidden ticket and auto-moves it on idle.
        e.detect_tick_with(&levels(id, SignalLevel::Active))
            .unwrap();
        e.detect_tick_with(&levels(id, SignalLevel::Idle)).unwrap();
        assert_eq!(
            e.db.get_ticket(id).unwrap().unwrap().status,
            Status::Review,
            "a hidden ticket is still auto-moved to Needs attention"
        );
    }

    #[test]
    fn u_triggers_self_update_when_update_available() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.app.update = Some("0.9.0".into());
        assert_eq!(
            e.on_key(key('u')).unwrap(),
            Effect::SelfUpdate {
                version: "0.9.0".into()
            }
        );
    }

    #[test]
    fn u_does_nothing_without_an_update() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        assert_eq!(e.on_key(key('u')).unwrap(), Effect::None);
    }
}
