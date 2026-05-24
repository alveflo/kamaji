use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph};
use ratatui::Frame;

use std::collections::HashMap;

use crate::app::App;
use crate::detect::SignalLevel;
use crate::models::{Status, Ticket};
use crate::theme::Theme;

/// A card occupies its border lines plus one content line.
const CARD_HEIGHT: u16 = 3;
/// Blank line between stacked cards.
const CARD_GAP: u16 = 1;

/// The fg color for a ticket's bullet, or `None` to inherit the card's text
/// style. Needs-attention bullets use the attention color; an actively working
/// In Progress bullet uses the active color; otherwise it inherits (idle).
fn bullet_color(theme: &Theme, status: Status, level: Option<SignalLevel>) -> Option<Color> {
    match status {
        Status::Review => Some(theme.attention),
        Status::InProgress if level == Some(SignalLevel::Active) => Some(theme.active),
        _ => None,
    }
}

pub fn render_board(frame: &mut Frame, app: &App, levels: &HashMap<i64, SignalLevel>) {
    let theme = &app.theme;

    // Paint the themed background (skip in default mode to keep the terminal's).
    if let Some(bg) = theme.base {
        frame.render_widget(Block::default().style(Style::new().bg(bg)), frame.area());
    }

    let [board_area, status_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());

    let columns = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(board_area);

    for (col_idx, status) in Status::all().into_iter().enumerate() {
        let tickets = app.column_tickets(status);
        let focused = col_idx == app.selected_col;
        render_column(
            frame,
            theme,
            columns[col_idx],
            status,
            &tickets,
            focused,
            app.selected_row,
            levels,
        );
    }

    let hints = " [↵]attach [e]dit [c]reate [m]ove [d]elete [t]heme [p]roject [?]help [q]uit";
    let left = format!(" project: {} ", app.project.name);
    let msg = app.status_message.clone().unwrap_or_default();
    let status_line = Paragraph::new(Line::from(vec![
        Span::styled(left, Style::new().fg(theme.accent())),
        Span::styled(msg, Style::new().fg(theme.error)),
        Span::styled(hints, Style::new().fg(theme.muted)),
    ]));
    frame.render_widget(status_line, status_area);
}

/// Render one Kanban column: a colored header (`TITLE · n`) and rule, then the
/// tickets as vertically stacked cards. The focused column's header is drawn in
/// the status accent; unfocused columns use the muted color.
#[allow(clippy::too_many_arguments)]
fn render_column(
    frame: &mut Frame,
    theme: &Theme,
    area: Rect,
    status: Status,
    tickets: &[&Ticket],
    focused: bool,
    selected_row: usize,
    levels: &HashMap<i64, SignalLevel>,
) {
    let accent = theme.status_color(status);
    let header_color = if focused { accent } else { theme.muted };

    let [header_area, rule_area, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);

    let title = format!(
        " {} · {}",
        status.title().to_uppercase(),
        tickets.len()
    );
    let mut header_style = Style::new().fg(header_color);
    if focused {
        header_style = header_style.add_modifier(Modifier::BOLD);
    }
    frame.render_widget(
        Paragraph::new(Line::styled(title, header_style)),
        header_area,
    );
    let rule = "─".repeat(rule_area.width as usize);
    frame.render_widget(
        Paragraph::new(Line::styled(rule, Style::new().fg(header_color))),
        rule_area,
    );

    if tickets.is_empty() || body.height == 0 {
        return;
    }

    let visible = visible_cards(body.height);
    let offset = if focused {
        first_visible(selected_row, visible, tickets.len())
    } else {
        0
    };

    let slot = CARD_HEIGHT + CARD_GAP;
    let bottom = body.y + body.height;
    for (i, ticket) in tickets.iter().enumerate().skip(offset) {
        let y = body.y + (i - offset) as u16 * slot;
        if y >= bottom {
            break;
        }
        let height = CARD_HEIGHT.min(bottom - y);
        let card = Rect {
            x: body.x,
            y,
            width: body.width,
            height,
        };
        let selected = focused && i == selected_row;
        let level = levels.get(&ticket.id).copied();
        render_card(frame, theme, card, ticket, selected, level);
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

/// Render a single ticket as a rounded card with a colored left accent strip.
/// The selected card gets an accent border and a `surface` fill.
fn render_card(
    frame: &mut Frame,
    theme: &Theme,
    area: Rect,
    ticket: &Ticket,
    selected: bool,
    level: Option<SignalLevel>,
) {
    let accent = theme.status_color(ticket.status);

    // 1-cell accent strip on the far left; the rounded box fills the rest.
    let strip = Rect {
        x: area.x,
        y: area.y,
        width: 1,
        height: area.height,
    };
    let box_area = Rect {
        x: area.x + 1,
        y: area.y,
        width: area.width.saturating_sub(1),
        height: area.height,
    };
    frame.render_widget(Block::default().style(Style::new().bg(accent)), strip);

    let (border_color, fill, base_text) = if selected {
        (
            accent,
            Some(theme.surface),
            Style::new().fg(theme.text).bg(theme.surface).add_modifier(Modifier::BOLD),
        )
    } else {
        (theme.border, None, Style::new().fg(theme.muted))
    };

    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border_color))
        .padding(Padding::horizontal(1));
    if let Some(bg) = fill {
        block = block.style(Style::new().bg(bg));
    }

    let marker = if ticket.session_name.is_some() {
        "●"
    } else {
        "○"
    };
    let marker_span = match bullet_color(theme, ticket.status, level) {
        Some(c) => Span::styled(marker, Style::new().fg(c)),
        None => Span::raw(marker),
    };

    // The id deliberately keeps the column's status accent even on unselected
    // (otherwise muted) cards, acting as a small per-column color swatch.
    let line = Line::from(vec![
        marker_span,
        Span::styled(format!(" #{} ", ticket.id), Style::new().fg(accent)),
        Span::styled(ticket.title.clone(), Style::new().fg(theme.text)),
    ])
    .style(base_text);

    frame.render_widget(Paragraph::new(line).block(block), box_area);
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

    fn app_with_theme(tickets: Vec<Ticket>, theme_name: &str) -> App {
        let mut app = App::new(project(), tickets);
        app.theme = crate::theme::Theme::by_name(theme_name);
        app
    }

    #[test]
    fn bullet_color_maps_status_and_activity() {
        let t = crate::theme::Theme::by_name("catppuccin");
        assert_eq!(bullet_color(&t, Status::Review, None), Some(t.attention));
        assert_eq!(
            bullet_color(&t, Status::Review, Some(SignalLevel::Active)),
            Some(t.attention)
        );
        assert_eq!(
            bullet_color(&t, Status::InProgress, Some(SignalLevel::Active)),
            Some(t.active)
        );
        assert_eq!(bullet_color(&t, Status::InProgress, Some(SignalLevel::Idle)), None);
        assert_eq!(bullet_color(&t, Status::InProgress, Some(SignalLevel::Unknown)), None);
        assert_eq!(bullet_color(&t, Status::InProgress, None), None);
        assert_eq!(bullet_color(&t, Status::Todo, Some(SignalLevel::Active)), None);
        assert_eq!(bullet_color(&t, Status::Done, None), None);
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
            auto_reviewed: false,
            instrumented: false,
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
        let app = app_with_theme(vec![ticket(1, Status::Todo)], "catppuccin");
        let buf = render(&app, &HashMap::new(), 80, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("#1"), "expected ticket id in:\n{text}");
        assert!(text.contains("title1"), "expected title in:\n{text}");
        // Rounded card corners.
        assert!(
            text.contains('╭') && text.contains('╰'),
            "expected rounded card borders in:\n{text}"
        );
    }

    #[test]
    fn selected_card_has_filled_background() {
        let app = app_with_theme(vec![ticket(1, Status::Todo)], "catppuccin");
        let theme = crate::theme::Theme::by_name("catppuccin");
        let buf = render(&app, &HashMap::new(), 80, 20);
        let has_surface = (0..buf.area.height)
            .any(|y| (0..buf.area.width).any(|x| buf[Position::new(x, y)].bg == theme.surface));
        assert!(has_surface, "selected card should be filled with theme.surface");
    }

    /// A ticket with a recorded session in the given column.
    fn live_ticket(id: i64, status: Status) -> Ticket {
        let mut t = ticket(id, status);
        t.session_name = Some(format!("sess{id}"));
        t
    }

    #[test]
    fn needs_attention_bullet_is_attention_color() {
        let app = app_with_theme(vec![live_ticket(1, Status::Review)], "catppuccin");
        let theme = crate::theme::Theme::by_name("catppuccin");
        let buf = render(&app, &HashMap::new(), 80, 20);
        assert_eq!(bullet_fg(&buf), Some(theme.attention));
    }

    #[test]
    fn in_progress_active_bullet_is_active_color() {
        let app = app_with_theme(vec![live_ticket(1, Status::InProgress)], "catppuccin");
        let theme = crate::theme::Theme::by_name("catppuccin");
        let mut levels = HashMap::new();
        levels.insert(1, SignalLevel::Active);
        let buf = render(&app, &levels, 80, 20);
        assert_eq!(bullet_fg(&buf), Some(theme.active));
    }

    #[test]
    fn in_progress_idle_bullet_keeps_default_color() {
        let mut app = app_with_theme(vec![live_ticket(1, Status::InProgress)], "catppuccin");
        // Focus the InProgress column (index 1) so the card is selected.
        app.selected_col = 1;
        let theme = crate::theme::Theme::by_name("catppuccin");
        let mut levels = HashMap::new();
        levels.insert(1, SignalLevel::Idle);
        let buf = render(&app, &levels, 80, 20);
        // Selected idle card: bullet inherits the selected text color (not active/attention).
        assert_eq!(bullet_fg(&buf), Some(theme.text));
    }

    #[test]
    fn overflowing_column_keeps_selection_visible_without_panic() {
        let tickets: Vec<Ticket> = (1..=20).map(|i| ticket(i, Status::Todo)).collect();
        let mut app = app_with_theme(tickets, "catppuccin");
        app.selected_row = 19;
        let buf = render(&app, &HashMap::new(), 80, 12);
        let text = buffer_text(&buf);
        assert!(text.contains("#20"), "selected card should be visible:\n{text}");
    }
}
