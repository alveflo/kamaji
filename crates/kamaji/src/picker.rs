use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Padding, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use std::path::PathBuf;
use std::time::Duration;

use crate::client::{ClientError, DaemonClient};
use crate::dir_select::{self, DirField, RootCheck};
use crate::theme::Theme;
use kamaji_core::models::Project;

/// Map a daemon `ClientError` into an `anyhow::Error` for the picker's
/// `Result`-returning loop. Shared with `main.rs`'s board-seeding reads.
pub(crate) fn client_err(e: ClientError) -> anyhow::Error {
    anyhow::anyhow!("daemon request failed: {e:?}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectField {
    Name,
    Root,
}

/// State for the new-project modal form. Mirrors `TicketForm` so the picker's
/// create flow behaves and looks like the ticket create/edit modal. The Root
/// field is a shared [`DirField`] so its fuzzy directory search matches the
/// worktree-location selector exactly.
struct ProjectForm {
    name: String,
    root: DirField,
    field: ProjectField,
    error: Option<String>,
}

impl ProjectForm {
    fn new() -> Self {
        ProjectForm {
            name: String::new(),
            root: DirField::new(),
            field: ProjectField::Name,
            error: None,
        }
    }

    fn next_field(&mut self) {
        // Switching fields invalidates a pending "create this directory?" prompt.
        self.root.pending_create = None;
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
            ProjectField::Name => {
                // Editing invalidates a pending confirmation against the old root.
                self.root.pending_create = None;
                self.name.push(c);
            }
            ProjectField::Root => self.root.input_char(c),
        }
    }

    fn backspace(&mut self) {
        match self.field {
            ProjectField::Name => {
                self.root.pending_create = None;
                self.name.pop();
            }
            ProjectField::Root => self.root.backspace(),
        };
    }

    /// Resolve the entered root directory, expanding a leading `~`.
    fn resolved_root(&self) -> PathBuf {
        self.root.resolved()
    }

    /// Recompute Root-field suggestions from its current text.
    fn refresh_suggestions(&mut self) {
        self.root.refresh();
    }

    /// Move the suggestion highlight by `delta`, clamped to the list bounds.
    fn move_suggestion(&mut self, delta: isize) {
        self.root.move_suggestion(delta);
    }

    /// Accept the highlighted suggestion into the Root field.
    fn accept_suggestion(&mut self) {
        self.root.accept_suggestion();
    }

    /// Handle Esc. Returns `true` when the whole form should close; when a
    /// directory-creation prompt is pending, Esc only dismisses that prompt and
    /// returns `false` so the form stays open for editing.
    fn escape(&mut self) -> bool {
        self.root.escape()
    }

    /// Create the directory awaiting confirmation (parents included) and return
    /// it. Returns `Ok(None)` when nothing was pending.
    fn confirm_create(&mut self) -> std::io::Result<Option<PathBuf>> {
        self.root.confirm_create()
    }
}

struct PickerState {
    projects: Vec<Project>,
    selected: usize,
    /// `Some` while the new-project modal is open.
    form: Option<ProjectForm>,
    theme: Theme,
}

/// Run the project picker loop. Returns the chosen project, or None if the user
/// quit without selecting.
pub fn run(
    terminal: &mut DefaultTerminal,
    client: &DaemonClient,
    theme: Theme,
) -> Result<Option<Project>> {
    let mut state = PickerState {
        projects: client.list_projects().map_err(client_err)?,
        selected: 0,
        form: None,
        theme,
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
                KeyCode::Esc if form.escape() => {
                    state.form = None;
                }
                KeyCode::Tab => {
                    if form.field == ProjectField::Root {
                        // On Root, Tab completes the highlighted entry; with no
                        // matches it does nothing (Shift-Tab returns to Name).
                        if !form.root.suggestions.is_empty() {
                            form.accept_suggestion();
                        }
                    } else {
                        form.next_field();
                        form.refresh_suggestions();
                    }
                }
                KeyCode::BackTab => {
                    form.prev_field();
                    form.refresh_suggestions();
                }
                KeyCode::Up if form.field == ProjectField::Root => form.move_suggestion(-1),
                KeyCode::Down if form.field == ProjectField::Root => form.move_suggestion(1),
                KeyCode::Enter => {
                    if form.name.trim().is_empty() {
                        form.error = Some("Name is required".into());
                    } else if form.root.pending_create.is_some() {
                        // Second Enter: confirm creating the missing directory.
                        match form.confirm_create() {
                            Ok(Some(root)) => {
                                let project = client
                                    .create_project(form.name.trim(), &root, None)
                                    .map_err(client_err)?;
                                return Ok(Some(project));
                            }
                            Ok(None) => {}
                            Err(e) => {
                                form.error = Some(format!("Couldn't create directory: {e}"));
                            }
                        }
                    } else {
                        match dir_select::check_root(form.resolved_root()) {
                            RootCheck::Ready(root) => {
                                let project = client
                                    .create_project(form.name.trim(), &root, None)
                                    .map_err(client_err)?;
                                return Ok(Some(project));
                            }
                            RootCheck::NeedsConfirm(path) => {
                                form.error = None;
                                form.root.pending_create = Some(path);
                            }
                            RootCheck::Invalid(msg) => form.error = Some(msg),
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

/// Visible project rows before the list starts scrolling.
const MAX_VISIBLE_ROWS: usize = 12;
/// Fixed modal width in columns.
const MODAL_WIDTH: u16 = 52;

fn render(frame: &mut Frame, state: &PickerState) {
    let theme = &state.theme;

    // 1. Dimmed backdrop over the whole screen so the modal reads as elevated.
    frame.render_widget(
        Block::default().style(Style::new().bg(theme.backdrop())),
        frame.area(),
    );

    // 2. Centered, fixed-size, content-aware modal box.
    //    height = border(2) + subtitle(1) + blank(1) + rows + blank(1) + hint(1)
    let rows = state.projects.len().clamp(1, MAX_VISIBLE_ROWS) as u16;
    let area = crate::ui::centered_fixed(MODAL_WIDTH, rows + 6, frame.area());
    frame.render_widget(Clear, area);

    // Reset (not Black) so the modal blends with the terminal background on themes with no forced base.
    let block = crate::ui::themed_block(theme, " kamaji ".to_string())
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(theme.base.unwrap_or(Color::Reset)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // 3. Inner layout: subtitle, blank, list, blank, hint.
    let [subtitle_area, _, list_area, _, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    frame.render_widget(
        Paragraph::new("Select a project").style(Style::new().fg(theme.muted)),
        subtitle_area,
    );

    if state.projects.is_empty() {
        frame.render_widget(
            Paragraph::new("No projects yet — press n to create one.")
                .style(Style::new().fg(theme.muted)),
            list_area,
        );
    } else {
        let name_w = state
            .projects
            .iter()
            .map(|p| p.name.chars().count())
            .max()
            .unwrap_or(0);
        let items: Vec<ListItem> = state
            .projects
            .iter()
            .map(|p| {
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{:<name_w$}", p.name), Style::new().fg(theme.text)),
                    Span::raw("  "),
                    Span::styled(
                        dir_select::contract_home(&p.root_dir),
                        Style::new().fg(theme.muted),
                    ),
                ]))
            })
            .collect();
        let mut list_state = ListState::default();
        list_state.select(Some(state.selected));
        // Black fallback matches modals.rs: dark text on the accent highlight bar.
        let list = List::new(items).highlight_symbol("› ").highlight_style(
            Style::new()
                .fg(theme.base.unwrap_or(Color::Black))
                .bg(theme.accent())
                .add_modifier(Modifier::BOLD),
        );
        frame.render_stateful_widget(list, list_area, &mut list_state);
    }

    frame.render_widget(
        Paragraph::new("↑/↓ select · ↵ open · n new · q quit").style(Style::new().fg(theme.muted)),
        hint_area,
    );

    // 4. The new-project form overlays everything when open.
    if let Some(form) = &state.form {
        // While awaiting confirmation to create a missing directory, the message
        // line warns about it and the hint explains how to respond — and the
        // suggestion list is hidden so the prompt stands on its own.
        let pending_msg = form
            .root
            .pending_create
            .as_ref()
            .map(|p| format!("⚠ {} doesn't exist.", dir_select::contract_home(p)));
        let on_root = form.field == ProjectField::Root;
        let (hint, message, suggestions): (&str, Option<&str>, &[String]) =
            if let Some(msg) = &pending_msg {
                ("Enter: create it   Esc: edit", Some(msg.as_str()), &[])
            } else if on_root {
                (
                    "↑/↓ choose · Tab complete · ↵ create · Esc cancel",
                    form.error.as_deref(),
                    &form.root.suggestions,
                )
            } else {
                (
                    "Tab/Shift-Tab: field   Enter: create   Esc: cancel",
                    form.error.as_deref(),
                    &[],
                )
            };
        crate::ui::render_field_modal(
            frame,
            &state.theme,
            "New project",
            &[
                ("Name", &form.name, form.field == ProjectField::Name),
                (
                    "Root directory (~ ok)",
                    &form.root.value,
                    form.field == ProjectField::Root,
                ),
            ],
            hint,
            message,
            (suggestions, form.root.suggestion_idx),
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
        assert!(form.root.value.is_empty());
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
        assert_eq!(form.root.value, "");

        form.next_field();
        form.input_char('~');
        form.input_char('/');
        assert_eq!(form.name, "ab");
        assert_eq!(form.root.value, "~/");
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
    fn switching_field_clears_a_pending_create() {
        let mut form = ProjectForm::new();
        form.root.pending_create = Some(PathBuf::from("/tmp/a"));
        form.next_field();
        assert!(
            form.root.pending_create.is_none(),
            "switching field clears pending"
        );
    }

    #[test]
    fn picker_renders_as_centered_modal() {
        use ratatui::backend::TestBackend;
        use ratatui::layout::Position;
        use ratatui::Terminal;
        use std::path::PathBuf;

        let theme = Theme::by_name("catppuccin");
        let state = PickerState {
            projects: vec![Project {
                id: 1,
                name: "kamaji".into(),
                root_dir: PathBuf::from("/home/u/dev/kamaji"),
                default_agent: None,
                created_at: String::new(),
            }],
            selected: 0,
            form: None,
            theme,
        };

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, &state)).unwrap();
        let buf = terminal.backend().buffer().clone();

        // (a) The modal frame is drawn in the theme's border color.
        let border_found = (0..buf.area.height)
            .any(|y| (0..buf.area.width).any(|x| buf[Position::new(x, y)].fg == theme.border));
        assert!(border_found, "modal frame should use theme.border");

        // (b) The top-left corner lies outside the centered modal and carries
        // the dimmed backdrop — proving it is a modal, not full-screen.
        assert_eq!(buf[Position::new(0, 0)].bg, theme.backdrop());
    }
}
