use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph};
use ratatui::Frame;

use std::collections::{HashMap, HashSet};

use crate::app::{App, StatusKind};
use crate::detect::SignalLevel;
use crate::models::{Status, Ticket};
use crate::theme::Theme;

/// A card occupies its border lines plus one content line.
const CARD_HEIGHT: u16 = 3;
/// Blank line between stacked cards.
const CARD_GAP: u16 = 1;

/// Per-column display parameters passed to `render_column`. Bundling them
/// avoids the `too_many_arguments` lint.
struct ColumnParams<'a> {
    /// Total tickets in the column, ignoring the active search filter.
    total: usize,
    /// `true` when a non-empty search query is active.
    filtering: bool,
    /// Whether this column is keyboard-focused.
    focused: bool,
    /// The currently selected card row (used only when `focused`).
    selected_row: usize,
    /// Ticket ids in the multi-select set (marks cards across all columns).
    selected_ids: &'a HashSet<i64>,
}

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

    let columns = Layout::horizontal([Constraint::Fill(1); 4])
        .spacing(1)
        .split(board_area);

    let filtering = !app.search.is_empty();
    for (col_idx, status) in Status::all().into_iter().enumerate() {
        let tickets = app.column_tickets(status);
        let params = ColumnParams {
            total: app.column_total(status),
            filtering,
            focused: col_idx == app.selected_col,
            selected_row: app.selected_row,
            selected_ids: &app.selected_ids,
        };
        render_column(
            frame,
            theme,
            columns[col_idx],
            status,
            &tickets,
            params,
            levels,
        );
    }

    let hints =
        " [↵]attach [s]main [e]dit [c]reate [m]ove [d]elete [space]select [D]close [/]search [t]heme [w]orktree [p]roject [?]help [q]uit";
    let left = format!(" project: {} ", app.project.name);
    // While cards are multi-selected, surface the count so the user knows a bulk
    // close will act on more than the focused card.
    let selected_span = if app.selected_ids.is_empty() {
        Span::raw("")
    } else {
        Span::styled(
            format!("{} selected ", app.selected_ids.len()),
            Style::new().fg(theme.accent()),
        )
    };
    let search_span = if app.search.editing {
        Span::styled(
            format!("search: {}_ ", app.search.query),
            Style::new().fg(theme.accent()),
        )
    } else if !app.search.is_empty() {
        Span::styled(
            format!("filter: {} — Esc to clear ", app.search.query),
            Style::new().fg(theme.accent()),
        )
    } else {
        Span::raw("")
    };
    let update_span = match &app.update {
        Some(v) => Span::styled(
            format!(" New version v{v} available — press [u] to update "),
            Style::new().fg(theme.active),
        ),
        None => Span::raw(""),
    };
    // Errors stay red; ordinary status updates use the (non-alarming) accent.
    let msg_span = match &app.status_message {
        Some(msg) => {
            let color = match msg.kind {
                StatusKind::Error => theme.error,
                StatusKind::Info => theme.accent(),
            };
            Span::styled(msg.text.clone(), Style::new().fg(color))
        }
        None => Span::raw(""),
    };
    let status_line = Paragraph::new(Line::from(vec![
        Span::styled(left, Style::new().fg(theme.accent())),
        selected_span,
        search_span,
        update_span,
        msg_span,
        Span::styled(hints, Style::new().fg(theme.muted)),
    ]));
    frame.render_widget(status_line, status_area);
}

/// Render one Kanban column: a rounded bordered box containing a per-column
/// colored header, a dashed rule, then the tickets as vertically stacked cards.
/// The focused column's box border uses the column accent; unfocused columns use
/// the muted border color.
fn render_column(
    frame: &mut Frame,
    theme: &Theme,
    area: Rect,
    status: Status,
    tickets: &[&Ticket],
    params: ColumnParams,
    levels: &HashMap<i64, SignalLevel>,
) {
    let accent = theme.status_color(status);
    let border_color = if params.focused { accent } else { theme.border };

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border_color))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let [header_area, rule_area, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);

    // Show "matches/total" while a search filter is active, else just the count.
    let count = if params.filtering {
        format!("{}/{}", tickets.len(), params.total)
    } else {
        params.total.to_string()
    };
    let title = format!("{} · {}", status.title().to_uppercase(), count);
    frame.render_widget(
        Paragraph::new(Line::styled(
            title,
            Style::new().fg(accent).add_modifier(Modifier::BOLD),
        )),
        header_area,
    );

    let rule = "┄".repeat(rule_area.width as usize);
    frame.render_widget(
        Paragraph::new(Line::styled(rule, Style::new().fg(theme.border))),
        rule_area,
    );

    if tickets.is_empty() || body.height == 0 {
        return;
    }

    let visible = visible_cards(body.height);
    let offset = if params.focused {
        first_visible(params.selected_row, visible, tickets.len())
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
        let selected = params.focused && i == params.selected_row;
        let multi_selected = params.selected_ids.contains(&ticket.id);
        let level = levels.get(&ticket.id).copied();
        render_card(frame, theme, card, ticket, selected, multi_selected, level);
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

/// Render a single ticket as a rounded card. The selected card gets an accent
/// border and a `surface` fill.
fn render_card(
    frame: &mut Frame,
    theme: &Theme,
    area: Rect,
    ticket: &Ticket,
    selected: bool,
    multi_selected: bool,
    level: Option<SignalLevel>,
) {
    let accent = theme.status_color(ticket.status);

    let (border_color, fill, base_text) = if selected {
        (
            accent,
            Some(theme.surface),
            Style::new()
                .fg(theme.text)
                .bg(theme.surface)
                .add_modifier(Modifier::BOLD),
        )
    } else if multi_selected {
        // A multi-selected card under no cursor still reads as picked: an accent
        // border (no fill) marks it across columns.
        (accent, None, Style::new().fg(theme.text))
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

    // The id deliberately keeps the column's status accent even on
    // unselected (otherwise muted) cards, acting as a small per-column swatch.
    let mut spans = Vec::new();
    if multi_selected {
        spans.push(Span::styled("✓ ", Style::new().fg(accent)));
    }
    spans.push(marker_span);
    spans.push(Span::styled(
        format!(" #{} ", ticket.id),
        Style::new().fg(accent),
    ));
    spans.push(Span::styled(
        ticket.title.clone(),
        Style::new().fg(theme.text),
    ));
    let line = Line::from(spans).style(base_text);

    frame.render_widget(Paragraph::new(line).block(block), area);
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
        assert_eq!(
            bullet_color(&t, Status::InProgress, Some(SignalLevel::Idle)),
            None
        );
        assert_eq!(
            bullet_color(&t, Status::InProgress, Some(SignalLevel::Unknown)),
            None
        );
        assert_eq!(bullet_color(&t, Status::InProgress, None), None);
        assert_eq!(
            bullet_color(&t, Status::Todo, Some(SignalLevel::Active)),
            None
        );
        assert_eq!(bullet_color(&t, Status::Done, None), None);
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
    fn renders_tickets_as_rounded_cards() {
        // Default selection is column 0, row 0 -> the only ticket is selected.
        // Render wide enough that the title fits inside the boxed column's card.
        let app = app_with_theme(vec![ticket(1, Status::Todo)], "catppuccin");
        let buf = render(&app, &HashMap::new(), 120, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("#1"), "expected ticket id in:\n{text}");
        assert!(text.contains("title1"), "expected title in:\n{text}");
        // Cards (and column boxes) use rounded corners.
        assert!(
            text.contains('╭') && text.contains('╰'),
            "expected rounded corners in:\n{text}"
        );
        // The selected card is highlighted in place, not expanded into a thick
        // (larger) box.
        assert!(
            !text.contains('┏') && !text.contains('┗'),
            "selected card must not be a thick box in:\n{text}"
        );
    }

    #[test]
    fn selected_card_has_filled_background() {
        let app = app_with_theme(vec![ticket(1, Status::Todo)], "catppuccin");
        let theme = crate::theme::Theme::by_name("catppuccin");
        let buf = render(&app, &HashMap::new(), 80, 20);
        let has_surface = (0..buf.area.height)
            .any(|y| (0..buf.area.width).any(|x| buf[Position::new(x, y)].bg == theme.surface));
        assert!(
            has_surface,
            "selected card should be filled with theme.surface"
        );
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
        assert!(
            text.contains("#20"),
            "selected card should be visible:\n{text}"
        );
    }

    #[test]
    fn column_title_shows_matches_over_total_when_filtering() {
        let mut app = App::new(
            project(),
            vec![ticket(1, Status::Todo), ticket(2, Status::Todo)],
        );
        // ticket() titles are "title1" / "title2"; "title1" matches only the first.
        app.search.query = "title1".into();
        let buf = render(&app, &HashMap::new(), 80, 20);
        let text = buffer_text(&buf);
        // The restyled header reads "TODO · 1/2" (matches/total) while filtering.
        assert!(
            text.contains("TODO · 1/2"),
            "expected matches/total count in title:\n{text}"
        );
    }

    #[test]
    fn status_bar_shows_search_prompt_while_editing() {
        let mut app = App::new(project(), vec![ticket(1, Status::Todo)]);
        app.search.editing = true;
        app.search.query = "lo".into();
        let buf = render(&app, &HashMap::new(), 80, 20);
        let text = buffer_text(&buf);
        assert!(
            text.contains("search: lo"),
            "expected the search prompt in the status bar:\n{text}"
        );
    }

    #[test]
    fn status_bar_lists_the_search_hint() {
        let app = App::new(project(), vec![ticket(1, Status::Todo)]);
        let buf = render(&app, &HashMap::new(), 120, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("[/]search"), "search hint present:\n{text}");
    }

    #[test]
    fn status_bar_lists_the_main_session_hint() {
        let app = App::new(project(), vec![ticket(1, Status::Todo)]);
        let buf = render(&app, &HashMap::new(), 120, 20);
        let text = buffer_text(&buf);
        assert!(
            text.contains("[s]main"),
            "main-session hint present:\n{text}"
        );
    }

    #[test]
    fn status_bar_shows_update_banner_when_available() {
        let mut app = App::new(project(), vec![ticket(1, Status::Todo)]);
        app.update = Some("0.9.0".to_string());
        let buf = render(&app, &HashMap::new(), 120, 20);
        let text = buffer_text(&buf);
        assert!(text.contains("0.9.0"), "version present:\n{text}");
        assert!(text.contains("[u]"), "update hint present:\n{text}");
    }

    #[test]
    fn visible_cards_counts_whole_cards() {
        assert_eq!(visible_cards(0), 1); // clipped single card
        assert_eq!(visible_cards(CARD_HEIGHT), 1);
        assert_eq!(visible_cards(CARD_HEIGHT + CARD_GAP + CARD_HEIGHT), 2);
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
    fn columns_are_drawn_as_boxes() {
        let app = app_with_theme(vec![ticket(1, Status::Todo)], "catppuccin");
        let buf = render(&app, &HashMap::new(), 80, 20);
        let text = buffer_text(&buf);
        for corner in ['╭', '╮', '╰', '╯'] {
            assert!(
                text.contains(corner),
                "missing column corner {corner} in:\n{text}"
            );
        }
    }

    #[test]
    fn column_header_uses_its_column_color() {
        let app = app_with_theme(vec![ticket(1, Status::Todo)], "catppuccin");
        let theme = crate::theme::Theme::by_name("catppuccin");
        let buf = render(&app, &HashMap::new(), 80, 20);
        // Column 0 starts at x=0: border at x=0, padding at x=1, header text from x=2.
        // Row 0 is the box top border; the header sits on row 1.
        let cell = &buf[Position::new(2, 1)];
        assert_eq!(
            cell.symbol(),
            "T",
            "expected start of 'TODO' header:\n{}",
            buffer_text(&buf)
        );
        assert_eq!(
            cell.fg, theme.todo,
            "header should use the Todo column color"
        );
    }

    #[test]
    fn dashed_rule_is_drawn_under_each_header() {
        let app = app_with_theme(vec![ticket(1, Status::Todo)], "catppuccin");
        let buf = render(&app, &HashMap::new(), 80, 20);
        assert!(
            buffer_text(&buf).contains('┄'),
            "expected a dashed header rule"
        );
    }

    #[test]
    fn unselected_ticket_is_an_outlined_card_without_fill() {
        // Focus the (empty) In Progress column so the Todo ticket is unselected.
        let mut app = app_with_theme(vec![ticket(1, Status::Todo)], "catppuccin");
        app.selected_col = 1;
        let theme = crate::theme::Theme::by_name("catppuccin");
        let buf = render(&app, &HashMap::new(), 80, 20);
        let text = buffer_text(&buf);
        // The ticket renders inside its own bordered card, in addition to the
        // four column boxes (so at least five rounded top-left corners).
        assert!(
            text.matches('╭').count() >= 5,
            "expected a bordered ticket card beyond the column boxes:\n{text}"
        );
        // An unselected card is not filled (only the selected card gets a surface
        // fill; none is selected here).
        let has_surface = (0..buf.area.height)
            .any(|y| (0..buf.area.width).any(|x| buf[Position::new(x, y)].bg == theme.surface));
        assert!(
            !has_surface,
            "an unselected card must not be filled with theme.surface"
        );
    }

    #[test]
    fn multi_selected_card_shows_a_check_glyph() {
        let mut app = app_with_theme(vec![ticket(1, Status::Todo)], "catppuccin");
        app.selected_ids.insert(1);
        let buf = render(&app, &HashMap::new(), 120, 20);
        assert!(
            buffer_text(&buf).contains('✓'),
            "a multi-selected card should show a check glyph:\n{}",
            buffer_text(&buf)
        );
    }

    #[test]
    fn unselected_card_has_no_check_glyph() {
        let app = app_with_theme(vec![ticket(1, Status::Todo)], "catppuccin");
        let buf = render(&app, &HashMap::new(), 120, 20);
        assert!(
            !buffer_text(&buf).contains('✓'),
            "an unselected card must not show a check glyph"
        );
    }

    #[test]
    fn status_bar_shows_the_selected_count() {
        let mut app = app_with_theme(
            vec![ticket(1, Status::Todo), ticket(2, Status::Todo)],
            "catppuccin",
        );
        app.selected_ids.insert(1);
        app.selected_ids.insert(2);
        let buf = render(&app, &HashMap::new(), 120, 20);
        assert!(
            buffer_text(&buf).contains("2 selected"),
            "status bar should show the selected count:\n{}",
            buffer_text(&buf)
        );
    }
}
