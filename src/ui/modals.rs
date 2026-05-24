use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{FormField, TicketForm};
use crate::models::{Agent, Status};
use crate::theme::Theme;
use crate::ui::centered_rect;

/// A rounded modal frame titled `title`, bordered in the theme's border color.
fn themed_block(theme: &Theme, title: String) -> Block<'static> {
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
) {
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
        field_line(
            theme,
            "Description",
            &form.description,
            form.field == FormField::Description,
        ),
    ];
    if form.editing_id.is_none() {
        lines.push(Line::raw(""));
        lines.push(field_line(
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
                    Style::new().fg(theme.base.unwrap_or(Color::Black)).bg(theme.accent())
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
        "Tab/Shift-Tab: field   ←/→: agent / toggle   Enter: save   Esc: cancel",
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
m         move ticket (then ←/→, Enter)
d         delete ticket
t         switch theme (live preview)
p         switch project
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
        let found = (0..buf.area.height).any(|y| {
            (0..buf.area.width).any(|x| buf[Position::new(x, y)].fg == theme.border)
        });
        assert!(found, "confirm modal should draw its border in theme.border");
    }
}
