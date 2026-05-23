use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{FormField, TicketForm};
use crate::models::{Agent, Status};
use crate::ui::centered_rect;

pub(crate) fn field_line(label: &str, value: &str, active: bool) -> Line<'static> {
    let style = if active {
        Style::new()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::Gray)
    };
    let cursor = if active { "_" } else { "" };
    Line::from(vec![
        Span::styled(format!("{label}: "), Style::new().fg(Color::Yellow)),
        Span::styled(format!("{value}{cursor}"), style),
    ])
}

/// Render a centered, bordered modal form: a list of labelled fields with an
/// active-field highlight, a hint line, and an optional error. Shared by modals
/// (like the new-project form) that want the same look as the ticket modal.
pub(crate) fn render_field_modal(
    frame: &mut Frame,
    title: &str,
    fields: &[(&str, &str, bool)],
    hint: &str,
    error: Option<&str>,
) {
    let area = centered_rect(70, 60, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::bordered()
        .title(format!(" {title} "))
        .border_style(Style::new().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for (i, (label, value, active)) in fields.iter().enumerate() {
        if i > 0 {
            lines.push(Line::raw(""));
        }
        lines.push(field_line(label, value, *active));
    }
    lines.push(Line::raw(""));
    if let Some(err) = error {
        lines.push(Line::styled(err.to_string(), Style::new().fg(Color::Red)));
        lines.push(Line::raw(""));
    }
    lines.push(Line::styled(
        hint.to_string(),
        Style::new().fg(Color::DarkGray),
    ));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

pub fn render_form(frame: &mut Frame, form: &TicketForm) {
    let area = centered_rect(70, 60, frame.area());
    frame.render_widget(Clear, area);

    let title = if form.editing_id.is_some() {
        " Edit ticket "
    } else {
        " New ticket "
    };
    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![
        field_line("Title", &form.title, form.field == FormField::Title),
        Line::raw(""),
        field_line(
            "Description",
            &form.description,
            form.field == FormField::Description,
        ),
    ];
    if form.editing_id.is_none() {
        lines.push(Line::raw(""));
        lines.push(field_line(
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
                    Style::new().fg(Color::Black).bg(Color::Cyan)
                } else if a == form.agent {
                    Style::new().fg(Color::Cyan)
                } else {
                    Style::new().fg(Color::DarkGray)
                };
                vec![
                    Span::styled(format!(" {} ", a.label()), style),
                    Span::raw(" "),
                ]
            })
            .collect();
        let mut agent_line = vec![Span::styled("Agent: ", Style::new().fg(Color::Yellow))];
        agent_line.extend(agents);
        lines.push(Line::from(agent_line));

        lines.push(Line::raw(""));
        let checkbox = if form.start_in_background { "[x]" } else { "[ ]" };
        lines.push(field_line(
            "Start in background",
            checkbox,
            form.field == FormField::Background,
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Tab/Shift-Tab: field   ←/→: agent / toggle   Enter: save   Esc: cancel",
        Style::new().fg(Color::DarkGray),
    ));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

pub fn render_move(frame: &mut Frame, target: Status) {
    let area = centered_rect(60, 25, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title(" Move ticket ")
        .border_style(Style::new().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [cols_area, hint_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(2)]).areas(inner);
    let spans: Vec<Span> = Status::all()
        .into_iter()
        .map(|s| {
            let style = if s == target {
                Style::new()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(Color::Gray)
            };
            Span::styled(format!(" {} ", s.title()), style)
        })
        .collect();
    frame.render_widget(Paragraph::new(Line::from(spans)), cols_area);
    frame.render_widget(
        Paragraph::new("←/→: choose   Enter: confirm   Esc: cancel")
            .style(Style::new().fg(Color::DarkGray)),
        hint_area,
    );
}

pub fn render_confirm(frame: &mut Frame, title: &str, body: &str) {
    let area = centered_rect(50, 20, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title(format!(" {title} "))
        .border_style(Style::new().fg(Color::Yellow));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: true }), inner);
}

pub fn render_help(frame: &mut Frame) {
    let area = centered_rect(50, 60, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title(" Help ")
        .border_style(Style::new().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let text = "\
↑/↓ j/k   select ticket
←/→ h/l   change column
c         create ticket
e         edit ticket
Enter     attach / start session
m         move ticket (then ←/→, Enter)
d         delete ticket
p         switch project
?         this help
q         quit

Any key closes this help.";
    frame.render_widget(Paragraph::new(text), inner);
}
