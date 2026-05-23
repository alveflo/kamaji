use anyhow::{bail, Result};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use std::path::PathBuf;

use crate::app::{App, FormField, Modal, TicketForm};
use crate::config::Config;
use crate::db::Db;
use crate::models::{Status, Ticket};
use crate::{agent, git, layout, slug, zellij};

/// Side effect the main loop must run by releasing the terminal.
#[derive(Debug, PartialEq)]
pub enum Effect {
    None,
    RunSession { name: String, layout_path: PathBuf },
    Attach { name: String },
}

pub struct Engine {
    pub db: Db,
    pub config: Config,
    pub app: App,
}

impl Engine {
    pub fn new(db: Db, config: Config, app: App) -> Self {
        Engine { db, config, app }
    }

    pub fn reload(&mut self) -> Result<()> {
        self.app.tickets = self.db.list_tickets(self.app.project.id)?;
        self.app.reclamp();
        Ok(())
    }

    fn layout_file(&self, name: &str, contents: &str) -> Result<PathBuf> {
        let dir = std::env::temp_dir().join("kamaji-layouts");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{name}.kdl"));
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
        let kdl = layout::render_layout(&worktree.to_string_lossy(), &argv);
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

    /// Terminate session + remove worktree + delete branch for a ticket.
    pub fn cleanup_ticket(&mut self, ticket_id: i64) -> Result<()> {
        if let Some(t) = self.db.get_ticket(ticket_id)? {
            if let Some(name) = &t.session_name {
                zellij::terminate_session(name);
            }
            let root = &self.app.project.root_dir;
            if let Some(wt) = &t.worktree_path {
                let _ = git::remove_worktree(root, wt);
            }
            if let Some(b) = &t.branch {
                let _ = git::delete_branch(root, b);
            }
        }
        Ok(())
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
            KeyCode::Char('?') => self.app.modal = Modal::Help,
            KeyCode::Left | KeyCode::Char('h') => self.app.left(),
            KeyCode::Right | KeyCode::Char('l') => self.app.right(),
            KeyCode::Up | KeyCode::Char('k') => self.app.up(),
            KeyCode::Down | KeyCode::Char('j') => self.app.down(),
            KeyCode::Char('c') => {
                self.app.modal = Modal::Form(TicketForm::new_create(self.config.default_agent()));
            }
            KeyCode::Char('o') | KeyCode::Enter => {
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
            KeyCode::Char('a') => {
                if let Some(t) = self.app.selected_ticket() {
                    if let Some(name) = t.session_name.clone() {
                        return Ok(Effect::Attach { name });
                    }
                    self.app.status_message = Some("No session for this ticket yet".into());
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
    use crate::models::Agent;
    use ratatui::crossterm::event::{KeyEvent, KeyModifiers};

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn engine_with_project(root: std::path::PathBuf) -> Engine {
        let db = Db::open_in_memory().unwrap();
        let project = db.create_project("p", &root, None).unwrap();
        let app = App::new(project, vec![]);
        Engine::new(db, Config::default(), app)
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
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
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

        let mut e = engine_with_project(root.clone());
        // Point worktrees somewhere isolated under tempdir.
        e.config.worktree_base = dir.path().join("wts").to_string_lossy().to_string();
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
            }
            other => panic!("expected RunSession, got {other:?}"),
        }
        let stored = e.db.get_ticket(t.id).unwrap().unwrap();
        assert_eq!(stored.status, Status::InProgress);
        assert_eq!(stored.session_name.as_deref(), Some(name.as_str()));
        assert!(dir.path().join("wts").join(&name).join("f").exists());
    }
}
