use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::models::Status;

pub fn render_board(frame: &mut Frame, app: &App) {
    let [board_area, status_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());

    let columns = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(board_area);

    for (col_idx, status) in Status::all().into_iter().enumerate() {
        let tickets = app.column_tickets(status);
        let focused = col_idx == app.selected_col;

        let items: Vec<ListItem> = tickets
            .iter()
            .map(|t| {
                let marker = if t.session_name.is_some() {
                    "●"
                } else {
                    "○"
                };
                ListItem::new(format!("{marker} #{} {}", t.id, t.title))
            })
            .collect();

        let border_style = if focused {
            Style::new().fg(Color::Cyan)
        } else {
            Style::new().fg(Color::DarkGray)
        };

        let block = Block::bordered().border_style(border_style).title(format!(
            " {} ({}) ",
            status.title(),
            tickets.len()
        ));

        let mut state = ListState::default();
        if focused && !tickets.is_empty() {
            state.select(Some(app.selected_row.min(tickets.len() - 1)));
        }

        let list = List::new(items).block(block).highlight_style(
            Style::new()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

        frame.render_stateful_widget(list, columns[col_idx], &mut state);
    }

    let hints = " [c]reate [m]ove [a]ttach [o]pen [d]elete [?]help [q]uit";
    let left = format!(" project: {} ", app.project.name);
    let msg = app.status_message.clone().unwrap_or_default();
    let status_line = Paragraph::new(Line::from(vec![
        Span::styled(left, Style::new().fg(Color::Yellow)),
        Span::styled(msg, Style::new().fg(Color::Red)),
        Span::raw(hints),
    ]));
    frame.render_widget(status_line, status_area);
}
