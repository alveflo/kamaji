use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};
use ratatui::Frame;

use std::collections::HashMap;

use crate::app::App;
use crate::detect::SignalLevel;
use crate::models::{Status, Ticket};

/// A card occupies its border lines plus one content line.
const CARD_HEIGHT: u16 = 3;
/// Blank line between stacked cards.
const CARD_GAP: u16 = 1;
/// Bullet color for the "Needs attention" column (true orange; truecolor).
const ORANGE: Color = Color::Rgb(255, 165, 0);

/// The fg color to apply to a ticket's bullet, or `None` to inherit the card's
/// existing text style (the default appearance). Needs-attention tickets are
/// always orange; an In Progress ticket whose agent is actively working is
/// green, and otherwise (idle/unknown/no signal) keeps its default color.
fn bullet_color(status: Status, level: Option<SignalLevel>) -> Option<Color> {
    match status {
        Status::Review => Some(ORANGE),
        Status::InProgress if level == Some(SignalLevel::Active) => Some(Color::Green),
        _ => None,
    }
}

pub fn render_board(frame: &mut Frame, app: &App, levels: &HashMap<i64, SignalLevel>) {
    let [board_area, status_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());

    let columns = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(board_area);

    for (col_idx, status) in Status::all().into_iter().enumerate() {
        let tickets = app.column_tickets(status);
        let focused = col_idx == app.selected_col;
        render_column(
            frame,
            columns[col_idx],
            status,
            &tickets,
            focused,
            app.selected_row,
            levels,
        );
    }

    let hints = " [↵]attach [e]dit [c]reate [m]ove [d]elete [p]roject [?]help [q]uit";
    let left = format!(" project: {} ", app.project.name);
    let msg = app.status_message.clone().unwrap_or_default();
    let status_line = Paragraph::new(Line::from(vec![
        Span::styled(left, Style::new().fg(Color::Yellow)),
        Span::styled(msg, Style::new().fg(Color::Red)),
        Span::raw(hints),
    ]));
    frame.render_widget(status_line, status_area);
}

/// Render one Kanban column: a bordered frame holding the ticket count in its
/// title and the tickets as vertically stacked cards.
fn render_column(
    frame: &mut Frame,
    area: Rect,
    status: Status,
    tickets: &[&Ticket],
    focused: bool,
    selected_row: usize,
    levels: &HashMap<i64, SignalLevel>,
) {
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
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if tickets.is_empty() || inner.height == 0 {
        return;
    }

    let visible = visible_cards(inner.height);
    let offset = if focused {
        first_visible(selected_row, visible, tickets.len())
    } else {
        0
    };

    let slot = CARD_HEIGHT + CARD_GAP;
    let bottom = inner.y + inner.height;
    for (i, ticket) in tickets.iter().enumerate().skip(offset) {
        let y = inner.y + (i - offset) as u16 * slot;
        if y >= bottom {
            break;
        }
        // Clip the final card to whatever vertical space remains.
        let height = CARD_HEIGHT.min(bottom - y);
        let card = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height,
        };
        let selected = focused && i == selected_row;
        let level = levels.get(&ticket.id).copied();
        render_card(frame, card, ticket, selected, level);
    }
}

/// How many whole cards fit in a column of the given inner height.
fn visible_cards(inner_height: u16) -> usize {
    if inner_height < CARD_HEIGHT {
        return 1; // show one (clipped) card rather than nothing
    }
    let slot = CARD_HEIGHT + CARD_GAP;
    // n cards need n*CARD_HEIGHT + (n-1)*CARD_GAP <= inner_height, i.e.
    // n <= (inner_height + CARD_GAP) / slot.
    ((inner_height + CARD_GAP) / slot) as usize
}

/// Index of the first card to draw so that `selected` stays on screen, given
/// that `visible` cards fit at once. Anchors the selection to the bottom of the
/// view once it scrolls past the first page.
fn first_visible(selected: usize, visible: usize, total: usize) -> usize {
    if visible == 0 || total == 0 {
        return 0;
    }
    if selected < visible {
        0
    } else {
        // Keep selected as the last visible card.
        (selected + 1 - visible).min(total.saturating_sub(visible))
    }
}

/// Render a single ticket as a bordered, padded card. The selected card gets a
/// filled accent background and a bold accent border.
fn render_card(
    frame: &mut Frame,
    area: Rect,
    ticket: &Ticket,
    selected: bool,
    level: Option<SignalLevel>,
) {
    let marker = if ticket.session_name.is_some() {
        "●"
    } else {
        "○"
    };

    let (fill, border_style, text_style) = if selected {
        (
            Style::new().bg(Color::Cyan),
            Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
            Style::new()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::new(),
            Style::new().fg(Color::DarkGray),
            Style::new().fg(Color::Gray),
        )
    };

    let block = Block::bordered()
        .border_style(border_style)
        .style(fill)
        .padding(Padding::horizontal(1));

    // The bullet carries the ticket's status/activity color (patched over the
    // card's text style, so a selected card keeps its background and bold);
    // everything else inherits the card's text style unchanged.
    let marker_span = match bullet_color(ticket.status, level) {
        Some(c) => Span::styled(marker, Style::new().fg(c)),
        None => Span::raw(marker),
    };

    let line = Line::from(vec![
        marker_span,
        Span::raw(format!(" #{} ", ticket.id)),
        Span::raw(ticket.title.clone()),
    ])
    .style(text_style);

    let paragraph = Paragraph::new(line).block(block);
    frame.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::models::{Agent, Project};
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;
    use ratatui::Terminal;
    use std::path::PathBuf;

    #[test]
    fn bullet_color_maps_status_and_activity() {
        // Needs attention (Review) is always orange, regardless of activity.
        assert_eq!(bullet_color(Status::Review, None), Some(ORANGE));
        assert_eq!(
            bullet_color(Status::Review, Some(SignalLevel::Active)),
            Some(ORANGE)
        );
        // In Progress is green only while the agent is actively working.
        assert_eq!(
            bullet_color(Status::InProgress, Some(SignalLevel::Active)),
            Some(Color::Green)
        );
        // Stale In Progress: idle, unknown, or no signal => default color.
        assert_eq!(
            bullet_color(Status::InProgress, Some(SignalLevel::Idle)),
            None
        );
        assert_eq!(
            bullet_color(Status::InProgress, Some(SignalLevel::Unknown)),
            None
        );
        assert_eq!(bullet_color(Status::InProgress, None), None);
        // Other columns keep their default color.
        assert_eq!(bullet_color(Status::Todo, Some(SignalLevel::Active)), None);
        assert_eq!(bullet_color(Status::Done, None), None);
    }

    #[test]
    fn first_visible_keeps_selection_on_screen() {
        // First page: no scrolling.
        assert_eq!(first_visible(0, 3, 10), 0);
        assert_eq!(first_visible(2, 3, 10), 0);
        // Past the page: selection anchored to the last visible slot.
        assert_eq!(first_visible(3, 3, 10), 1);
        assert_eq!(first_visible(9, 3, 10), 7);
        // Degenerate inputs.
        assert_eq!(first_visible(0, 0, 10), 0);
        assert_eq!(first_visible(0, 3, 0), 0);
    }

    #[test]
    fn visible_cards_counts_whole_cards() {
        assert_eq!(visible_cards(0), 1); // clipped single card
        assert_eq!(visible_cards(CARD_HEIGHT), 1);
        assert_eq!(visible_cards(CARD_HEIGHT + CARD_GAP + CARD_HEIGHT), 2);
    }

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
            title: format!("title{id}"),
            description: String::new(),
            initial_prompt: None,
            agent: Agent::Claude,
            status,
            position: 0,
            session_name: None,
            worktree_path: None,
            branch: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn render(
        app: &App,
        levels: &HashMap<i64, SignalLevel>,
        w: u16,
        h: u16,
    ) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_board(f, app, levels)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// fg color of the first bullet cell (`●`/`○`) found in the buffer.
    fn bullet_fg(buf: &ratatui::buffer::Buffer) -> Option<Color> {
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let sym = buf[Position::new(x, y)].symbol();
                if sym == "●" || sym == "○" {
                    return Some(buf[Position::new(x, y)].fg);
                }
            }
        }
        None
    }

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[Position::new(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn renders_tickets_as_cards_with_borders() {
        let app = App::new(project(), vec![ticket(1, Status::Todo)]);
        let buf = render(&app, &HashMap::new(), 80, 20);
        let text = buffer_text(&buf);
        // Card content present.
        assert!(text.contains("#1"), "expected ticket id in:\n{text}");
        assert!(text.contains("title1"), "expected title in:\n{text}");
        // Card border characters present (box drawing).
        assert!(
            text.contains('┌') && text.contains('└'),
            "expected card borders in:\n{text}"
        );
    }

    #[test]
    fn selected_card_has_filled_background() {
        let app = App::new(project(), vec![ticket(1, Status::Todo)]);
        let buf = render(&app, &HashMap::new(), 80, 20);
        let has_cyan_bg = (0..buf.area.height)
            .any(|y| (0..buf.area.width).any(|x| buf[Position::new(x, y)].bg == Color::Cyan));
        assert!(
            has_cyan_bg,
            "selected card should have a colored background"
        );
    }

    /// A ticket with a recorded session in the given column.
    fn live_ticket(id: i64, status: Status) -> Ticket {
        let mut t = ticket(id, status);
        t.session_name = Some(format!("sess{id}"));
        t
    }

    #[test]
    fn needs_attention_bullet_is_orange() {
        let app = App::new(project(), vec![live_ticket(1, Status::Review)]);
        let buf = render(&app, &HashMap::new(), 80, 20);
        assert_eq!(bullet_fg(&buf), Some(ORANGE));
    }

    #[test]
    fn in_progress_active_bullet_is_green() {
        let app = App::new(project(), vec![live_ticket(1, Status::InProgress)]);
        let mut levels = HashMap::new();
        levels.insert(1, SignalLevel::Active);
        let buf = render(&app, &levels, 80, 20);
        assert_eq!(bullet_fg(&buf), Some(Color::Green));
    }

    #[test]
    fn in_progress_idle_bullet_keeps_default_color() {
        let app = App::new(project(), vec![live_ticket(1, Status::InProgress)]);
        let mut levels = HashMap::new();
        levels.insert(1, SignalLevel::Idle);
        let buf = render(&app, &levels, 80, 20);
        // Stale: the bullet keeps the default unselected card color, not green.
        assert_eq!(bullet_fg(&buf), Some(Color::Gray));
    }

    #[test]
    fn overflowing_column_keeps_selection_visible_without_panic() {
        let tickets: Vec<Ticket> = (1..=20).map(|i| ticket(i, Status::Todo)).collect();
        let mut app = App::new(project(), tickets);
        app.selected_row = 19; // bottom-most card

        // Small height forces scrolling; should not panic and the selected
        // ticket must be on screen.
        let buf = render(&app, &HashMap::new(), 80, 12);
        let text = buffer_text(&buf);
        assert!(
            text.contains("#20"),
            "selected card should be visible:\n{text}"
        );
    }
}
