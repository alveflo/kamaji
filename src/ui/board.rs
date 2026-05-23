use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::models::Status;

/// Index of the first card to draw so the selected card stays visible.
///
/// `selected` is the selected card's index, `visible` is how many cards fit in
/// the column's inner area. Scrolls the minimum amount: nothing while the
/// selection is on the first page, then enough to pin the selection to the
/// bottom visible row.
fn scroll_offset(selected: usize, visible: usize) -> usize {
    if visible == 0 || selected < visible {
        0
    } else {
        selected + 1 - visible
    }
}

pub fn render_board(frame: &mut Frame, app: &App) {
    let [board_area, status_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());

    let columns = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(board_area);

    // Card geometry: a bordered box (top border, one content row, bottom
    // border) with a blank row separating it from the next card.
    const CARD_HEIGHT: u16 = 3;
    const CARD_GAP: u16 = 1;
    const CARD_STRIDE: u16 = CARD_HEIGHT + CARD_GAP;

    for (col_idx, status) in Status::all().into_iter().enumerate() {
        let tickets = app.column_tickets(status);
        let focused = col_idx == app.selected_col;
        let col_area = columns[col_idx];

        let column_border = if focused {
            Style::new().fg(Color::Cyan)
        } else {
            Style::new().fg(Color::DarkGray)
        };
        let column = Block::bordered().border_style(column_border).title(format!(
            " {} ({}) ",
            status.title(),
            tickets.len()
        ));
        let inner = column.inner(col_area);
        frame.render_widget(column, col_area);

        if tickets.is_empty() || inner.height < CARD_HEIGHT {
            continue;
        }

        // How many cards fit, and how far to scroll so the selection stays in
        // view. `visible` is >= 1 because inner.height >= CARD_HEIGHT here.
        let visible = ((inner.height + CARD_GAP) / CARD_STRIDE) as usize;
        let selected = app.selected_row.min(tickets.len() - 1);
        let offset = if focused {
            scroll_offset(selected, visible)
        } else {
            0
        };

        for (slot, idx) in (offset..tickets.len()).take(visible).enumerate() {
            let t = tickets[idx];
            let card_area = Rect {
                x: inner.x,
                y: inner.y + slot as u16 * CARD_STRIDE,
                width: inner.width,
                height: CARD_HEIGHT,
            };
            let is_selected = focused && idx == selected;

            let (card_style, card_border) = if is_selected {
                (
                    Style::new()
                        .bg(Color::Blue)
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                    Style::new().fg(Color::Cyan),
                )
            } else {
                (Style::new(), Style::new().fg(Color::DarkGray))
            };

            let active = t.session_name.is_some();
            let marker_fg = if active {
                Color::Green
            } else {
                Color::DarkGray
            };
            let mut marker_style = Style::new().fg(marker_fg);
            if is_selected {
                marker_style = marker_style.bg(Color::Blue);
            }

            let card = Block::bordered()
                .border_style(card_border)
                .style(card_style)
                .padding(Padding::horizontal(1));
            let line = Line::from(vec![
                Span::styled(if active { "●" } else { "○" }, marker_style),
                Span::raw(format!(" #{} {}", t.id, t.title)),
            ]);
            frame.render_widget(Paragraph::new(line).block(card), card_area);
        }
    }

    let hints = " [c]reate [m]ove [a]ttach [o]pen [d]elete [p]roject [?]help [q]uit";
    let left = format!(" project: {} ", app.project.name);
    let msg = app.status_message.clone().unwrap_or_default();
    let status_line = Paragraph::new(Line::from(vec![
        Span::styled(left, Style::new().fg(Color::Yellow)),
        Span::styled(msg, Style::new().fg(Color::Red)),
        Span::raw(hints),
    ]));
    frame.render_widget(status_line, status_area);
}

#[cfg(test)]
mod tests {
    use super::{render_board, scroll_offset};
    use crate::app::App;
    use crate::models::{Agent, Project, Status, Ticket};
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;
    use ratatui::Terminal;
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

    fn ticket(id: i64, status: Status, active: bool) -> Ticket {
        Ticket {
            id,
            project_id: 1,
            title: format!("t{id}"),
            description: String::new(),
            initial_prompt: None,
            agent: Agent::Claude,
            status,
            position: 0,
            session_name: active.then(|| format!("s{id}")),
            worktree_path: None,
            branch: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn render(app: &App, w: u16, h: u16) -> Buffer {
        let backend = ratatui::backend::TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render_board(f, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn text(buf: &Buffer) -> String {
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    /// Rows (top to bottom) that contain at least one blue-background cell —
    /// i.e. where the selected card is drawn.
    fn highlighted_rows(buf: &Buffer) -> Vec<u16> {
        let mut rows: Vec<u16> = buf
            .content
            .iter()
            .enumerate()
            .filter(|(_, c)| c.bg == Color::Blue)
            .map(|(i, _)| i as u16 / buf.area.width)
            .collect();
        rows.sort_unstable();
        rows.dedup();
        rows
    }

    #[test]
    fn renders_each_ticket_as_a_bordered_card() {
        let app = App::new(
            project(),
            vec![
                ticket(1, Status::Todo, false),
                ticket(2, Status::Todo, true),
            ],
        );
        let s = text(&render(&app, 80, 24));
        assert!(s.contains("#1 t1"), "card for ticket 1 missing: {s:?}");
        assert!(s.contains("#2 t2"), "card for ticket 2 missing");
        assert!(s.contains('●'), "active-session marker missing");
        assert!(s.contains('○'), "idle-session marker missing");
        assert!(
            s.contains('┌') && s.contains('└'),
            "no bordered cards drawn"
        );
    }

    #[test]
    fn selected_card_has_accent_background_that_follows_selection() {
        let mut app = App::new(
            project(),
            vec![
                ticket(1, Status::Todo, false),
                ticket(2, Status::Todo, false),
            ],
        );
        let top = highlighted_rows(&render(&app, 80, 24));
        app.down();
        let bottom = highlighted_rows(&render(&app, 80, 24));

        assert!(!top.is_empty(), "selected card has no colored background");
        assert!(
            !bottom.is_empty(),
            "selected card has no colored background"
        );
        assert!(
            bottom[0] > top[0],
            "highlight should move down with the selection: {top:?} -> {bottom:?}"
        );
    }

    #[test]
    fn overflow_scrolls_to_keep_the_selection_visible() {
        let tickets: Vec<Ticket> = (1..=8).map(|i| ticket(i, Status::Todo, false)).collect();
        let mut app = App::new(project(), tickets);
        for _ in 0..7 {
            app.down(); // select the last card
        }
        // 12 rows total leaves room for only ~2 cards in the Todo column.
        let s = text(&render(&app, 80, 12));
        assert!(
            s.contains("#8 t8"),
            "selected last card must stay visible: {s:?}"
        );
        assert!(
            !s.contains("#1 t1"),
            "first card should have scrolled out of view: {s:?}"
        );
    }

    #[test]
    fn no_scroll_when_selection_within_first_page() {
        // visible=4: indices 0..=3 fit, so a selection there needs no scroll.
        assert_eq!(scroll_offset(2, 4), 0);
    }

    #[test]
    fn selection_just_past_the_page_scrolls_one() {
        // visible=4: index 4 is the first that doesn't fit on page one.
        assert_eq!(scroll_offset(4, 4), 1);
    }

    #[test]
    fn scrolls_to_pin_selection_to_bottom_row() {
        // visible=4, selection 5 → show 2,3,4,5 with 5 on the last row.
        assert_eq!(scroll_offset(5, 4), 2);
    }

    #[test]
    fn scrolls_to_last_card() {
        assert_eq!(scroll_offset(9, 4), 6);
    }

    #[test]
    fn zero_visible_is_offset_zero() {
        assert_eq!(scroll_offset(3, 0), 0);
    }
}
