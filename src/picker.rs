use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Padding, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::db::Db;
use crate::models::Project;
use crate::theme::Theme;

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
    /// Subdirectory names matching the current Root field segment.
    suggestions: Vec<String>,
    /// Highlighted entry in `suggestions`.
    suggestion_idx: usize,
    /// `Some(path)` once the user has submitted a root directory that doesn't
    /// exist yet and we're awaiting their confirmation to create it.
    pending_create: Option<PathBuf>,
}

impl ProjectForm {
    fn new() -> Self {
        ProjectForm {
            name: String::new(),
            root: String::new(),
            field: ProjectField::Name,
            error: None,
            suggestions: Vec::new(),
            suggestion_idx: 0,
            pending_create: None,
        }
    }

    fn next_field(&mut self) {
        // Switching fields invalidates a pending "create this directory?" prompt.
        self.pending_create = None;
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
        // Editing the path invalidates a pending confirmation against the old value.
        self.pending_create = None;
        match self.field {
            ProjectField::Name => self.name.push(c),
            ProjectField::Root => self.root.push(c),
        }
    }

    fn backspace(&mut self) {
        self.pending_create = None;
        match self.field {
            ProjectField::Name => self.name.pop(),
            ProjectField::Root => self.root.pop(),
        };
    }

    /// Resolve the entered root directory, expanding a leading `~`.
    fn resolved_root(&self) -> PathBuf {
        PathBuf::from(shellexpand(&self.root))
    }

    /// Recompute suggestions for the Root field from its current text, expanding a
    /// leading `~` only to read the filesystem. Resets the highlight to the top.
    fn refresh_suggestions(&mut self) {
        let (parent, partial) = split_root(&self.root);
        let parent_expanded = if parent.is_empty() {
            PathBuf::from(".")
        } else {
            PathBuf::from(shellexpand(parent))
        };
        self.suggestions = dir_suggestions(&parent_expanded, partial);
        self.suggestion_idx = 0;
    }

    /// Move the suggestion highlight by `delta`, clamped to the list bounds.
    fn move_suggestion(&mut self, delta: isize) {
        if self.suggestions.is_empty() {
            return;
        }
        let max = self.suggestions.len() as isize - 1;
        let next = (self.suggestion_idx as isize + delta).clamp(0, max);
        self.suggestion_idx = next as usize;
    }

    /// Accept the highlighted suggestion: replace the in-progress segment with the
    /// chosen directory name plus a trailing `/`, keeping the literal parent text
    /// (e.g. a `~/` prefix). Then refresh suggestions for the new level.
    fn accept_suggestion(&mut self) {
        // Completing the path edits the root, which invalidates a pending
        // "create this directory?" prompt against the old value.
        self.pending_create = None;
        let Some(name) = self.suggestions.get(self.suggestion_idx).cloned() else {
            return;
        };
        let (parent, _partial) = split_root(&self.root);
        self.root = format!("{parent}{name}/");
        self.refresh_suggestions();
    }

    /// Handle Esc. Returns `true` when the whole form should close; when a
    /// directory-creation prompt is pending, Esc only dismisses that prompt and
    /// returns `false` so the form stays open for editing.
    fn escape(&mut self) -> bool {
        if self.pending_create.is_some() {
            self.pending_create = None;
            false
        } else {
            true
        }
    }

    /// Create the directory awaiting confirmation (parents included) and return
    /// it. Returns `Ok(None)` when nothing was pending.
    fn confirm_create(&mut self) -> std::io::Result<Option<PathBuf>> {
        match self.pending_create.take() {
            Some(path) => {
                std::fs::create_dir_all(&path)?;
                Ok(Some(path))
            }
            None => Ok(None),
        }
    }
}

/// Outcome of validating a submitted root directory.
enum RootCheck {
    /// Exists and is a directory — ready to create the project here.
    Ready(PathBuf),
    /// Does not exist — offer to create it.
    NeedsConfirm(PathBuf),
    /// Exists but is not a directory (e.g. a file) — unusable, with a message.
    Invalid(String),
}

fn check_root(path: PathBuf) -> RootCheck {
    if path.is_dir() {
        RootCheck::Ready(path)
    } else if path.exists() {
        RootCheck::Invalid(format!("Not a directory: {}", contract_home(&path)))
    } else {
        RootCheck::NeedsConfirm(path)
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
pub fn run(terminal: &mut DefaultTerminal, db: &Db, theme: Theme) -> Result<Option<Project>> {
    let mut state = PickerState {
        projects: db.list_projects()?,
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
                KeyCode::Esc => {
                    if form.escape() {
                        state.form = None;
                    }
                }
                KeyCode::Tab => {
                    if form.field == ProjectField::Root {
                        // On Root, Tab completes the highlighted entry; with no
                        // matches it does nothing (Shift-Tab returns to Name).
                        if !form.suggestions.is_empty() {
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
                    } else if form.pending_create.is_some() {
                        // Second Enter: confirm creating the missing directory.
                        match form.confirm_create() {
                            Ok(Some(root)) => {
                                let project = db.create_project(form.name.trim(), &root, None)?;
                                return Ok(Some(project));
                            }
                            Ok(None) => {}
                            Err(e) => {
                                form.error = Some(format!("Couldn't create directory: {e}"));
                            }
                        }
                    } else {
                        match check_root(form.resolved_root()) {
                            RootCheck::Ready(root) => {
                                let project = db.create_project(form.name.trim(), &root, None)?;
                                return Ok(Some(project));
                            }
                            RootCheck::NeedsConfirm(path) => {
                                form.error = None;
                                form.pending_create = Some(path);
                            }
                            RootCheck::Invalid(msg) => form.error = Some(msg),
                        }
                    }
                }
                KeyCode::Backspace => {
                    form.backspace();
                    if form.field == ProjectField::Root {
                        form.refresh_suggestions();
                    }
                }
                KeyCode::Char(c) => {
                    form.input_char(c);
                    if form.field == ProjectField::Root {
                        form.refresh_suggestions();
                    }
                }
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

/// Split a raw path string at its last `/` into `(parent, partial)`.
/// `parent` keeps its trailing slash (or is empty when there is no slash);
/// `partial` is the in-progress final segment.
fn split_root(raw: &str) -> (&str, &str) {
    match raw.rfind('/') {
        Some(i) => (&raw[..=i], &raw[i + 1..]),
        None => ("", raw),
    }
}

/// Case-insensitive subsequence test: are all chars of `partial` found in
/// `candidate` in order (not necessarily contiguous)? Empty `partial` matches.
fn fuzzy_subsequence(partial: &str, candidate: &str) -> bool {
    let mut cand = candidate.chars().flat_map(char::to_lowercase);
    'outer: for pc in partial.chars().flat_map(char::to_lowercase) {
        for cc in cand.by_ref() {
            if cc == pc {
                continue 'outer;
            }
        }
        return false;
    }
    true
}

/// List subdirectory names of `parent` whose name fuzzy-matches `partial`.
/// Names that start with `partial` (case-insensitive) sort first; the rest
/// follow, each group alphabetical (case-insensitive). A parent that cannot be
/// read yields an empty list.
fn dir_suggestions(parent: &Path, partial: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let lower_partial = partial.to_lowercase();
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| fuzzy_subsequence(partial, name))
        .collect();
    names.sort_by(|a, b| {
        let a_pref = a.to_lowercase().starts_with(&lower_partial);
        let b_pref = b.to_lowercase().starts_with(&lower_partial);
        b_pref
            .cmp(&a_pref)
            .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
    });
    names
}

/// Contract a leading home-directory prefix to `~` for display (inverse of `shellexpand`).
fn contract_home(path: &Path) -> String {
    if let Some(home) = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()) {
        if let Ok(rest) = path.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
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
                    Span::styled(contract_home(&p.root_dir), Style::new().fg(theme.muted)),
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
            .pending_create
            .as_ref()
            .map(|p| format!("⚠ {} doesn't exist.", contract_home(p)));
        let on_root = form.field == ProjectField::Root;
        let (hint, message, suggestions): (&str, Option<&str>, &[String]) =
            if let Some(msg) = &pending_msg {
                ("Enter: create it   Esc: edit", Some(msg.as_str()), &[])
            } else if on_root {
                (
                    "↑/↓ choose · Tab complete · ↵ create · Esc cancel",
                    form.error.as_deref(),
                    &form.suggestions,
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
                    &form.root,
                    form.field == ProjectField::Root,
                ),
            ],
            hint,
            message,
            (suggestions, form.suggestion_idx),
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

    #[test]
    fn split_root_splits_at_last_slash() {
        assert_eq!(split_root("~/dev/kam"), ("~/dev/", "kam"));
        assert_eq!(split_root("~/dev/"), ("~/dev/", ""));
        assert_eq!(split_root("/abs/path/to/x"), ("/abs/path/to/", "x"));
    }

    #[test]
    fn split_root_with_no_slash_has_empty_parent() {
        assert_eq!(split_root("kam"), ("", "kam"));
        assert_eq!(split_root(""), ("", ""));
    }

    #[test]
    fn fuzzy_subsequence_matches_in_order() {
        assert!(fuzzy_subsequence("km", "kamaji"));
        assert!(fuzzy_subsequence("kam", "kamaji"));
        assert!(!fuzzy_subsequence("mk", "kamaji")); // wrong order
        assert!(!fuzzy_subsequence("xyz", "kamaji"));
    }

    #[test]
    fn fuzzy_subsequence_is_case_insensitive() {
        assert!(fuzzy_subsequence("KM", "kamaji"));
        assert!(fuzzy_subsequence("km", "KamAji"));
    }

    #[test]
    fn fuzzy_subsequence_empty_partial_matches_everything() {
        assert!(fuzzy_subsequence("", "anything"));
        assert!(fuzzy_subsequence("", ""));
    }

    #[test]
    fn dir_suggestions_returns_only_matching_subdirs_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        std::fs::create_dir(base.join("kamaji")).unwrap();
        std::fs::create_dir(base.join("kafka")).unwrap();
        std::fs::create_dir(base.join("zzz")).unwrap();
        std::fs::write(base.join("kamfile.txt"), b"x").unwrap(); // a file, must be excluded

        // partial "ka" matches the two k-dirs (prefix matches first, alphabetical)
        let got = dir_suggestions(base, "ka");
        assert_eq!(got, vec!["kafka".to_string(), "kamaji".to_string()]);

        // empty partial lists all subdirs, prefix group is empty so plain alphabetical
        let all = dir_suggestions(base, "");
        assert_eq!(
            all,
            vec!["kafka".to_string(), "kamaji".to_string(), "zzz".to_string()]
        );
    }

    #[test]
    fn check_root_is_ready_for_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            check_root(dir.path().to_path_buf()),
            RootCheck::Ready(_)
        ));
    }

    #[test]
    fn check_root_needs_confirm_for_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does/not/exist");
        assert!(matches!(check_root(missing), RootCheck::NeedsConfirm(_)));
    }

    #[test]
    fn check_root_is_invalid_for_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a-file");
        std::fs::write(&file, b"x").unwrap();
        assert!(matches!(check_root(file), RootCheck::Invalid(_)));
    }

    #[test]
    fn confirm_create_makes_missing_directory_with_parents() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("new/deeply/nested");

        let mut form = ProjectForm::new();
        // Arm the prompt as the Enter handler would for a missing path.
        form.pending_create = Some(nested.clone());

        let created = form.confirm_create().unwrap();
        assert_eq!(created.as_deref(), Some(nested.as_path()));
        assert!(nested.is_dir(), "directory and parents should be created");
        assert!(form.pending_create.is_none(), "pending is consumed");
    }

    #[test]
    fn confirm_create_is_noop_without_pending() {
        let mut form = ProjectForm::new();
        assert!(form.confirm_create().unwrap().is_none());
    }

    #[test]
    fn escape_dismisses_pending_before_closing_form() {
        let mut form = ProjectForm::new();
        form.pending_create = Some(PathBuf::from("/tmp/whatever"));

        // First Esc only cancels the pending create; form stays open.
        assert!(!form.escape());
        assert!(form.pending_create.is_none());
        // Next Esc closes the form.
        assert!(form.escape());
    }

    #[test]
    fn editing_clears_a_pending_create() {
        let mut form = ProjectForm::new();

        form.pending_create = Some(PathBuf::from("/tmp/a"));
        form.input_char('x');
        assert!(form.pending_create.is_none(), "typing clears pending");

        form.pending_create = Some(PathBuf::from("/tmp/a"));
        form.backspace();
        assert!(form.pending_create.is_none(), "backspace clears pending");

        form.pending_create = Some(PathBuf::from("/tmp/a"));
        form.next_field();
        assert!(
            form.pending_create.is_none(),
            "switching field clears pending"
        );
    }

    #[test]
    fn dir_suggestions_orders_prefix_matches_first() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        std::fs::create_dir(base.join("alpha")).unwrap();
        std::fs::create_dir(base.join("banana")).unwrap();
        std::fs::create_dir(base.join("ant")).unwrap();
        // partial "an": "ant" is a prefix match, "banana" only a subsequence match.
        let got = dir_suggestions(base, "an");
        assert_eq!(got, vec!["ant".to_string(), "banana".to_string()]);
    }

    #[test]
    fn dir_suggestions_nonexistent_parent_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        assert!(dir_suggestions(&missing, "x").is_empty());
    }

    #[test]
    fn refresh_suggestions_lists_subdirs_of_parent() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("kamaji")).unwrap();
        std::fs::create_dir(tmp.path().join("other")).unwrap();

        let mut form = ProjectForm::new();
        form.field = ProjectField::Root;
        // Type an absolute parent path with partial "kam".
        form.root = format!("{}/kam", tmp.path().display());
        form.refresh_suggestions();

        assert_eq!(form.suggestions, vec!["kamaji".to_string()]);
        assert_eq!(form.suggestion_idx, 0);
    }

    #[test]
    fn move_suggestion_clamps_at_both_ends() {
        let mut form = ProjectForm::new();
        form.suggestions = vec!["a".into(), "b".into(), "c".into()];
        form.suggestion_idx = 0;

        form.move_suggestion(-1); // already at top
        assert_eq!(form.suggestion_idx, 0);

        form.move_suggestion(1);
        form.move_suggestion(1);
        assert_eq!(form.suggestion_idx, 2);

        form.move_suggestion(1); // already at bottom
        assert_eq!(form.suggestion_idx, 2);
    }

    #[test]
    fn accept_suggestion_replaces_partial_and_appends_slash() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("kamaji")).unwrap();

        let mut form = ProjectForm::new();
        form.field = ProjectField::Root;
        form.root = format!("{}/kam", tmp.path().display());
        form.refresh_suggestions();
        assert_eq!(form.suggestions, vec!["kamaji".to_string()]);

        form.accept_suggestion();
        assert_eq!(form.root, format!("{}/kamaji/", tmp.path().display()));
    }

    #[test]
    fn accept_suggestion_preserves_tilde_parent() {
        // No filesystem read needed: drive the suggestion list directly.
        let mut form = ProjectForm::new();
        form.field = ProjectField::Root;
        form.root = "~/dev/kam".into();
        form.suggestions = vec!["kamaji".into()];
        form.suggestion_idx = 0;

        form.accept_suggestion();
        // Parent text (including ~/) is preserved; partial replaced + trailing slash.
        assert!(form.root.starts_with("~/dev/kamaji/"));
    }

    #[test]
    fn accept_suggestion_with_empty_list_is_noop() {
        let mut form = ProjectForm::new();
        form.field = ProjectField::Root;
        form.root = "~/dev/".into();
        form.suggestions.clear();
        form.accept_suggestion();
        assert_eq!(form.root, "~/dev/");
    }

    #[test]
    fn contract_home_abbreviates_home_prefix() {
        use std::path::PathBuf;

        let home = directories::BaseDirs::new()
            .map(|b| b.home_dir().to_path_buf())
            .expect("home dir");

        // A path under home is shown with a leading `~`.
        assert_eq!(contract_home(&home.join("dev/kamaji")), "~/dev/kamaji");
        // The home directory itself contracts to a bare `~`.
        assert_eq!(contract_home(&home), "~");
        // Round-trips with shellexpand, the inverse operation.
        assert_eq!(
            shellexpand(&contract_home(&home.join("dev/kamaji"))),
            home.join("dev/kamaji").to_string_lossy()
        );
        // A path outside home is left untouched.
        assert_eq!(contract_home(&PathBuf::from("/opt/foo")), "/opt/foo");
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
