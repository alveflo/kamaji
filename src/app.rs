use crate::models::{Agent, Project, Status, Ticket};
use crate::theme::Theme;

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

#[derive(Debug, Clone)]
pub enum Modal {
    None,
    Form(TicketForm),
    Move { ticket_id: i64, target: Status },
    ConfirmDone { ticket_id: i64 },
    ConfirmDelete { ticket_id: i64 },
    Help,
}

pub struct App {
    pub project: Project,
    pub tickets: Vec<Ticket>,
    pub selected_col: usize,
    pub selected_row: usize,
    pub modal: Modal,
    pub status_message: Option<String>,
    pub should_quit: bool,
    pub theme: Theme,
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
            should_quit: false,
            theme: Theme::by_name("catppuccin"),
        }
    }

    pub fn selected_status(&self) -> Status {
        Status::all()[self.selected_col]
    }

    pub fn column_tickets(&self, status: Status) -> Vec<&Ticket> {
        self.tickets.iter().filter(|t| t.status == status).collect()
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
        if self.selected_col < 3 {
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
        if self.selected_col > 3 {
            self.selected_col = 3;
        }
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
    fn app_has_a_default_theme() {
        let app = App::new(project(), vec![]);
        assert_eq!(app.theme.name, "catppuccin");
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
}
