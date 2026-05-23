use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use std::path::PathBuf;
use std::time::Duration;

use crate::db::Db;
use crate::models::Project;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectField {
    Name,
    Root,
}

/// State for the new-project modal form. Mirrors `TicketForm` so the picker's
/// create flow behaves and looks like the ticket create/edit modal.
struct ProjectForm {
    name: String,
    root: String,
    field: ProjectField,
    error: Option<String>,
}

impl ProjectForm {
    fn new() -> Self {
        ProjectForm {
            name: String::new(),
            root: String::new(),
            field: ProjectField::Name,
            error: None,
        }
    }

    fn next_field(&mut self) {
        self.field = match self.field {
            ProjectField::Name => ProjectField::Root,
            ProjectField::Root => ProjectField::Name,
        };
    }

    // With two fields, the previous field is the same as the next.
    fn prev_field(&mut self) {
        self.next_field();
    }

    fn input_char(&mut self, c: char) {
        match self.field {
            ProjectField::Name => self.name.push(c),
            ProjectField::Root => self.root.push(c),
        }
    }

    fn backspace(&mut self) {
        match self.field {
            ProjectField::Name => self.name.pop(),
            ProjectField::Root => self.root.pop(),
        };
    }

    /// Resolve the entered root directory, expanding a leading `~`.
    fn resolved_root(&self) -> PathBuf {
        PathBuf::from(shellexpand(&self.root))
    }
}

struct PickerState {
    projects: Vec<Project>,
    selected: usize,
    /// `Some` while the new-project modal is open.
    form: Option<ProjectForm>,
}

/// Run the project picker loop. Returns the chosen project, or None if the user
/// quit without selecting.
pub fn run(terminal: &mut DefaultTerminal, db: &Db) -> Result<Option<Project>> {
    let mut state = PickerState {
        projects: db.list_projects()?,
        selected: 0,
        form: None,
    };

    loop {
        terminal.draw(|frame| render(frame, &state))?;

        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match &mut state.form {
            None => match key.code {
                KeyCode::Char('q') => return Ok(None),
                KeyCode::Char('n') => {
                    state.form = Some(ProjectForm::new());
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    state.selected = state.selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') if state.selected + 1 < state.projects.len() => {
                    state.selected += 1;
                }
                KeyCode::Enter => {
                    if let Some(p) = state.projects.get(state.selected) {
                        return Ok(Some(p.clone()));
                    }
                }
                _ => {}
            },
            Some(form) => match key.code {
                KeyCode::Esc => state.form = None,
                KeyCode::Tab => form.next_field(),
                KeyCode::BackTab => form.prev_field(),
                KeyCode::Enter => {
                    if form.name.trim().is_empty() {
                        form.error = Some("Name is required".into());
                    } else {
                        let root = form.resolved_root();
                        if !root.is_dir() {
                            form.error = Some(format!("Not a directory: {}", root.display()));
                        } else {
                            let project = db.create_project(form.name.trim(), &root, None)?;
                            return Ok(Some(project));
                        }
                    }
                }
                KeyCode::Backspace => form.backspace(),
                KeyCode::Char(c) => form.input_char(c),
                _ => {}
            },
        }
    }
}

/// Expand a leading `~` to the home directory.
fn shellexpand(input: &str) -> String {
    if let Some(rest) = input.strip_prefix("~/") {
        if let Some(home) = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()) {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    input.to_string()
}

fn render(frame: &mut Frame, state: &PickerState) {
    let [title_area, body_area, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(
        Paragraph::new(" kamaji — select a project").style(Style::new().fg(Color::Cyan)),
        title_area,
    );

    // The project list is always rendered; the new-project form (when open)
    // overlays it as a centered modal, matching the ticket create/edit modal.
    let items: Vec<ListItem> = state
        .projects
        .iter()
        .map(|p| ListItem::new(format!("{}  ({})", p.name, p.root_dir.display())))
        .collect();
    let mut list_state = ListState::default();
    if !state.projects.is_empty() {
        list_state.select(Some(state.selected));
    }
    let list = List::new(items)
        .block(Block::bordered().title(" Projects "))
        .highlight_style(
            Style::new()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, body_area, &mut list_state);
    frame.render_widget(
        Paragraph::new(" ↑/↓ select   Enter open   n new   q quit"),
        hint_area,
    );

    if let Some(form) = &state.form {
        crate::ui::render_field_modal(
            frame,
            "New project",
            &[
                ("Name", &form.name, form.field == ProjectField::Name),
                (
                    "Root directory (~ ok)",
                    &form.root,
                    form.field == ProjectField::Root,
                ),
            ],
            "Tab/Shift-Tab: field   Enter: create   Esc: cancel",
            form.error.as_deref(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_project_form_starts_on_name_field() {
        let form = ProjectForm::new();
        assert_eq!(form.field, ProjectField::Name);
        assert!(form.name.is_empty());
        assert!(form.root.is_empty());
    }

    #[test]
    fn field_navigation_toggles_between_name_and_root() {
        let mut form = ProjectForm::new();
        form.next_field();
        assert_eq!(form.field, ProjectField::Root);
        form.next_field();
        assert_eq!(form.field, ProjectField::Name);
        form.prev_field();
        assert_eq!(form.field, ProjectField::Root);
    }

    #[test]
    fn typing_targets_the_active_field() {
        let mut form = ProjectForm::new();
        form.input_char('a');
        form.input_char('b');
        assert_eq!(form.name, "ab");
        assert_eq!(form.root, "");

        form.next_field();
        form.input_char('~');
        form.input_char('/');
        assert_eq!(form.name, "ab");
        assert_eq!(form.root, "~/");
    }

    #[test]
    fn backspace_removes_from_active_field() {
        let mut form = ProjectForm::new();
        form.input_char('x');
        form.input_char('y');
        form.backspace();
        assert_eq!(form.name, "x");
    }

    #[test]
    fn resolved_root_expands_leading_tilde() {
        let mut form = ProjectForm::new();
        form.next_field();
        for c in "~/foo".chars() {
            form.input_char(c);
        }
        let resolved = form.resolved_root();
        assert!(!resolved.to_string_lossy().starts_with('~'));
        assert!(resolved.to_string_lossy().ends_with("/foo"));
    }
}
