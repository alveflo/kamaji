//! Renders the board's overlay modals — the ticket/project create-edit forms,
//! the move and confirm dialogs, and the directory-search field — as centered
//! frames drawn over the board.

use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{FormField, ProjectSettingsField, ProjectSettingsForm, TicketForm, WorktreeForm};
use crate::dir_select;
use crate::theme::Theme;
use crate::ui::centered_rect;
use kamaji_core::models::{Agent, Status};

/// Maximum suggestion rows shown at once in a field modal; longer lists scroll.
const MAX_VISIBLE_SUGGESTIONS: usize = 5;

/// A rounded modal frame titled `title`, bordered in the theme's border color.
pub(crate) fn themed_block(theme: &Theme, title: String) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.border))
        .title(title)
}

pub(crate) fn field_line(theme: &Theme, label: &str, value: &str, active: bool) -> Line<'static> {
    let style = if active {
        Style::new()
            .fg(theme.base.unwrap_or(Color::Black))
            .bg(theme.accent())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.text)
    };
    let cursor = if active { "_" } else { "" };
    Line::from(vec![
        Span::styled(format!("{label}: "), Style::new().fg(theme.accent())),
        Span::styled(format!("{value}{cursor}"), style),
    ])
}

/// Render a multi-line text-area field: the label sits on the first row beside
/// the opening line of `value`, and each embedded `\n` starts a fresh row. The
/// cursor `_` trails the final row when the field is active. A value with no
/// newline renders the same single row as [`field_line`].
pub(crate) fn field_lines(
    theme: &Theme,
    label: &str,
    value: &str,
    active: bool,
) -> Vec<Line<'static>> {
    let style = if active {
        Style::new()
            .fg(theme.base.unwrap_or(Color::Black))
            .bg(theme.accent())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.text)
    };
    let cursor = if active { "_" } else { "" };
    let segments: Vec<&str> = value.split('\n').collect();
    let last = segments.len() - 1;
    segments
        .into_iter()
        .enumerate()
        .map(|(i, seg)| {
            let text = if i == last {
                format!("{seg}{cursor}")
            } else {
                seg.to_string()
            };
            if i == 0 {
                Line::from(vec![
                    Span::styled(format!("{label}: "), Style::new().fg(theme.accent())),
                    Span::styled(text, style),
                ])
            } else {
                Line::from(Span::styled(text, style))
            }
        })
        .collect()
}

/// Render a centered, bordered modal form: a list of labelled fields with an
/// active-field highlight, a hint line, and an optional error. Shared by modals
/// (like the new-project form) that want the same look as the ticket modal.
pub(crate) fn render_field_modal(
    frame: &mut Frame,
    theme: &Theme,
    title: &str,
    fields: &[(&str, &str, bool)],
    hint: &str,
    error: Option<&str>,
    suggestions: (&[String], usize),
) {
    let (suggestions, selected) = suggestions;
    let area = centered_rect(70, 60, frame.area());
    frame.render_widget(Clear, area);

    let block = themed_block(theme, format!(" {title} "));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for (i, (label, value, active)) in fields.iter().enumerate() {
        if i > 0 {
            lines.push(Line::raw(""));
        }
        lines.push(field_line(theme, label, value, *active));
    }

    if !suggestions.is_empty() {
        lines.push(Line::raw(""));
        // Scroll a fixed-size window so the selected entry stays visible.
        let start = selected.saturating_sub(MAX_VISIBLE_SUGGESTIONS - 1);
        let end = (start + MAX_VISIBLE_SUGGESTIONS).min(suggestions.len());
        for (i, name) in suggestions.iter().enumerate().take(end).skip(start) {
            let style = if i == selected {
                Style::new()
                    .fg(theme.base.unwrap_or(Color::Black))
                    .bg(theme.accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(theme.text)
            };
            let marker = if i == selected { "› " } else { "  " };
            lines.push(Line::styled(format!("{marker}{name}"), style));
        }
    }

    lines.push(Line::raw(""));
    if let Some(err) = error {
        lines.push(Line::styled(err.to_string(), Style::new().fg(theme.error)));
        lines.push(Line::raw(""));
    }
    lines.push(Line::styled(hint.to_string(), Style::new().fg(theme.muted)));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

pub fn render_form(frame: &mut Frame, theme: &Theme, form: &TicketForm) {
    let area = centered_rect(70, 60, frame.area());
    frame.render_widget(Clear, area);

    let title = if form.editing_id.is_some() {
        " Edit ticket "
    } else {
        " New ticket "
    };
    let block = themed_block(theme, title.to_string());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![
        field_line(theme, "Title", &form.title, form.field == FormField::Title),
        Line::raw(""),
    ];
    lines.extend(field_lines(
        theme,
        "Description",
        &form.description,
        form.field == FormField::Description,
    ));
    if form.editing_id.is_none() {
        lines.push(Line::raw(""));
        lines.extend(field_lines(
            theme,
            "Prompt",
            &form.initial_prompt,
            form.field == FormField::InitialPrompt,
        ));
        lines.push(Line::raw(""));
        let agents: Vec<Span> = Agent::all()
            .into_iter()
            .flat_map(|a| {
                let sel = a == form.agent && form.field == FormField::Agent;
                let style = if sel {
                    Style::new()
                        .fg(theme.base.unwrap_or(Color::Black))
                        .bg(theme.accent())
                } else if a == form.agent {
                    Style::new().fg(theme.accent())
                } else {
                    Style::new().fg(theme.muted)
                };
                vec![
                    Span::styled(format!(" {} ", a.label()), style),
                    Span::raw(" "),
                ]
            })
            .collect();
        let mut agent_line = vec![Span::styled("Agent: ", Style::new().fg(theme.accent()))];
        agent_line.extend(agents);
        lines.push(Line::from(agent_line));

        lines.push(Line::raw(""));
        let checkbox = if form.start_in_background {
            "[x]"
        } else {
            "[ ]"
        };
        lines.push(field_line(
            theme,
            "Start in background",
            checkbox,
            form.field == FormField::Background,
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Tab/Shift-Tab: field   ←/→: agent / toggle   Enter: newline (Desc/Prompt)   Ctrl-S: save   Esc: cancel",
        Style::new().fg(theme.muted),
    ));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

pub fn render_move(frame: &mut Frame, theme: &Theme, target: Status) {
    let area = centered_rect(60, 25, frame.area());
    frame.render_widget(Clear, area);
    let block = themed_block(theme, " Move ticket ".to_string());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [cols_area, hint_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(2)]).areas(inner);
    let spans: Vec<Span> = Status::all()
        .into_iter()
        .map(|s| {
            let style = if s == target {
                Style::new()
                    .fg(theme.base.unwrap_or(Color::Black))
                    .bg(theme.accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(theme.text)
            };
            Span::styled(format!(" {} ", s.title()), style)
        })
        .collect();
    frame.render_widget(Paragraph::new(Line::from(spans)), cols_area);
    frame.render_widget(
        Paragraph::new("←/→: choose   Enter: confirm   Esc: cancel")
            .style(Style::new().fg(theme.muted)),
        hint_area,
    );
}

pub fn render_confirm(frame: &mut Frame, theme: &Theme, title: &str, body: &str) {
    let area = centered_rect(50, 20, frame.area());
    frame.render_widget(Clear, area);
    let block = themed_block(theme, format!(" {title} "));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(body)
            .style(Style::new().fg(theme.text))
            .wrap(Wrap { trim: true }),
        inner,
    );
}

pub fn render_theme_picker(frame: &mut Frame, theme: &Theme, selected: usize) {
    let area = centered_rect(40, 50, frame.area());
    frame.render_widget(Clear, area);
    let block = themed_block(theme, " Theme ".to_string());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for (i, make) in Theme::ALL.iter().enumerate() {
        let label = make().label;
        let marker = if i == selected { "▸ " } else { "  " };
        let style = if i == selected {
            Style::new()
                .fg(theme.base.unwrap_or(Color::Black))
                .bg(theme.accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.text)
        };
        lines.push(Line::styled(format!("{marker}{label}"), style));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "↑/↓ preview · ↵ save · Esc cancel",
        Style::new().fg(theme.muted),
    ));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

pub fn render_agent_picker(frame: &mut Frame, theme: &Theme, selected: usize) {
    let area = centered_rect(40, 50, frame.area());
    frame.render_widget(Clear, area);
    let block = themed_block(theme, " Default agent ".to_string());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for (i, agent) in Agent::all().iter().enumerate() {
        let marker = if i == selected { "▸ " } else { "  " };
        let style = if i == selected {
            Style::new()
                .fg(theme.base.unwrap_or(Color::Black))
                .bg(theme.accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.text)
        };
        lines.push(Line::styled(format!("{marker}{}", agent.label()), style));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "↑/↓ select · ↵ save · Esc cancel",
        Style::new().fg(theme.muted),
    ));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Render the worktree-location selector: a single directory field with the
/// same fuzzy suggestion list and create-confirm flow as the project-root
/// field, so the two selectors feel identical (issue #48).
pub fn render_worktree_location(frame: &mut Frame, theme: &Theme, form: &WorktreeForm) {
    let pending_msg = form
        .dir
        .pending_create
        .as_ref()
        .map(|p| format!("⚠ {} doesn't exist.", dir_select::contract_home(p)));
    let (hint, message, suggestions): (&str, Option<&str>, &[String]) =
        if let Some(msg) = &pending_msg {
            ("Enter: create it   Esc: edit", Some(msg.as_str()), &[])
        } else {
            (
                "↑/↓ choose · Tab complete · ↵ save · Esc cancel",
                form.error.as_deref(),
                &form.dir.suggestions,
            )
        };
    render_field_modal(
        frame,
        theme,
        "Worktree location",
        &[("Directory (~ ok)", &form.dir.value, true)],
        hint,
        message,
        (suggestions, form.dir.suggestion_idx),
    );
}

/// Render the project-settings modal: the project's read-only properties (id,
/// created-at), its editable Name / Root / Agent fields, and a Delete action
/// row. The Root field carries the same fuzzy suggestion list + create-confirm
/// flow as the project-root and worktree selectors.
pub fn render_project_settings(frame: &mut Frame, theme: &Theme, form: &ProjectSettingsForm) {
    use ProjectSettingsField as F;
    let area = centered_rect(70, 75, frame.area());
    frame.render_widget(Clear, area);
    let block = themed_block(theme, " Project settings ".to_string());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let prop = |label: &str, value: String| -> Line<'static> {
        Line::from(vec![
            Span::styled(format!("{label}: "), Style::new().fg(theme.accent())),
            Span::styled(value, Style::new().fg(theme.muted)),
        ])
    };

    let mut lines: Vec<Line> = Vec::new();
    // Read-only properties.
    lines.push(prop("ID", form.id.to_string()));
    if !form.created_at.is_empty() {
        lines.push(prop("Created", form.created_at.clone()));
    }
    lines.push(Line::raw(""));

    // Editable: Name.
    lines.push(field_line(theme, "Name", &form.name, form.field == F::Name));
    lines.push(Line::raw(""));

    // Editable: Root (basepath), with the directory-search affordances.
    lines.push(field_line(
        theme,
        "Root",
        &form.root.value,
        form.field == F::Root,
    ));
    let pending = form
        .root
        .pending_create
        .as_ref()
        .map(|p| format!("⚠ {} doesn't exist.", dir_select::contract_home(p)));
    if form.field == F::Root && pending.is_none() && !form.root.suggestions.is_empty() {
        let selected = form.root.suggestion_idx;
        let start = selected.saturating_sub(MAX_VISIBLE_SUGGESTIONS - 1);
        let end = (start + MAX_VISIBLE_SUGGESTIONS).min(form.root.suggestions.len());
        for (i, name) in form
            .root
            .suggestions
            .iter()
            .enumerate()
            .take(end)
            .skip(start)
        {
            let style = if i == selected {
                Style::new()
                    .fg(theme.base.unwrap_or(Color::Black))
                    .bg(theme.accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(theme.text)
            };
            let marker = if i == selected { "› " } else { "  " };
            lines.push(Line::styled(format!("{marker}{name}"), style));
        }
    }
    lines.push(Line::raw(""));

    // Editable: default Agent.
    let agent_label = form
        .default_agent
        .map(|a| a.label().to_string())
        .unwrap_or_else(|| "(global default)".to_string());
    lines.push(field_line(
        theme,
        "Agent",
        &agent_label,
        form.field == F::Agent,
    ));
    lines.push(Line::raw(""));

    // Delete action row.
    let del_style = if form.field == F::Delete {
        Style::new()
            .fg(theme.base.unwrap_or(Color::Black))
            .bg(theme.error)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.error)
    };
    lines.push(Line::styled(" Delete project ", del_style));
    lines.push(Line::raw(""));

    if let Some(err) = &form.error {
        lines.push(Line::styled(err.clone(), Style::new().fg(theme.error)));
        lines.push(Line::raw(""));
    }

    let hint = if pending.is_some() {
        "Enter: create the directory   Esc: edit"
    } else {
        match form.field {
            F::Root => "↑/↓ choose · Tab complete · ↵ save · Esc cancel",
            F::Agent => "←/→ agent · Tab field · ↵ save · Esc cancel",
            F::Delete => "↵ delete project · Tab field · Esc cancel",
            F::Name => "Tab/Shift-Tab field · ↵ save · Esc cancel",
        }
    };
    if let Some(msg) = &pending {
        lines.push(Line::styled(msg.clone(), Style::new().fg(theme.error)));
        lines.push(Line::raw(""));
    }
    lines.push(Line::styled(hint.to_string(), Style::new().fg(theme.muted)));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

pub fn render_help(frame: &mut Frame, theme: &Theme) {
    let area = centered_rect(50, 60, frame.area());
    frame.render_widget(Clear, area);
    let block = themed_block(theme, " Help ".to_string());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let text = "\
↑/↓ j/k   select ticket
←/→ h/l   change column
c         create ticket (auto-starts a background session)
e         edit ticket
Enter     attach / start session
s         open main session (project-wide, no ticket)
/         search / filter tickets (Esc clears)
m         move ticket (then ←/→, Enter)
d         delete ticket
Space     toggle multi-select on a ticket
Shift+D   close selected tickets (or the focused one)
t         switch theme (live preview)
a         set default agent
w         set worktree location (where worktrees are created)
u         update kamaji (shown when a new version is available)
p         switch project
P         project settings (edit / delete)
?         this help
q         quit

Any key closes this help.";
    frame.render_widget(
        Paragraph::new(text).style(Style::new().fg(theme.text)),
        inner,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;
    use ratatui::Terminal;

    #[test]
    fn confirm_modal_border_uses_theme() {
        let theme = Theme::by_name("nord");
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_confirm(f, &theme, "T", "body"))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        // Some cell must carry the theme's border color (the modal frame).
        let found = (0..buf.area.height)
            .any(|y| (0..buf.area.width).any(|x| buf[Position::new(x, y)].fg == theme.border));
        assert!(
            found,
            "confirm modal should draw its border in theme.border"
        );
    }

    #[test]
    fn agent_picker_lists_agents_and_titled_border() {
        let theme = Theme::by_name("nord");
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_agent_picker(f, &theme, 0))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf[Position::new(x, y)].symbol());
            }
        }
        assert!(text.contains("Default agent"), "titled border:\n{text}");
        for agent in Agent::all() {
            assert!(
                text.contains(agent.label()),
                "lists {}:\n{text}",
                agent.label()
            );
        }
        let bordered = (0..buf.area.height)
            .any(|y| (0..buf.area.width).any(|x| buf[Position::new(x, y)].fg == theme.border));
        assert!(bordered, "agent picker should draw its themed border");
    }

    #[test]
    fn help_lists_the_search_key() {
        let theme = Theme::by_name("catppuccin");
        let backend = TestBackend::new(60, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_help(f, &theme)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf[Position::new(x, y)].symbol());
            }
        }
        assert!(
            text.contains("search"),
            "help should mention search:\n{text}"
        );
    }

    #[test]
    fn help_lists_the_main_session_key() {
        let theme = Theme::by_name("catppuccin");
        let backend = TestBackend::new(60, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_help(f, &theme)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf[Position::new(x, y)].symbol());
            }
        }
        assert!(
            text.contains("main session"),
            "help should mention the main session:\n{text}"
        );
    }

    #[test]
    fn help_lists_the_multi_select_keys() {
        let theme = Theme::by_name("catppuccin");
        let backend = TestBackend::new(60, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_help(f, &theme)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf[Position::new(x, y)].symbol());
            }
        }
        assert!(
            text.contains("Space") && text.contains("Shift+D"),
            "help should document the Space (select) and Shift+D (close) keys:\n{text}"
        );
    }

    #[test]
    fn field_modal_draws_suggestions() {
        let theme = Theme::by_name("catppuccin");
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let suggestions = ["kamaji".to_string(), "kafka".to_string()];
        terminal
            .draw(|f| {
                render_field_modal(
                    f,
                    &theme,
                    "New project",
                    &[("Name", "x", false), ("Root", "~/dev/kam", true)],
                    "hint",
                    None,
                    (&suggestions, 0),
                )
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf[Position::new(x, y)].symbol());
            }
        }
        assert!(
            text.contains("kamaji"),
            "suggestion list should render:\n{text}"
        );
        assert!(text.contains("kafka"));
    }

    #[test]
    fn field_modal_windows_suggestions_to_five() {
        let theme = Theme::by_name("catppuccin");
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let suggestions: Vec<String> = (0..8).map(|i| format!("dir{i}")).collect();
        terminal
            .draw(|f| {
                render_field_modal(
                    f,
                    &theme,
                    "New project",
                    &[("Root", "~/", true)],
                    "hint",
                    None,
                    (&suggestions, 7), // last entry selected
                )
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf[Position::new(x, y)].symbol());
            }
        }
        // Selected entry is visible; an early entry has scrolled out.
        assert!(
            text.contains("dir7"),
            "selected entry must be visible:\n{text}"
        );
        assert!(
            !text.contains("dir0"),
            "early entry should scroll out of the 5-window:\n{text}"
        );
        // At most 5 of the dirN labels are rendered.
        let visible = (0..8).filter(|i| text.contains(&format!("dir{i}"))).count();
        assert!(
            visible <= 5,
            "at most 5 suggestions visible, saw {visible}:\n{text}"
        );
    }

    #[test]
    fn project_settings_modal_lists_properties_and_delete_action() {
        use crate::app::ProjectSettingsForm;
        use kamaji_core::models::{Agent, Project};
        let theme = Theme::by_name("catppuccin");
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let p = Project {
            id: 7,
            name: "kamaji".into(),
            root_dir: "/home/u/dev/kamaji".into(),
            default_agent: Some(Agent::Claude),
            created_at: "2026-06-10T12:00:00Z".into(),
        };
        let form = ProjectSettingsForm::from_project(&p);
        terminal
            .draw(|f| render_project_settings(f, &theme, &form))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf[Position::new(x, y)].symbol());
            }
        }
        assert!(text.contains("Project settings"), "titled:\n{text}");
        assert!(text.contains("kamaji"), "name shown:\n{text}");
        assert!(
            text.contains("/home/u/dev/kamaji"),
            "root (basepath) shown:\n{text}"
        );
        assert!(text.contains("Claude"), "default agent shown:\n{text}");
        assert!(
            text.contains("Delete project"),
            "delete action row shown:\n{text}"
        );
    }

    #[test]
    fn form_renders_multiline_description_across_rows() {
        use crate::app::{FormField, TicketForm};
        let theme = Theme::by_name("nord");
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut form = TicketForm::new_create(Agent::Claude);
        form.title = "Title".into();
        form.description = "first line\nsecond line".into();
        form.field = FormField::Description;
        terminal.draw(|f| render_form(f, &theme, &form)).unwrap();
        let buf = terminal.backend().buffer().clone();
        // Each embedded newline starts its own row, so the two segments land on
        // different terminal rows.
        let row_of = |needle: &str| {
            (0..buf.area.height).find(|&y| {
                let row: String = (0..buf.area.width)
                    .map(|x| buf[Position::new(x, y)].symbol())
                    .collect();
                row.contains(needle)
            })
        };
        let first = row_of("first line").expect("first line rendered");
        let second = row_of("second line").expect("second line rendered");
        assert!(
            second > first,
            "the newline should push 'second line' onto a later row (first={first}, second={second})"
        );
    }

    #[test]
    fn worktree_location_modal_shows_title_and_value() {
        let theme = Theme::by_name("catppuccin");
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let form = WorktreeForm::new(Some("~/code/worktrees"));
        terminal
            .draw(|f| render_worktree_location(f, &theme, &form))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf[Position::new(x, y)].symbol());
            }
        }
        assert!(
            text.contains("Worktree location"),
            "modal should be titled:\n{text}"
        );
        assert!(
            text.contains("~/code/worktrees"),
            "the pre-filled location should render:\n{text}"
        );
    }
}
