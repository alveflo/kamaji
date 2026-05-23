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

enum Mode {
    List,
    NewName,
    NewRoot,
}

struct PickerState {
    projects: Vec<Project>,
    selected: usize,
    mode: Mode,
    name_buf: String,
    root_buf: String,
    error: Option<String>,
}

/// Run the project picker loop. Returns the chosen project, or None if the user
/// quit without selecting.
pub fn run(terminal: &mut DefaultTerminal, db: &Db) -> Result<Option<Project>> {
    let mut state = PickerState {
        projects: db.list_projects()?,
        selected: 0,
        mode: Mode::List,
        name_buf: String::new(),
        root_buf: String::new(),
        error: None,
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

        match state.mode {
            Mode::List => match key.code {
                KeyCode::Char('q') => return Ok(None),
                KeyCode::Char('n') => {
                    state.mode = Mode::NewName;
                    state.name_buf.clear();
                    state.root_buf.clear();
                    state.error = None;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    state.selected = state.selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if state.selected + 1 < state.projects.len() {
                        state.selected += 1;
                    }
                }
                KeyCode::Enter => {
                    if let Some(p) = state.projects.get(state.selected) {
                        return Ok(Some(p.clone()));
                    }
                }
                _ => {}
            },
            Mode::NewName => match key.code {
                KeyCode::Esc => state.mode = Mode::List,
                KeyCode::Enter => {
                    if state.name_buf.trim().is_empty() {
                        state.error = Some("Name is required".into());
                    } else {
                        state.mode = Mode::NewRoot;
                    }
                }
                KeyCode::Backspace => {
                    state.name_buf.pop();
                }
                KeyCode::Char(c) => state.name_buf.push(c),
                _ => {}
            },
            Mode::NewRoot => match key.code {
                KeyCode::Esc => state.mode = Mode::NewName,
                KeyCode::Enter => {
                    let root = PathBuf::from(shellexpand(&state.root_buf));
                    if !root.is_dir() {
                        state.error = Some(format!("Not a directory: {}", root.display()));
                    } else {
                        let project = db.create_project(state.name_buf.trim(), &root, None)?;
                        return Ok(Some(project));
                    }
                }
                KeyCode::Backspace => {
                    state.root_buf.pop();
                }
                KeyCode::Char(c) => state.root_buf.push(c),
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

    match state.mode {
        Mode::List => {
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
        }
        Mode::NewName => {
            let block = Block::bordered().title(" New project — name ");
            frame.render_widget(
                Paragraph::new(format!("{}_", state.name_buf)).block(block),
                body_area,
            );
            frame.render_widget(Paragraph::new(" Enter: next   Esc: cancel"), hint_area);
        }
        Mode::NewRoot => {
            let block = Block::bordered().title(" New project — root directory (~ ok) ");
            frame.render_widget(
                Paragraph::new(format!("{}_", state.root_buf)).block(block),
                body_area,
            );
            frame.render_widget(Paragraph::new(" Enter: create   Esc: back"), hint_area);
        }
    }

    if let Some(err) = &state.error {
        frame.render_widget(
            Paragraph::new(err.clone()).style(Style::new().fg(Color::Red)),
            hint_area,
        );
    }
}
