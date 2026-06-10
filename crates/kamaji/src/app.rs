//! The TUI's in-memory view state: the board (project, tickets, selection),
//! the active modal, and search/filter state. `App` is plain data with small
//! pure helpers — the [`Engine`](crate::engine::Engine) mutates it and
//! [`ui::render`](crate::ui::render) draws it; it performs no I/O of its own.

use crate::dir_select::DirField;
use crate::theme::Theme;
use kamaji_core::detect::SignalLevel;
use kamaji_core::models::{Agent, Project, Status, Ticket};
use std::collections::{HashMap, HashSet};

/// Index of the rightmost board column (Done). Derived from the status list so
/// the navigation clamps don't hardcode the column count.
pub const LAST_COLUMN: usize = Status::all().len() - 1;

/// Board search/filter state. An empty query means no filter is applied.
#[derive(Debug, Clone, Default)]
pub struct Search {
    /// The current query text.
    pub query: String,
    /// True while the user is typing the query (board input is captured by
    /// search instead of the normal hotkeys).
    pub editing: bool,
}

impl Search {
    /// True when no query is set (the board shows every ticket).
    pub fn is_empty(&self) -> bool {
        self.query.is_empty()
    }

    /// Case-insensitive substring match against the ticket title. An empty
    /// query matches every ticket.
    pub fn matches(&self, ticket: &Ticket) -> bool {
        if self.query.is_empty() {
            return true;
        }
        ticket
            .title
            .to_lowercase()
            .contains(&self.query.to_lowercase())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormField {
    Title,
    Description,
    InitialPrompt,
    Agent,
    Background,
}

#[derive(Debug, Clone)]
pub struct TicketForm {
    pub editing_id: Option<i64>,
    pub title: String,
    pub description: String,
    pub initial_prompt: String,
    pub agent: Agent,
    pub start_in_background: bool,
    pub field: FormField,
}

impl TicketForm {
    pub fn new_create(default_agent: Agent) -> Self {
        TicketForm {
            editing_id: None,
            title: String::new(),
            description: String::new(),
            initial_prompt: String::new(),
            agent: default_agent,
            start_in_background: true,
            field: FormField::Title,
        }
    }

    pub fn from_ticket(t: &Ticket) -> Self {
        TicketForm {
            editing_id: Some(t.id),
            title: t.title.clone(),
            description: t.description.clone(),
            initial_prompt: t.initial_prompt.clone().unwrap_or_default(),
            agent: t.agent,
            start_in_background: false,
            field: FormField::Title,
        }
    }

    /// Fields available for the current mode. Editing existing tickets only
    /// exposes Title and Description (per spec).
    fn fields(&self) -> &'static [FormField] {
        if self.editing_id.is_some() {
            &[FormField::Title, FormField::Description]
        } else {
            &[
                FormField::Title,
                FormField::Description,
                FormField::InitialPrompt,
                FormField::Agent,
                FormField::Background,
            ]
        }
    }

    pub fn next_field(&mut self) {
        let fs = self.fields();
        let i = fs.iter().position(|f| *f == self.field).unwrap_or(0);
        self.field = fs[(i + 1) % fs.len()];
    }

    pub fn prev_field(&mut self) {
        let fs = self.fields();
        let i = fs.iter().position(|f| *f == self.field).unwrap_or(0);
        self.field = fs[(i + fs.len() - 1) % fs.len()];
    }

    pub fn input_char(&mut self, c: char) {
        match self.field {
            FormField::Title => self.title.push(c),
            FormField::Description => self.description.push(c),
            FormField::InitialPrompt => self.initial_prompt.push(c),
            FormField::Agent | FormField::Background => {}
        }
    }

    /// The Description and Prompt fields are multi-line text areas: pressing
    /// Enter inserts a newline rather than submitting the form. (Single-line
    /// fields like Title keep Enter as "save".)
    pub fn is_multiline_field(&self) -> bool {
        matches!(
            self.field,
            FormField::Description | FormField::InitialPrompt
        )
    }

    /// Insert a literal newline into the active field. A no-op unless the active
    /// field is a multi-line text area, so a stray Enter elsewhere can't sneak a
    /// newline into a single-line value.
    pub fn input_newline(&mut self) {
        match self.field {
            FormField::Description => self.description.push('\n'),
            FormField::InitialPrompt => self.initial_prompt.push('\n'),
            FormField::Title | FormField::Agent | FormField::Background => {}
        }
    }

    pub fn backspace(&mut self) {
        match self.field {
            FormField::Title => self.title.pop(),
            FormField::Description => self.description.pop(),
            FormField::InitialPrompt => self.initial_prompt.pop(),
            FormField::Agent | FormField::Background => None,
        };
    }

    pub fn toggle_background(&mut self) {
        self.start_in_background = !self.start_in_background;
    }

    pub fn cycle_agent(&mut self, forward: bool) {
        let all = Agent::all();
        let i = all.iter().position(|a| *a == self.agent).unwrap_or(0);
        let n = all.len();
        self.agent = if forward {
            all[(i + 1) % n]
        } else {
            all[(i + n - 1) % n]
        };
    }

    pub fn prompt_opt(&self) -> Option<String> {
        if self.initial_prompt.is_empty() {
            None
        } else {
            Some(self.initial_prompt.clone())
        }
    }
}

/// State for the worktree-location selector: a single directory field with the
/// same fuzzy subdirectory search as the project-root field, plus an optional
/// validation error.
#[derive(Debug, Clone, Default)]
pub struct WorktreeForm {
    pub dir: DirField,
    pub error: Option<String>,
}

impl WorktreeForm {
    /// Open the form, pre-filling `current` (the existing configured location,
    /// if any) so the user edits rather than retypes it.
    pub fn new(current: Option<&str>) -> Self {
        WorktreeForm {
            dir: DirField::with_value(current.unwrap_or_default()),
            error: None,
        }
    }
}

/// The focusable rows of the project-settings modal. `Name`/`Root` are text
/// fields; `Agent` cycles the default agent with ←/→; `Delete` is an action row
/// whose Enter opens the delete-project confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectSettingsField {
    Name,
    Root,
    Agent,
    Delete,
}

/// State for the project-settings modal (`P` on the board): the editable name,
/// root directory (a [`DirField`] so its fuzzy search matches the project-root
/// and worktree selectors), and default agent — plus the read-only id and
/// creation time shown for reference, and an optional validation error.
#[derive(Debug, Clone)]
pub struct ProjectSettingsForm {
    pub id: i64,
    pub created_at: String,
    pub name: String,
    pub root: DirField,
    pub default_agent: Option<Agent>,
    pub field: ProjectSettingsField,
    pub error: Option<String>,
}

impl ProjectSettingsForm {
    /// Open the form pre-filled from an existing project.
    pub fn from_project(p: &Project) -> Self {
        ProjectSettingsForm {
            id: p.id,
            created_at: p.created_at.clone(),
            name: p.name.clone(),
            root: DirField::with_value(p.root_dir.to_string_lossy()),
            default_agent: p.default_agent,
            field: ProjectSettingsField::Name,
            error: None,
        }
    }

    /// The agent options, in cycle order: the global default (None) followed by
    /// every concrete agent.
    fn agent_options() -> Vec<Option<Agent>> {
        std::iter::once(None)
            .chain(Agent::all().into_iter().map(Some))
            .collect()
    }

    pub fn next_field(&mut self) {
        use ProjectSettingsField::*;
        // Switching off Root invalidates a pending "create this directory?" prompt.
        self.root.pending_create = None;
        self.field = match self.field {
            Name => Root,
            Root => Agent,
            Agent => Delete,
            Delete => Name,
        };
    }

    pub fn prev_field(&mut self) {
        use ProjectSettingsField::*;
        self.root.pending_create = None;
        self.field = match self.field {
            Name => Delete,
            Root => Name,
            Agent => Root,
            Delete => Agent,
        };
    }

    pub fn input_char(&mut self, c: char) {
        match self.field {
            ProjectSettingsField::Name => {
                // Editing invalidates a pending confirmation against the old root.
                self.root.pending_create = None;
                self.name.push(c);
            }
            ProjectSettingsField::Root => self.root.input_char(c),
            ProjectSettingsField::Agent | ProjectSettingsField::Delete => {}
        }
    }

    pub fn backspace(&mut self) {
        match self.field {
            ProjectSettingsField::Name => {
                self.root.pending_create = None;
                self.name.pop();
            }
            ProjectSettingsField::Root => {
                self.root.backspace();
            }
            ProjectSettingsField::Agent | ProjectSettingsField::Delete => {}
        }
    }

    /// Cycle the default agent through `[global default, …agents]`.
    pub fn cycle_agent(&mut self, forward: bool) {
        let opts = Self::agent_options();
        let i = opts
            .iter()
            .position(|a| *a == self.default_agent)
            .unwrap_or(0);
        let n = opts.len();
        self.default_agent = if forward {
            opts[(i + 1) % n]
        } else {
            opts[(i + n - 1) % n]
        };
    }
}

#[derive(Debug, Clone)]
pub enum Modal {
    None,
    Form(TicketForm),
    Move {
        ticket_id: i64,
        target: Status,
    },
    /// Confirm closing (moving to Done) one or more tickets. Carries every id
    /// so a single close and a bulk close share one flow.
    ConfirmDone {
        ticket_ids: Vec<i64>,
    },
    ConfirmDelete {
        ticket_id: i64,
    },
    Help,
    /// Theme picker: live-previews `Theme::ALL[selected]`; `original` is the
    /// index to restore on cancel.
    ThemePicker {
        selected: usize,
        original: usize,
    },
    /// Global default-agent picker: `selected` indexes `Agent::all()`. Unlike
    /// the theme picker there is no live preview (nothing changes on the board),
    /// so there is no `original` to restore — Esc simply closes.
    AgentPicker {
        selected: usize,
    },
    /// Directory selector for the worktree location (issue #48). Reuses the
    /// project-root fuzzy search; on confirm it persists `config.worktree_base`.
    WorktreeLocation(WorktreeForm),
    /// Edit/delete the current project: lists its properties and edits name,
    /// root dir, and default agent. On save it `PATCH`es `/projects/:id`.
    ProjectSettings(ProjectSettingsForm),
    /// Confirm deleting `id` (named `name` for the prompt) and all its tickets.
    ConfirmDeleteProject {
        id: i64,
        name: String,
    },
}

/// Severity of a transient status-bar message, controlling its color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    /// Informational / success — a neutral color, not alarming.
    Info,
    /// Something went wrong — rendered in the theme's error color.
    Error,
}

/// A transient message shown in the status bar, tagged with a severity so the
/// renderer can color errors differently from ordinary status updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusMessage {
    pub kind: StatusKind,
    pub text: String,
}

pub struct App {
    pub project: Project,
    pub tickets: Vec<Ticket>,
    pub selected_col: usize,
    pub selected_row: usize,
    pub modal: Modal,
    pub status_message: Option<StatusMessage>,
    pub search: Search,
    pub should_quit: bool,
    pub theme: Theme,
    /// Newer version available (set by the background update check), shown in
    /// the status bar and triggering self-update on `u`.
    pub update: Option<String>,
    /// Ticket ids in the multi-select set, for bulk actions (e.g. close
    /// several at once). Independent of the cursor and of the search filter.
    pub selected_ids: HashSet<i64>,
    /// Latest per-ticket activity level streamed from the daemon over SSE
    /// (`session.signal`), used to colour the per-session "working" bullet.
    /// Empty until the first signal arrives; a missing entry renders neutral.
    pub signal_levels: HashMap<i64, SignalLevel>,
}

impl App {
    pub fn new(project: Project, tickets: Vec<Ticket>) -> Self {
        App {
            project,
            tickets,
            selected_col: 0,
            selected_row: 0,
            modal: Modal::None,
            status_message: None,
            search: Search::default(),
            should_quit: false,
            theme: Theme::default(),
            update: None,
            selected_ids: HashSet::new(),
            signal_levels: HashMap::new(),
        }
    }

    /// Toggle the focused ticket's membership in the multi-select set. A no-op
    /// when no ticket is focused (e.g. an empty column).
    pub fn toggle_selected(&mut self) {
        if let Some(id) = self.selected_ticket().map(|t| t.id) {
            if !self.selected_ids.remove(&id) {
                self.selected_ids.insert(id);
            }
        }
    }

    /// Empty the multi-select set.
    pub fn clear_selection(&mut self) {
        self.selected_ids.clear();
    }

    /// Drop any selected ids that no longer correspond to a live ticket (called
    /// after a reload so closed/deleted tickets don't linger in the set).
    pub fn prune_selection(&mut self) {
        let live: HashSet<i64> = self.tickets.iter().map(|t| t.id).collect();
        self.selected_ids.retain(|id| live.contains(id));
    }

    /// Show a neutral informational status message (e.g. a confirmation or an
    /// automatic status transition). Not colored as an error.
    pub fn set_info(&mut self, text: impl Into<String>) {
        self.status_message = Some(StatusMessage {
            kind: StatusKind::Info,
            text: text.into(),
        });
    }

    /// Show an error status message, rendered in the theme's error color.
    pub fn set_error(&mut self, text: impl Into<String>) {
        self.status_message = Some(StatusMessage {
            kind: StatusKind::Error,
            text: text.into(),
        });
    }

    pub fn selected_status(&self) -> Status {
        Status::all()[self.selected_col]
    }

    pub fn column_tickets(&self, status: Status) -> Vec<&Ticket> {
        self.tickets
            .iter()
            .filter(|t| t.status == status && self.search.matches(t))
            .collect()
    }

    /// Count of all tickets in a column, ignoring the active search filter
    /// (used to render the `matches/total` count in the column title).
    pub fn column_total(&self, status: Status) -> usize {
        self.tickets.iter().filter(|t| t.status == status).count()
    }

    pub fn selected_ticket(&self) -> Option<&Ticket> {
        self.column_tickets(self.selected_status())
            .get(self.selected_row)
            .copied()
    }

    fn clamp_row(&mut self) {
        let len = self.column_tickets(self.selected_status()).len();
        self.selected_row = if len == 0 {
            0
        } else {
            self.selected_row.min(len - 1)
        };
    }

    pub fn left(&mut self) {
        if self.selected_col > 0 {
            self.selected_col -= 1;
            self.clamp_row();
        }
    }

    pub fn right(&mut self) {
        if self.selected_col < LAST_COLUMN {
            self.selected_col += 1;
            self.clamp_row();
        }
    }

    pub fn up(&mut self) {
        self.selected_row = self.selected_row.saturating_sub(1);
    }

    pub fn down(&mut self) {
        let len = self.column_tickets(self.selected_status()).len();
        if len > 0 && self.selected_row + 1 < len {
            self.selected_row += 1;
        }
    }

    pub fn reclamp(&mut self) {
        if self.selected_col > LAST_COLUMN {
            self.selected_col = LAST_COLUMN;
        }
        self.clamp_row();
    }

    /// Begin editing the search query (keeps any existing query so `/` re-edits).
    pub fn search_start(&mut self) {
        self.search.editing = true;
    }

    /// Append a character to the query and re-clamp the cursor to the now-
    /// filtered column.
    pub fn search_push(&mut self, c: char) {
        self.search.query.push(c);
        self.clamp_row();
    }

    /// Delete the last query character and re-clamp the cursor.
    pub fn search_backspace(&mut self) {
        self.search.query.pop();
        self.clamp_row();
    }

    /// Commit the current filter: stop editing but keep the query applied.
    pub fn search_commit(&mut self) {
        self.search.editing = false;
    }

    /// Clear the filter entirely and exit editing.
    pub fn search_clear(&mut self) {
        self.search.query.clear();
        self.search.editing = false;
        self.clamp_row();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn project() -> Project {
        Project {
            id: 1,
            name: "p".into(),
            root_dir: PathBuf::from("/tmp/p"),
            default_agent: None,
            created_at: String::new(),
        }
    }

    fn ticket(id: i64, status: Status) -> Ticket {
        Ticket {
            id,
            project_id: 1,
            title: format!("t{id}"),
            description: String::new(),
            initial_prompt: None,
            agent: Agent::Claude,
            status,
            session_name: None,
            worktree_path: None,
            branch: None,
            auto_reviewed: false,
            instrumented: false,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn version_skew_surfaces_as_a_warning_toast() {
        // Mirror the startup glue in `run()`: a detected skew is surfaced as a
        // non-blocking toast (Error kind = the prominent color, but the app is
        // fully usable — warn-only, never blocking).
        let mut app = App::new(project(), vec![]);
        let warning = crate::client::version_skew_warning("0.4.0", "0.3.0")
            .expect("a version mismatch should produce a warning");
        app.set_error(warning);

        let toast = app
            .status_message
            .expect("the skew warning should be visible as a toast");
        assert_eq!(toast.kind, StatusKind::Error);
        assert!(toast.text.contains("0.3.0") && toast.text.contains("0.4.0"));
        assert!(toast.text.contains("restart the daemon"));
    }

    #[test]
    fn navigation_selects_within_columns() {
        let tickets = vec![
            ticket(1, Status::Todo),
            ticket(2, Status::Todo),
            ticket(3, Status::InProgress),
        ];
        let mut app = App::new(project(), tickets);
        assert_eq!(app.selected_ticket().unwrap().id, 1);
        app.down();
        assert_eq!(app.selected_ticket().unwrap().id, 2);
        app.down(); // clamped at bottom
        assert_eq!(app.selected_ticket().unwrap().id, 2);
        app.right(); // In Progress column, row clamps to 0
        assert_eq!(app.selected_status(), Status::InProgress);
        assert_eq!(app.selected_ticket().unwrap().id, 3);
        app.right(); // Review column: empty
        assert!(app.selected_ticket().is_none());
    }

    #[test]
    fn edit_form_only_cycles_title_and_description() {
        let mut f = TicketForm::from_ticket(&ticket(5, Status::Todo));
        assert_eq!(f.field, FormField::Title);
        f.next_field();
        assert_eq!(f.field, FormField::Description);
        f.next_field();
        assert_eq!(f.field, FormField::Title);
    }

    #[test]
    fn description_and_prompt_are_multiline_fields() {
        let mut f = TicketForm::new_create(Agent::Claude);
        f.field = FormField::Title;
        assert!(!f.is_multiline_field(), "title is single-line");
        f.field = FormField::Description;
        assert!(f.is_multiline_field(), "description is a text area");
        f.field = FormField::InitialPrompt;
        assert!(f.is_multiline_field(), "prompt is a text area");
        f.field = FormField::Agent;
        assert!(!f.is_multiline_field(), "agent is not editable text");
    }

    #[test]
    fn input_newline_only_affects_multiline_text_areas() {
        let mut f = TicketForm::new_create(Agent::Claude);
        // Title is single-line: a newline must be ignored.
        f.field = FormField::Title;
        f.input_char('a');
        f.input_newline();
        f.input_char('b');
        assert_eq!(f.title, "ab", "title ignores newlines");

        // Description is a text area: newlines are inserted verbatim.
        f.field = FormField::Description;
        f.input_char('x');
        f.input_newline();
        f.input_char('y');
        assert_eq!(f.description, "x\ny");

        // Prompt is a text area too.
        f.field = FormField::InitialPrompt;
        f.input_char('1');
        f.input_newline();
        f.input_char('2');
        assert_eq!(f.initial_prompt, "1\n2");
        assert_eq!(f.prompt_opt().as_deref(), Some("1\n2"));
    }

    #[test]
    fn create_form_typing_and_agent_cycle() {
        let mut f = TicketForm::new_create(Agent::Claude);
        f.input_char('H');
        f.input_char('i');
        assert_eq!(f.title, "Hi");
        f.field = FormField::Agent;
        f.input_char('x'); // ignored on agent field
        f.cycle_agent(true);
        assert_eq!(f.agent, Agent::Codex);
        assert_eq!(f.prompt_opt(), None);
    }

    #[test]
    fn create_form_has_background_toggle_on_by_default() {
        let mut f = TicketForm::new_create(Agent::Claude);
        assert!(f.start_in_background, "background toggle defaults on");
        // Background is reachable by tabbing in create mode.
        f.field = FormField::Background;
        f.toggle_background();
        assert!(!f.start_in_background);
        f.toggle_background();
        assert!(f.start_in_background);
    }

    #[test]
    fn project_settings_form_prefills_from_project() {
        let mut p = project();
        p.name = "kamaji".into();
        p.root_dir = PathBuf::from("/home/u/dev/kamaji");
        p.default_agent = Some(Agent::Codex);
        let f = ProjectSettingsForm::from_project(&p);
        assert_eq!(f.id, p.id);
        assert_eq!(f.name, "kamaji");
        assert_eq!(f.root.value, "/home/u/dev/kamaji");
        assert_eq!(f.default_agent, Some(Agent::Codex));
        assert_eq!(f.field, ProjectSettingsField::Name);
    }

    #[test]
    fn project_settings_field_navigation_cycles_all_four_rows() {
        use ProjectSettingsField::*;
        let mut f = ProjectSettingsForm::from_project(&project());
        assert_eq!(f.field, Name);
        f.next_field();
        assert_eq!(f.field, Root);
        f.next_field();
        assert_eq!(f.field, Agent);
        f.next_field();
        assert_eq!(f.field, Delete);
        f.next_field();
        assert_eq!(f.field, Name, "wraps back to Name");
        f.prev_field();
        assert_eq!(f.field, Delete, "prev from Name wraps to Delete");
    }

    #[test]
    fn project_settings_typing_targets_active_text_field_only() {
        use ProjectSettingsField::*;
        let mut f = ProjectSettingsForm::from_project(&project());
        f.name.clear();
        f.input_char('h');
        f.input_char('i');
        assert_eq!(f.name, "hi");
        // Agent/Delete rows ignore typed characters.
        f.field = Agent;
        f.input_char('x');
        assert_eq!(f.name, "hi");
    }

    #[test]
    fn project_settings_cycles_agent_including_the_global_default() {
        let mut f = ProjectSettingsForm::from_project(&project());
        f.default_agent = None;
        f.cycle_agent(true);
        assert_eq!(f.default_agent, Some(Agent::all()[0]));
        // Cycling backward from the first agent returns to the global default.
        f.cycle_agent(false);
        assert_eq!(f.default_agent, None);
        // Backward from None wraps to the last agent.
        f.cycle_agent(false);
        assert_eq!(f.default_agent, Some(*Agent::all().last().unwrap()));
    }

    #[test]
    fn new_app_has_no_update() {
        let app = App::new(project(), vec![]);
        assert!(app.update.is_none());
    }

    #[test]
    fn status_helpers_tag_severity() {
        let mut app = App::new(project(), vec![]);
        app.set_error("boom");
        let m = app.status_message.as_ref().unwrap();
        assert_eq!(m.kind, StatusKind::Error);
        assert_eq!(m.text, "boom");

        app.set_info("all good");
        let m = app.status_message.as_ref().unwrap();
        assert_eq!(m.kind, StatusKind::Info);
        assert_eq!(m.text, "all good");
    }

    #[test]
    fn app_has_a_default_theme() {
        let app = App::new(project(), vec![]);
        assert_eq!(app.theme.name, "catppuccin");
    }

    #[test]
    fn toggling_selection_adds_then_removes_the_focused_ticket() {
        let mut app = App::new(project(), vec![ticket(1, Status::Todo)]);
        assert!(app.selected_ids.is_empty());
        app.toggle_selected();
        assert!(app.selected_ids.contains(&1), "first toggle selects");
        app.toggle_selected();
        assert!(!app.selected_ids.contains(&1), "second toggle deselects");
    }

    #[test]
    fn toggling_with_no_focused_ticket_is_a_noop() {
        // Empty Todo column, focused there: nothing to toggle.
        let mut app = App::new(project(), vec![]);
        app.toggle_selected();
        assert!(app.selected_ids.is_empty());
    }

    #[test]
    fn clear_selection_empties_the_set() {
        let mut app = App::new(project(), vec![ticket(1, Status::Todo)]);
        app.toggle_selected();
        assert!(!app.selected_ids.is_empty());
        app.clear_selection();
        assert!(app.selected_ids.is_empty());
    }

    #[test]
    fn prune_selection_drops_ids_no_longer_present() {
        let mut app = App::new(project(), vec![ticket(1, Status::Todo)]);
        app.selected_ids.insert(1);
        app.selected_ids.insert(99); // a ticket that no longer exists
        app.prune_selection();
        assert!(app.selected_ids.contains(&1));
        assert!(!app.selected_ids.contains(&99), "stale id is pruned");
    }

    #[test]
    fn edit_form_omits_background_field() {
        let mut f = TicketForm::from_ticket(&ticket(5, Status::Todo));
        // Cycle through every field; Background must never appear in edit mode.
        for _ in 0..4 {
            assert_ne!(f.field, FormField::Background);
            f.next_field();
        }
    }

    #[test]
    fn narrowing_search_reclamps_selected_row() {
        let mut t1 = ticket(1, Status::Todo);
        t1.title = "alpha".into();
        let mut t2 = ticket(2, Status::Todo);
        t2.title = "alpine".into();
        let mut t3 = ticket(3, Status::Todo);
        t3.title = "beta".into();
        let mut app = App::new(project(), vec![t1, t2, t3]);
        app.selected_row = 2; // "beta"

        app.search_start();
        assert!(app.search.editing);
        for c in "alp".chars() {
            app.search_push(c);
        }
        // Only alpha + alpine remain (2 cards); the cursor must clamp into range.
        assert_eq!(app.column_tickets(Status::Todo).len(), 2);
        assert!(app.selected_row <= 1, "row clamped into the filtered range");
        assert!(app.selected_ticket().is_some());

        // Backspace re-widens the filter and stays valid.
        app.search_backspace();
        assert_eq!(app.search.query, "al");

        // Esc clears the query and exits editing.
        app.search_clear();
        assert!(app.search.is_empty());
        assert!(!app.search.editing);
    }

    #[test]
    fn column_tickets_filters_by_search_and_total_ignores_it() {
        let mut t1 = ticket(1, Status::Todo);
        t1.title = "Add login".into();
        let mut t2 = ticket(2, Status::Todo);
        t2.title = "Fix logout bug".into();
        let mut t3 = ticket(3, Status::Todo);
        t3.title = "Update README".into();
        let mut app = App::new(project(), vec![t1, t2, t3]);

        app.search.query = "log".into();
        let matches = app.column_tickets(Status::Todo);
        assert_eq!(matches.len(), 2, "login + logout match 'log'");
        assert_eq!(
            app.column_total(Status::Todo),
            3,
            "the unfiltered total ignores the search query"
        );
    }

    #[test]
    fn search_matches_is_case_insensitive_substring() {
        let mut t = ticket(1, Status::Todo);
        t.title = "Add Login".into();

        let mut s = Search::default();
        assert!(s.matches(&t), "an empty query matches everything");

        s.query = "login".into();
        assert!(s.matches(&t), "case-insensitive substring matches");

        s.query = "LOG".into();
        assert!(s.matches(&t));

        s.query = "logout".into();
        assert!(!s.matches(&t), "a non-substring does not match");
    }
}
