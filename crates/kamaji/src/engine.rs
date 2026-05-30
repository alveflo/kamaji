use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, FormField, Modal, TicketForm, WorktreeForm};
use crate::client::{ClientError, DaemonClient};
use crate::dir_select::{self, RootCheck};
use crate::theme::Theme;
use kamaji_core::config::Config;
use kamaji_core::models::{Agent, Status, Ticket};

/// Side effect the main loop must run by releasing the terminal.
#[derive(Debug, PartialEq)]
pub enum Effect {
    None,
    /// Leave the board and return to the project picker.
    SwitchProject,
    /// Download the latest release and replace the running binary.
    SelfUpdate {
        version: String,
    },
    /// Attach to an existing zellij session by name. The daemon owns session
    /// creation (start/main-session); the TUI only ever releases the terminal
    /// to attach to a session the daemon has already started.
    Attach {
        name: String,
    },
}

pub struct Engine {
    /// The single write path and read source of truth: the kamajid daemon over
    /// HTTP. Every mutation goes through this client; the daemon owns the DB,
    /// git, zellij, and auto-review polling.
    pub client: DaemonClient,
    pub config: Config,
    pub app: App,
    /// Set when a client call (or the SSE stream) reports the daemon is
    /// unreachable. The mutation handlers convert client errors into toasts
    /// internally, so this flag is how a connection-loss signal reaches the main
    /// loop, which drains it via `take_connection_lost` and attempts a reconnect.
    connection_lost: bool,
}

impl Engine {
    pub fn new(client: DaemonClient, config: Config, app: App) -> Self {
        Engine {
            client,
            config,
            app,
            connection_lost: false,
        }
    }

    /// Record a client error: if it means the daemon is unreachable, raise the
    /// `connection_lost` flag so the main loop attempts a reconnect. Domain
    /// errors (BadRequest/NotFound/…) leave the flag untouched.
    fn note_client_error(&mut self, e: &ClientError) {
        if crate::client::is_connection_lost(e) {
            self.connection_lost = true;
        }
    }

    /// Directly flag a connection loss (used when the SSE stream reports it is
    /// disconnected, a secondary trigger independent of the command path).
    pub fn flag_connection_lost(&mut self) {
        self.connection_lost = true;
    }

    /// Read-and-clear the connection-lost flag. The main loop calls this after
    /// handling input / draining SSE; a `true` return means "attempt reconnect".
    pub fn take_connection_lost(&mut self) -> bool {
        std::mem::take(&mut self.connection_lost)
    }

    /// Re-fetch the current project's tickets from the daemon and re-clamp the
    /// UI. Used after every mutation, after SSE deltas, and after attach. The
    /// daemon is the single source of truth.
    pub fn refresh_from_client(&mut self) -> anyhow::Result<()> {
        match self.client.list_tickets(self.app.project.id) {
            Ok(tickets) => self.app.tickets = tickets,
            Err(e) => {
                self.note_client_error(&e);
                self.app
                    .set_error(format!("could not refresh board: {e:?}"));
            }
        }
        self.app.reclamp();
        self.app.prune_selection();
        Ok(())
    }

    /// Apply one SSE delta to the in-memory board for the CURRENT project. Events
    /// for other projects are ignored. Id-only events that need the full row
    /// (`session.*`) re-fetch via the client. Mirrors today's handle_poll_events
    /// toast for an auto-review move.
    pub fn apply_sse_event(&mut self, ev: kamaji_core::events::Event) {
        use kamaji_core::events::Event;
        let pid = self.app.project.id;
        match ev {
            Event::TicketCreated(t) | Event::TicketUpdated(t) => {
                if t.project_id != pid {
                    return;
                }
                match self.app.tickets.iter_mut().find(|x| x.id == t.id) {
                    Some(slot) => *slot = t,
                    None => self.app.tickets.push(t),
                }
            }
            Event::TicketMoved { id, to, .. } => {
                if let Some(slot) = self.app.tickets.iter_mut().find(|x| x.id == id) {
                    slot.status = to;
                    match to {
                        Status::Review => self
                            .app
                            .set_info(format!("#{id} → Needs attention (agent idle)")),
                        Status::InProgress => self
                            .app
                            .set_info(format!("#{id} → In Progress (agent active)")),
                        _ => {}
                    }
                }
            }
            Event::TicketDeleted { id } => {
                self.app.tickets.retain(|x| x.id != id);
            }
            Event::SessionStarted { ticket_id, .. } | Event::SessionExited { ticket_id, .. } => {
                self.refetch_ticket(ticket_id);
            }
            Event::SessionIdle { .. } => { /* informational; the ticket.moved carries the column */
            }
        }
        self.app.reclamp();
        self.app.prune_selection();
    }

    /// Splice a freshly-fetched ticket row (after a session.* event). Best-effort:
    /// a failed fetch leaves the stale row until the next refresh.
    fn refetch_ticket(&mut self, id: i64) {
        if let Ok(t) = self.client.get_ticket(id) {
            if t.project_id != self.app.project.id {
                return;
            }
            match self.app.tickets.iter_mut().find(|x| x.id == id) {
                Some(slot) => *slot = t,
                None => self.app.tickets.push(t),
            }
        }
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

    /// Route a column move through the daemon. Moving to In Progress is a
    /// "start/attach": a ticket that already carries a session is moved and
    /// attached; one without a session is `start`ed by the daemon (which owns
    /// the worktree precondition — a missing worktree location surfaces as a
    /// `BadRequest` toast) and then attached to the returned session name.
    fn apply_move(&mut self, ticket: Ticket, target: Status) -> Result<Effect> {
        if target == Status::InProgress {
            return match ticket.session_name.clone() {
                Some(name) => {
                    if let Err(e) = self.client.move_ticket(ticket.id, Status::InProgress) {
                        self.note_client_error(&e);
                        self.app.set_error(format!("could not move: {e:?}"));
                        return Ok(Effect::None);
                    }
                    self.refresh_from_client()?;
                    Ok(Effect::Attach { name })
                }
                None => match self.client.start_ticket(ticket.id) {
                    Ok(t) => {
                        self.refresh_from_client()?;
                        match t.session_name {
                            Some(name) => Ok(Effect::Attach { name }),
                            None => Ok(Effect::None),
                        }
                    }
                    Err(ClientError::BadRequest(m)) => {
                        self.app.set_error(m);
                        Ok(Effect::None)
                    }
                    Err(e) => {
                        self.note_client_error(&e);
                        self.app.set_error(format!("could not start: {e:?}"));
                        Ok(Effect::None)
                    }
                },
            };
        }
        match self.client.move_ticket(ticket.id, target) {
            Ok(_) => self.refresh_from_client()?,
            Err(e) => {
                self.note_client_error(&e);
                self.app.set_error(format!("could not move: {e:?}"));
            }
        }
        Ok(Effect::None)
    }

    fn submit_form(&mut self, form: &TicketForm) -> Result<Effect> {
        match form.editing_id {
            Some(id) => {
                match self.client.update_ticket(
                    id,
                    &form.title,
                    Some(&form.description),
                    form.prompt_opt().as_deref(),
                    Some(form.agent),
                ) {
                    Ok(_) => self.refresh_from_client()?,
                    Err(e) => {
                        self.note_client_error(&e);
                        self.app.set_error(format!("could not save ticket: {e:?}"));
                    }
                }
                Ok(Effect::None)
            }
            None => {
                let created = match self.client.create_ticket(
                    self.app.project.id,
                    &form.title,
                    &form.description,
                    form.prompt_opt().as_deref(),
                    form.agent,
                ) {
                    Ok(t) => t,
                    Err(e) => {
                        self.note_client_error(&e);
                        self.app
                            .set_error(format!("could not create ticket: {e:?}"));
                        return Ok(Effect::None);
                    }
                };
                self.refresh_from_client()?;
                if !form.start_in_background {
                    return Ok(Effect::None);
                }
                match self.client.start_ticket(created.id) {
                    Ok(t) => {
                        self.refresh_from_client()?;
                        self.app.set_info(format!(
                            "Started '{}' in the background",
                            t.session_name.as_deref().unwrap_or("")
                        ));
                    }
                    Err(ClientError::BadRequest(m)) => self.app.set_error(m),
                    Err(e) => {
                        self.note_client_error(&e);
                        self.app
                            .set_error(format!("could not start session: {e:?}"));
                    }
                }
                Ok(Effect::None)
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
                        self.app.modal = Modal::ConfirmDone {
                            ticket_ids: vec![ticket_id],
                        };
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
            Modal::ConfirmDone { ticket_ids } => match key.code {
                KeyCode::Char('y') => {
                    // 'y' closes with cleanup (terminate session, remove worktree).
                    for id in &ticket_ids {
                        if let Err(e) = self.client.done_ticket(*id, true) {
                            self.note_client_error(&e);
                            self.app
                                .set_error(format!("could not complete #{id}: {e:?}"));
                        }
                    }
                    self.app.clear_selection();
                    self.refresh_from_client()?;
                    Ok(Effect::None)
                }
                KeyCode::Char('n') => {
                    // 'n' marks done but leaves the session/worktree in place.
                    for id in &ticket_ids {
                        if let Err(e) = self.client.done_ticket(*id, false) {
                            self.note_client_error(&e);
                            self.app
                                .set_error(format!("could not complete #{id}: {e:?}"));
                        }
                    }
                    self.app.clear_selection();
                    self.refresh_from_client()?;
                    Ok(Effect::None)
                }
                _ => {
                    // Esc cancels: restore the modal-less board but keep the
                    // selection so the user can retry.
                    Ok(Effect::None)
                }
            },
            Modal::ConfirmDelete { ticket_id } => match key.code {
                KeyCode::Char('y') => {
                    if let Err(e) = self.client.delete_ticket(ticket_id) {
                        self.note_client_error(&e);
                        self.app
                            .set_error(format!("could not delete #{ticket_id}: {e:?}"));
                    }
                    self.refresh_from_client()?;
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
                    match self.client.update_config(Some(chosen.name), None, None) {
                        Ok(cfg) => {
                            self.config = cfg;
                            self.app.set_info(format!("theme: {}", chosen.label));
                        }
                        Err(e) => {
                            // Persisting failed: revert the live theme to the original so
                            // what's shown matches what will load next launch.
                            self.note_client_error(&e);
                            self.app.theme = Theme::ALL[original]();
                            self.app.set_error(format!("could not save theme: {e:?}"));
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
                    match self.client.update_config(None, Some(chosen.as_str()), None) {
                        Ok(cfg) => {
                            self.config = cfg;
                            self.app
                                .set_info(format!("default agent: {}", chosen.label()));
                        }
                        Err(e) => {
                            self.note_client_error(&e);
                            self.app
                                .set_error(format!("could not save default agent: {e:?}"));
                        }
                    }
                    Ok(Effect::None)
                }
                _ => {
                    self.app.modal = Modal::AgentPicker { selected };
                    Ok(Effect::None)
                }
            },
            Modal::WorktreeLocation(mut form) => {
                match key.code {
                    KeyCode::Esc => {
                        // Esc dismisses a pending create-confirm first; only a
                        // second Esc (nothing pending) closes the modal.
                        if !form.dir.escape() {
                            self.app.modal = Modal::WorktreeLocation(form);
                        }
                    }
                    KeyCode::Tab => {
                        if !form.dir.suggestions.is_empty() {
                            form.dir.accept_suggestion();
                        }
                        self.app.modal = Modal::WorktreeLocation(form);
                    }
                    KeyCode::Up => {
                        form.dir.move_suggestion(-1);
                        self.app.modal = Modal::WorktreeLocation(form);
                    }
                    KeyCode::Down => {
                        form.dir.move_suggestion(1);
                        self.app.modal = Modal::WorktreeLocation(form);
                    }
                    KeyCode::Backspace => {
                        form.dir.backspace();
                        self.app.modal = Modal::WorktreeLocation(form);
                    }
                    KeyCode::Enter => {
                        if form.dir.pending_create.is_some() {
                            // Second Enter: create the missing directory, then save.
                            match form.dir.confirm_create() {
                                Ok(Some(path)) => self.save_worktree_location(&path),
                                Ok(None) => self.app.modal = Modal::WorktreeLocation(form),
                                Err(e) => {
                                    form.error = Some(format!("Couldn't create directory: {e}"));
                                    self.app.modal = Modal::WorktreeLocation(form);
                                }
                            }
                        } else {
                            match dir_select::check_root(form.dir.resolved()) {
                                RootCheck::Ready(path) => self.save_worktree_location(&path),
                                RootCheck::NeedsConfirm(path) => {
                                    form.error = None;
                                    form.dir.pending_create = Some(path);
                                    self.app.modal = Modal::WorktreeLocation(form);
                                }
                                RootCheck::Invalid(msg) => {
                                    form.error = Some(msg);
                                    self.app.modal = Modal::WorktreeLocation(form);
                                }
                            }
                        }
                    }
                    KeyCode::Char(c) => {
                        form.dir.input_char(c);
                        self.app.modal = Modal::WorktreeLocation(form);
                    }
                    _ => self.app.modal = Modal::WorktreeLocation(form),
                }
                Ok(Effect::None)
            }
        }
    }

    /// Persist a chosen worktree location through the daemon and toast the
    /// result. The daemon owns the config file; on success its returned config
    /// becomes our in-memory state, on failure the live config is untouched.
    fn save_worktree_location(&mut self, path: &std::path::Path) {
        match self
            .client
            .update_config(None, None, Some(&path.to_string_lossy()))
        {
            Ok(cfg) => {
                self.config = cfg;
                self.app.set_info(format!(
                    "worktree location: {}",
                    dir_select::contract_home(path)
                ));
            }
            Err(e) => {
                self.note_client_error(&e);
                self.app
                    .set_error(format!("could not save worktree location: {e:?}"));
            }
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
            // Esc clears the multi-selection first, then (on a second press) the
            // active search filter.
            KeyCode::Esc if !self.app.selected_ids.is_empty() => self.app.clear_selection(),
            KeyCode::Esc if !self.app.search.is_empty() => self.app.search_clear(),
            // Space toggles the focused card in/out of the multi-select set.
            KeyCode::Char(' ') => self.app.toggle_selected(),
            // Shift-D closes the selected tickets (or the focused one when the
            // selection is empty) by moving them to Done after a confirm.
            KeyCode::Char('D') => {
                let ids: Vec<i64> = if self.app.selected_ids.is_empty() {
                    self.app
                        .selected_ticket()
                        .map(|t| t.id)
                        .into_iter()
                        .collect()
                } else {
                    self.app.selected_ids.iter().copied().collect()
                };
                if !ids.is_empty() {
                    self.app.modal = Modal::ConfirmDone { ticket_ids: ids };
                }
            }
            KeyCode::Char('p') => return Ok(Effect::SwitchProject),
            // Open the project's main session (not tied to any ticket): the
            // daemon attaches if it's already running, otherwise starts it, and
            // returns the session name for the TUI to attach to.
            KeyCode::Char('s') => {
                return match self.client.main_session(self.app.project.id) {
                    Ok(name) => Ok(Effect::Attach { name }),
                    Err(e) => {
                        self.note_client_error(&e);
                        self.app
                            .set_error(format!("could not open main session: {e:?}"));
                        Ok(Effect::None)
                    }
                };
            }
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
            KeyCode::Char('w') => {
                self.app.modal = Modal::WorktreeLocation(WorktreeForm::new(
                    self.config.worktree_base.as_deref(),
                ));
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
            // Enter "enters" the ticket: this is the same as moving it to In
            // Progress — attach to its session if one exists, otherwise have the
            // daemon start one (creating the worktree + session) and attach.
            KeyCode::Enter => {
                if let Some(t) = self.app.selected_ticket().cloned() {
                    return self.apply_move(t, Status::InProgress);
                }
            }
            _ => {}
        }
        Ok(Effect::None)
    }
}

// ── What moved to the daemon (no longer tested here) ─────────────────────────
// The orchestration these tests used to cover now lives in the daemon, so the
// coverage moved with it:
//   - Auto-review polling / idle→Needs-attention / resume→In-Progress /
//     manual-drag provenance / non-instrumented signal handling
//     → `kamaji-core` poll tests + `crates/kamajid/tests/api.rs`.
//   - Worktree creation on start, the worktree-location precondition,
//     compact-bar layout, background-start, session resume, and cleanup
//     (terminate session + remove worktree + clear columns)
//     → `crates/kamajid/tests/api.rs` (the daemon owns git/zellij now).
//   - `main_session` start-vs-attach selection → daemon.
// What remains here: keymap → modal/Effect decisions and the thin client wiring
// in the mutation handlers, exercised against the shared in-memory test daemon.

#[cfg(test)]
mod tests {
    use super::*;
    use kamaji_core::events::Event as CoreEvent;
    use kamaji_core::models::Agent;
    use ratatui::crossterm::event::{KeyEvent, KeyModifiers};

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// Connect a `DaemonClient` to a single shared in-process kamajid spawned
    /// once for the whole test binary. Every `engine_with_project` creates its
    /// own project on that daemon, so tickets are isolated per test.
    fn test_client() -> DaemonClient {
        use crate::test_support::spawn_test_daemon;
        use std::sync::OnceLock;
        static BASE: OnceLock<String> = OnceLock::new();
        let base = BASE.get_or_init(spawn_test_daemon).clone();
        DaemonClient::connect(base).unwrap()
    }

    /// An engine wired to the shared test daemon, with a fresh project created on
    /// it so the board starts empty and isolated from other tests.
    fn engine_with_project(root: std::path::PathBuf) -> Engine {
        let client = test_client();
        let project = client.create_project("p", &root, None).unwrap();
        let app = App::new(project, vec![]);
        Engine::new(client, Config::default(), app)
    }

    /// An engine wired to a *dedicated* in-process daemon (not the shared one),
    /// for the config-persistence tests. Each such test points
    /// `XDG_CONFIG_HOME` at a tempdir before spawning so the daemon's
    /// `PATCH /config` writes there instead of the developer's real config; a
    /// private daemon keeps those PATCHes from leaking into the shared daemon's
    /// state that the other tests read.
    fn engine_on_isolated_daemon() -> Engine {
        use crate::test_support::spawn_test_daemon;
        let client = DaemonClient::connect(spawn_test_daemon()).unwrap();
        let project = client
            .create_project("p", std::path::Path::new("/tmp/none"), None)
            .unwrap();
        let app = App::new(project, vec![]);
        Engine::new(client, Config::default(), app)
    }

    /// A minimal `Ticket` for applier tests: only the fields the applier reads
    /// (id, project_id, title, status, session_name) carry meaning; the rest are
    /// defaults.
    fn sample_ticket(project_id: i64, id: i64, title: &str, status: Status) -> Ticket {
        Ticket {
            id,
            project_id,
            title: title.to_string(),
            description: String::new(),
            initial_prompt: None,
            agent: Agent::Claude,
            status,
            position: 0,
            session_name: None,
            worktree_path: None,
            branch: None,
            auto_reviewed: false,
            instrumented: false,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    // ── Effect enum shape ────────────────────────────────────────────────────

    #[test]
    fn effect_enum_has_only_the_collapsed_variants() {
        // Compile-time guard: constructing each remaining variant must type-check.
        let _ = [
            Effect::None,
            Effect::SwitchProject,
            Effect::SelfUpdate {
                version: "x".into(),
            },
            Effect::Attach { name: "s".into() },
        ];
    }

    // ── connection-loss flag ─────────────────────────────────────────────────

    #[test]
    fn take_connection_lost_reads_and_clears() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        assert!(!e.take_connection_lost(), "starts clear");
        e.flag_connection_lost();
        assert!(e.take_connection_lost(), "flag is observed once");
        assert!(!e.take_connection_lost(), "and cleared after reading");
    }

    /// A domain error from a *live* daemon (background start with no worktree
    /// location → BadRequest) must NOT raise the reconnect flag — only
    /// `Unreachable` does.
    #[test]
    fn bad_request_handler_does_not_flag_connection_lost() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.on_key(key('c')).unwrap();
        for ch in "Add login".chars() {
            e.on_key(key(ch)).unwrap();
        }
        e.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        // The background start hit a BadRequest (no worktree location); that is a
        // domain error and must not request a reconnect.
        assert_eq!(e.app.tickets.len(), 1);
        assert!(
            !e.take_connection_lost(),
            "a BadRequest from a live daemon must not flag connection loss"
        );
    }

    // ── SSE application ──────────────────────────────────────────────────────

    #[test]
    fn sse_ticket_created_for_current_project_inserts() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let pid = e.app.project.id;
        let t = sample_ticket(pid, 1, "New", Status::Todo);
        e.apply_sse_event(CoreEvent::TicketCreated(t));
        assert_eq!(e.app.tickets.len(), 1);
        assert_eq!(e.app.tickets[0].title, "New");
    }

    #[test]
    fn sse_ticket_created_for_other_project_is_ignored() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let other = sample_ticket(e.app.project.id + 999, 1, "Elsewhere", Status::Todo);
        e.apply_sse_event(CoreEvent::TicketCreated(other));
        assert!(
            e.app.tickets.is_empty(),
            "events for other projects are ignored"
        );
    }

    #[test]
    fn sse_ticket_moved_to_review_updates_status_and_toasts() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let pid = e.app.project.id;
        e.app.tickets = vec![sample_ticket(pid, 1, "t", Status::InProgress)];
        e.apply_sse_event(CoreEvent::TicketMoved {
            id: 1,
            from: Status::InProgress,
            to: Status::Review,
            at: String::new(),
        });
        assert_eq!(e.app.tickets[0].status, Status::Review);
        let msg = e.app.status_message.as_ref().unwrap();
        assert!(msg.text.contains("Needs attention"));
        assert_eq!(msg.kind, crate::app::StatusKind::Info);
    }

    #[test]
    fn sse_ticket_deleted_removes_and_prunes() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let pid = e.app.project.id;
        e.app.tickets = vec![sample_ticket(pid, 1, "t", Status::Todo)];
        e.app.selected_ids.insert(1);
        e.apply_sse_event(CoreEvent::TicketDeleted { id: 1 });
        assert!(e.app.tickets.is_empty());
        assert!(!e.app.selected_ids.contains(&1));
    }

    // ── Mutation handlers → client ───────────────────────────────────────────

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

    /// A non-start column move (e.g. to Review) routes through the daemon and is
    /// reflected on the board after `refresh_from_client`. No git/zellij needed.
    #[test]
    fn move_to_review_routes_through_daemon() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let t = e
            .client
            .create_ticket(e.app.project.id, "t", "", None, Agent::Claude)
            .unwrap();
        e.refresh_from_client().unwrap();
        assert_eq!(e.move_selected(Status::Review).unwrap(), Effect::None);
        assert_eq!(e.client.get_ticket(t.id).unwrap().status, Status::Review);
        assert_eq!(e.app.tickets[0].status, Status::Review);
    }

    /// `apply_move` to In Progress on a ticket that ALREADY carries a session
    /// returns `Effect::Attach { name }` — the pure branch decision, no real
    /// worktree/zellij. The move itself is a cheap no-op round-trip to the daemon.
    #[test]
    fn apply_move_with_session_name_returns_attach() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let created = e
            .client
            .create_ticket(e.app.project.id, "t", "", None, Agent::Claude)
            .unwrap();
        e.refresh_from_client().unwrap();
        // Construct a ticket value that carries a session_name (the daemon row
        // doesn't have one, but apply_move's branch keys off the passed Ticket).
        let mut ticket = e.app.tickets[0].clone();
        assert_eq!(ticket.id, created.id);
        ticket.session_name = Some("kamaji-sess".into());
        assert_eq!(
            e.apply_move(ticket, Status::InProgress).unwrap(),
            Effect::Attach {
                name: "kamaji-sess".into()
            }
        );
    }

    /// Background start with no worktree location configured: the card is created
    /// (in Todo) and the daemon's `/start` rejects the start with a BadRequest,
    /// which the TUI surfaces as an error toast.
    #[test]
    fn create_with_background_no_worktree_location_toasts_bad_request() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        // worktree_base stays None; the daemon owns the precondition.
        e.on_key(key('c')).unwrap();
        for ch in "Add login".chars() {
            e.on_key(key(ch)).unwrap();
        }
        let effect = e
            .on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(effect, Effect::None);
        assert_eq!(e.app.tickets.len(), 1);
        assert_eq!(e.app.tickets[0].status, Status::Todo);
        let msg = e
            .app
            .status_message
            .as_ref()
            .expect("an error toast is shown");
        assert_eq!(msg.kind, crate::app::StatusKind::Error);
    }

    /// With the background toggle off, creation is the classic Todo card with no
    /// session and no start attempt.
    #[test]
    fn create_without_background_toggle_makes_todo_card() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.on_key(key('c')).unwrap();
        for ch in "Plan only".chars() {
            e.on_key(key(ch)).unwrap();
        }
        // Tab to Background and turn it off.
        for _ in 0..4 {
            e.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
                .unwrap();
        }
        e.on_key(key(' ')).unwrap();
        let effect = e
            .on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(effect, Effect::None);
        assert_eq!(e.app.tickets[0].status, Status::Todo);
        assert_eq!(e.app.tickets[0].session_name, None);
    }

    fn enter() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
    }

    /// Enter on a ticket the daemon reports with a session_name attaches to it.
    /// Needs a real git repo + zellij for the daemon to actually start a session,
    /// so it is ignored by default (Phase 1 convention); the pure branch decision
    /// is covered by `apply_move_with_session_name_returns_attach`.
    #[test]
    #[ignore = "requires a real git worktree + zellij in the daemon"]
    fn enter_attaches_to_existing_session() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let t = e
            .client
            .create_ticket(e.app.project.id, "t", "", None, Agent::Claude)
            .unwrap();
        let started = e.client.start_ticket(t.id).unwrap();
        let name = started.session_name.clone().unwrap();
        e.refresh_from_client().unwrap();
        assert_eq!(
            e.apply_move(started, Status::InProgress).unwrap(),
            Effect::Attach { name }
        );
    }

    /// Enter starting a fresh session needs a real worktree + zellij; ignored.
    #[test]
    #[ignore = "requires a real git worktree + zellij in the daemon"]
    fn enter_starts_session_for_todo_ticket() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.client
            .create_ticket(e.app.project.id, "Add login", "", Some("go"), Agent::Claude)
            .unwrap();
        e.refresh_from_client().unwrap();
        match e.on_key(enter()).unwrap() {
            Effect::Attach { .. } => {}
            other => panic!("expected Attach, got {other:?}"),
        }
    }

    // ── keymap → modal/effect decisions ──────────────────────────────────────

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

    /// Confirming a theme in the picker persists it through the daemon's
    /// `PATCH /config`: both the engine's in-memory config and the daemon's
    /// own `GET /config` must reflect the choice. A dedicated daemon is spawned
    /// after pointing `XDG_CONFIG_HOME` at a tempdir so the PATCH writes there
    /// (and never the developer's real `~/.config/kamaji/config.toml`).
    #[test]
    fn theme_picker_enter_persists_via_daemon() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());

        let mut e = engine_on_isolated_daemon();
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
        assert_eq!(e.client.get_config().unwrap().theme, "nord");
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

    /// Pressing Enter on the already-selected theme still round-trips through
    /// the daemon (no short-circuit on "no change"), so the persisted config
    /// reflects the current theme.
    #[test]
    fn picker_enter_persists_even_with_no_change() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());

        let mut e = engine_on_isolated_daemon();
        let first = crate::theme::Theme::ALL[0]().name;
        e.app.theme = crate::theme::Theme::ALL[0]();
        e.app.modal = Modal::ThemePicker {
            selected: 0,
            original: 0,
        };
        e.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(e.app.modal, Modal::None));
        assert_eq!(e.client.get_config().unwrap().theme, first);
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
    fn agent_picker_enter_persists_default_via_daemon() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());

        let mut e = engine_on_isolated_daemon();
        e.app.modal = Modal::AgentPicker {
            selected: Agent::Copilot.index(),
        };
        e.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(e.app.modal, Modal::None));
        assert_eq!(e.config.default_agent, "copilot");
        let saved = e.client.get_config().unwrap();
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
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.on_key(key('c')).unwrap();
        for _ in 0..4 {
            e.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
                .unwrap();
        }
        match &e.app.modal {
            Modal::Form(f) => assert_eq!(f.field, FormField::Background),
            other => panic!("expected form, got {other:?}"),
        }
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
        let t = e
            .client
            .create_ticket(e.app.project.id, "t", "", None, Agent::Claude)
            .unwrap();
        e.refresh_from_client().unwrap();
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
        e.on_key(key('/')).unwrap();
        for c in "log".chars() {
            e.on_key(key(c)).unwrap();
        }
        e.on_key(esc()).unwrap();
        assert!(!e.app.search.editing);
        assert!(e.app.search.query.is_empty());

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

    /// `s` asks the daemon for the project's main session and attaches to the
    /// returned name. The daemon needs zellij to actually start one, so it is
    /// ignored by default.
    #[test]
    #[ignore = "requires zellij in the daemon to start the main session"]
    fn s_targets_the_main_session() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        match e.on_key(key('s')).unwrap() {
            Effect::Attach { name } => assert!(!name.is_empty()),
            other => panic!("expected Attach, got {other:?}"),
        }
    }

    /// Three Todo tickets created via the daemon with the cursor on the first;
    /// returns their ids.
    fn three_todo(e: &mut Engine) -> Vec<i64> {
        let ids: Vec<i64> = (0..3)
            .map(|i| {
                e.client
                    .create_ticket(e.app.project.id, &format!("t{i}"), "", None, Agent::Claude)
                    .unwrap()
                    .id
            })
            .collect();
        e.refresh_from_client().unwrap();
        ids
    }

    #[test]
    fn space_toggles_multi_selection_of_focused_ticket() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let ids = three_todo(&mut e);
        e.on_key(key(' ')).unwrap();
        assert!(
            e.app.selected_ids.contains(&ids[0]),
            "space selects the focused card"
        );
        e.on_key(key(' ')).unwrap();
        assert!(
            !e.app.selected_ids.contains(&ids[0]),
            "space again deselects"
        );
    }

    #[test]
    fn shift_d_with_selection_opens_confirm_done_with_all_ids() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let ids = three_todo(&mut e);
        e.on_key(key(' ')).unwrap();
        e.on_key(key('j')).unwrap();
        e.on_key(key(' ')).unwrap();
        e.on_key(key('D')).unwrap();
        match &e.app.modal {
            Modal::ConfirmDone { ticket_ids } => {
                let mut got = ticket_ids.clone();
                got.sort();
                assert_eq!(got, vec![ids[0], ids[1]]);
            }
            other => panic!("expected ConfirmDone, got {other:?}"),
        }
    }

    #[test]
    fn shift_d_without_selection_targets_focused_ticket() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let ids = three_todo(&mut e);
        e.on_key(key('D')).unwrap();
        match &e.app.modal {
            Modal::ConfirmDone { ticket_ids } => assert_eq!(ticket_ids, &vec![ids[0]]),
            other => panic!("expected ConfirmDone, got {other:?}"),
        }
    }

    #[test]
    fn confirm_done_yes_closes_all_and_clears_selection() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let ids = three_todo(&mut e);
        e.app.modal = Modal::ConfirmDone {
            ticket_ids: vec![ids[0], ids[1]],
        };
        e.app.selected_ids.insert(ids[0]);
        e.app.selected_ids.insert(ids[1]);
        e.on_key(key('y')).unwrap();
        assert_eq!(e.client.get_ticket(ids[0]).unwrap().status, Status::Done);
        assert_eq!(e.client.get_ticket(ids[1]).unwrap().status, Status::Done);
        assert_eq!(
            e.client.get_ticket(ids[2]).unwrap().status,
            Status::Todo,
            "an unselected ticket is untouched"
        );
        assert!(
            e.app.selected_ids.is_empty(),
            "selection is cleared after closing"
        );
    }

    #[test]
    fn confirm_done_no_marks_done_without_cleanup_and_clears_selection() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let ids = three_todo(&mut e);
        e.app.modal = Modal::ConfirmDone {
            ticket_ids: vec![ids[0]],
        };
        e.app.selected_ids.insert(ids[0]);
        e.on_key(key('n')).unwrap();
        assert_eq!(e.client.get_ticket(ids[0]).unwrap().status, Status::Done);
        assert!(e.app.selected_ids.is_empty());
    }

    #[test]
    fn confirm_done_esc_cancels_and_keeps_selection() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let ids = three_todo(&mut e);
        e.app.modal = Modal::ConfirmDone {
            ticket_ids: vec![ids[0]],
        };
        e.app.selected_ids.insert(ids[0]);
        e.on_key(esc()).unwrap();
        assert_eq!(
            e.client.get_ticket(ids[0]).unwrap().status,
            Status::Todo,
            "Esc leaves the ticket open"
        );
        assert!(
            e.app.selected_ids.contains(&ids[0]),
            "Esc keeps the selection so the user can retry"
        );
    }

    #[test]
    fn confirm_delete_yes_removes_ticket() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let ids = three_todo(&mut e);
        e.app.modal = Modal::ConfirmDelete { ticket_id: ids[0] };
        e.on_key(key('y')).unwrap();
        assert!(matches!(
            e.client.get_ticket(ids[0]),
            Err(crate::client::ClientError::NotFound)
        ));
        assert_eq!(e.app.tickets.len(), 2);
    }

    #[test]
    fn esc_clears_selection_before_clearing_search() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        three_todo(&mut e);
        e.app.search.query = "t".into();
        e.on_key(key(' ')).unwrap();
        assert!(!e.app.selected_ids.is_empty());
        e.on_key(esc()).unwrap();
        assert!(
            e.app.selected_ids.is_empty(),
            "first Esc clears the selection"
        );
        assert_eq!(
            e.app.search.query, "t",
            "the search filter survives the first Esc"
        );
        e.on_key(esc()).unwrap();
        assert!(
            e.app.search.query.is_empty(),
            "second Esc clears the filter"
        );
    }

    #[test]
    fn shift_d_with_no_tickets_does_nothing() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.on_key(key('D')).unwrap();
        assert!(matches!(e.app.modal, Modal::None));
    }

    /// `w` opens the worktree-location selector, pre-filled from config.
    #[test]
    fn w_opens_worktree_location_picker() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.config.worktree_base = Some("/wt".to_string());
        e.on_key(key('w')).unwrap();
        match &e.app.modal {
            Modal::WorktreeLocation(form) => assert_eq!(form.dir.value, "/wt"),
            other => panic!("expected WorktreeLocation, got {other:?}"),
        }
    }

    /// Confirming an existing directory in the selector persists it through the
    /// daemon and closes the modal.
    #[test]
    fn worktree_picker_enter_saves_existing_dir_via_daemon() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
        let target = dir.path().join("worktrees");
        std::fs::create_dir(&target).unwrap();

        let mut e = engine_on_isolated_daemon();
        e.on_key(key('w')).unwrap();
        for c in target.to_string_lossy().chars() {
            e.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
                .unwrap();
        }
        e.on_key(enter()).unwrap();

        assert!(matches!(e.app.modal, Modal::None), "modal closes on save");
        assert_eq!(
            e.config.worktree_base,
            Some(target.to_string_lossy().to_string())
        );
        let saved = e.client.get_config().unwrap();
        assert_eq!(saved.worktree_base, e.config.worktree_base);
    }

    /// A missing directory is not saved on the first Enter; it arms a
    /// create-confirm prompt instead.
    #[test]
    fn worktree_picker_missing_dir_arms_confirm_before_saving() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
        let target = dir.path().join("does-not-exist-yet");

        let mut e = engine_on_isolated_daemon();
        e.on_key(key('w')).unwrap();
        for c in target.to_string_lossy().chars() {
            e.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
                .unwrap();
        }
        e.on_key(enter()).unwrap();
        assert!(e.config.worktree_base.is_none(), "not saved before confirm");
        match &e.app.modal {
            Modal::WorktreeLocation(form) => assert!(form.dir.pending_create.is_some()),
            other => panic!("expected WorktreeLocation, got {other:?}"),
        }
        e.on_key(enter()).unwrap();
        assert!(target.is_dir(), "directory is created on confirm");
        assert_eq!(
            e.config.worktree_base,
            Some(target.to_string_lossy().to_string())
        );
        assert!(matches!(e.app.modal, Modal::None));
    }

    /// Esc closes the selector without touching the config.
    #[test]
    fn worktree_picker_esc_closes_without_saving() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.on_key(key('w')).unwrap();
        e.on_key(esc()).unwrap();
        assert!(matches!(e.app.modal, Modal::None));
        assert!(e.config.worktree_base.is_none());
    }
}
