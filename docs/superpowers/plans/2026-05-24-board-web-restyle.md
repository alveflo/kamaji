# Board Web-Restyle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restyle the kamaji Kanban board TUI (`src/ui/board.rs`) to match the web "board preview" — bordered columns with per-column colored headers + dashed rules, tickets as filled `surface` bars, and the selected ticket as a thick accent-bordered box.

**Architecture:** A pure, unit-tested `card_layout` helper computes variable-height card placements + scrolling (selected card = 3 rows, others = 1 row). `render_column` draws a bordered box (rounded), a per-column colored header, a dashed rule, then the laid-out cards. `render_card` draws either a single-line `surface`-filled bar or, when selected, a thick accent-bordered box. Palette is unchanged.

**Tech Stack:** Rust, ratatui 0.29 (`Block`, `BorderType`, `Padding`, `Paragraph`, `Layout`), `TestBackend` for buffer-level UI tests.

**Reference spec:** `docs/superpowers/specs/2026-05-24-board-web-restyle-design.md`

**Scope note / spec refinement:** The committed spec says the column box border is `theme.border`. During planning we add one small refinement: the **focused** column's box border is drawn in the column accent color (`theme.status_color`), unfocused in `theme.border`. This preserves a focus indicator even for an empty focused column (the old code showed focus via header color, which is now always per-column). All other decisions follow the spec.

---

## File structure

Only one file changes:

- **Modify:** `src/ui/board.rs`
  - Remove: `CARD_HEIGHT` const, `visible_cards`, `first_visible` (+ their tests).
  - Add: `BAR_HEIGHT`/`SELECTED_HEIGHT` consts, `CardPlacement` struct, `card_height`, `scroll_start`, `card_layout` (pure), `truncate`, `card_line`.
  - Rewrite: `render_column`, `render_card`. Minor edit: `render_board` (column gutter spacing).
  - Keep unchanged: `bullet_color`, `ColumnParams`, the status-line code in `render_board`.

No other files change. `src/theme.rs`, modals, and all non-UI code are untouched.

---

## Task 1: Pure variable-height card layout (`card_layout`)

Introduce the layout/scroll math as a pure function, fully unit-tested, before any rendering changes. The new items are marked `#[allow(dead_code)]` until Task 2 wires them in (they are only referenced by tests in this task).

**Files:**
- Modify: `src/ui/board.rs` (add consts/struct/fns near the top, after the existing `CARD_HEIGHT`/`CARD_GAP` consts; add tests in the existing `#[cfg(test)] mod tests`).

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `src/ui/board.rs`:

```rust
#[test]
fn card_layout_handles_degenerate_inputs() {
    assert!(card_layout(0, None, 10).is_empty());
    assert!(card_layout(5, Some(0), 0).is_empty());
}

#[test]
fn card_layout_uniform_bars_when_no_selection() {
    // body 6 rows, all 1-row bars with a 1-row gap => slots at y 0,2,4.
    let placed = card_layout(5, None, 6);
    let got: Vec<(usize, u16, u16)> =
        placed.iter().map(|p| (p.index, p.y, p.height)).collect();
    assert_eq!(got, vec![(0, 0, 1), (1, 2, 1), (2, 4, 1)]);
}

#[test]
fn card_layout_first_page_includes_tall_selected() {
    // selected #0 is 3 rows; rest are 1-row bars; body 10 rows.
    let placed = card_layout(5, Some(0), 10);
    let got: Vec<(usize, u16, u16)> =
        placed.iter().map(|p| (p.index, p.y, p.height)).collect();
    assert_eq!(got, vec![(0, 0, 3), (1, 4, 1), (2, 6, 1), (3, 8, 1)]);
}

#[test]
fn card_layout_scrolls_to_keep_tall_selected_visible() {
    // 10 cards, selected last (#9, 3 rows tall), body only 6 rows.
    let placed = card_layout(10, Some(9), 6);
    let got: Vec<(usize, u16, u16)> =
        placed.iter().map(|p| (p.index, p.y, p.height)).collect();
    assert_eq!(got, vec![(8, 0, 1), (9, 2, 3)]);
}

#[test]
fn card_layout_clips_selected_when_body_too_short() {
    // body shorter than the 3-row selected card: draw it clipped, not nothing.
    let placed = card_layout(1, Some(0), 2);
    let got: Vec<(usize, u16, u16)> =
        placed.iter().map(|p| (p.index, p.y, p.height)).collect();
    assert_eq!(got, vec![(0, 0, 2)]);
}

#[test]
fn scroll_start_anchors_tall_selection_to_bottom() {
    assert_eq!(scroll_start(10, Some(9), 6), 8);
    assert_eq!(scroll_start(5, Some(2), 100), 0);
    assert_eq!(scroll_start(5, None, 10), 0);
    // Out-of-range selection is ignored.
    assert_eq!(scroll_start(3, Some(5), 10), 0);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib ui::board::tests::card_layout 2>&1 | tail -20`
Expected: FAIL — `cannot find function card_layout` / `scroll_start` / `CardPlacement` not found.

- [ ] **Step 3: Implement the layout helpers**

In `src/ui/board.rs`, find the existing constants near the top:

```rust
/// A card occupies its border lines plus one content line.
const CARD_HEIGHT: u16 = 3;
/// Blank line between stacked cards.
const CARD_GAP: u16 = 1;
```

Replace those two lines with:

```rust
/// A selected card expands into a thick-bordered box (border lines + content).
const SELECTED_HEIGHT: u16 = 3;
/// An unselected card is a single filled bar.
const BAR_HEIGHT: u16 = 1;
/// Blank line between stacked cards.
const CARD_GAP: u16 = 1;

/// Where one card sits within a column body: its index in the ticket slice, its
/// `y` offset from the top of the body, and its drawn height in rows.
#[allow(dead_code)] // wired into render_column in the rendering rewrite
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CardPlacement {
    index: usize,
    y: u16,
    height: u16,
}

/// Drawn height of card `index`: the selected card is a 3-row box, others are
/// 1-row bars.
#[allow(dead_code)]
fn card_height(index: usize, selected: Option<usize>) -> u16 {
    if selected == Some(index) {
        SELECTED_HEIGHT
    } else {
        BAR_HEIGHT
    }
}

/// First card index to draw so the selected (taller) card stays fully visible,
/// anchoring it to the bottom of the view once it scrolls past the first page.
#[allow(dead_code)]
fn scroll_start(count: usize, selected: Option<usize>, body_height: u16) -> usize {
    let sel = match selected {
        Some(s) if s < count => s,
        _ => return 0,
    };
    // Rows needed to render cards [start..=sel] contiguously, including gaps.
    let span = |start: usize| -> u16 {
        let mut rows = 0u16;
        for i in start..=sel {
            rows = rows.saturating_add(card_height(i, selected));
            if i < sel {
                rows = rows.saturating_add(CARD_GAP);
            }
        }
        rows
    };
    let mut start = 0;
    while start < sel && span(start) > body_height {
        start += 1;
    }
    start
}

/// Lay out variable-height cards within a `body_height`-row column body. The
/// selected card is 3 rows; every other card is 1 row; cards are separated by a
/// 1-row gap. Returns only the cards that fit (scrolled to keep the selection
/// visible); a card that does not fully fit is clipped rather than dropped.
#[allow(dead_code)]
fn card_layout(count: usize, selected: Option<usize>, body_height: u16) -> Vec<CardPlacement> {
    if count == 0 || body_height == 0 {
        return Vec::new();
    }
    let start = scroll_start(count, selected, body_height);
    let mut placements = Vec::new();
    let mut y = 0u16;
    for index in start..count {
        if y >= body_height {
            break;
        }
        let full = card_height(index, selected);
        let height = full.min(body_height - y);
        placements.push(CardPlacement { index, y, height });
        y += full + CARD_GAP;
    }
    placements
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib ui::board::tests 2>&1 | tail -20`
Expected: the new `card_layout_*` and `scroll_start_*` tests PASS. (Pre-existing tests `renders_tickets_as_cards_with_borders`, `visible_cards_*`, `first_visible_*` still pass — the old helpers are untouched in this task.)

- [ ] **Step 5: Commit**

```bash
cd ../kamaji-worktrees/board-web-restyle
git add src/ui/board.rs
git commit -m "feat(ui): add variable-height card_layout helper

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Rewrite the board rendering to the web style

Rewrite `render_card` (filled bar / thick selected box), `render_column` (bordered box + per-column colored header + dashed rule, driven by `card_layout`), and add a column gutter in `render_board`. Remove the old `visible_cards`/`first_visible` helpers and update all affected tests. This is one atomic change because the renderers and their buffer-level tests must change together to keep the suite green.

**Files:**
- Modify: `src/ui/board.rs` (rewrite two functions, add two helpers, edit one layout line, drop old helpers + their tests, update + add tests).

- [ ] **Step 1: Add the `truncate` + `card_line` helpers**

In `src/ui/board.rs`, immediately above `fn render_card`, add:

```rust
/// Truncate `s` to at most `max` characters, appending `…` when shortened.
fn truncate(s: &str, max: usize) -> String {
    let len = s.chars().count();
    if len <= max {
        return s.to_string();
    }
    match max {
        0 => String::new(),
        1 => "…".to_string(),
        _ => {
            let mut out: String = s.chars().take(max - 1).collect();
            out.push('…');
            out
        }
    }
}

/// The content line for a ticket: `<pip> #<id> <title>`, with the title
/// truncated to `title_max` characters. The pip uses `bullet_color`; the id
/// keeps the column status accent; the title uses `theme.text`.
fn card_line(
    theme: &Theme,
    ticket: &Ticket,
    level: Option<SignalLevel>,
    title_max: usize,
) -> Line<'static> {
    let accent = theme.status_color(ticket.status);
    let marker = if ticket.session_name.is_some() {
        "●"
    } else {
        "○"
    };
    let marker_span = match bullet_color(theme, ticket.status, level) {
        Some(c) => Span::styled(marker, Style::new().fg(c)),
        None => Span::raw(marker),
    };
    Line::from(vec![
        marker_span,
        Span::styled(format!(" #{} ", ticket.id), Style::new().fg(accent)),
        Span::styled(truncate(&ticket.title, title_max), Style::new().fg(theme.text)),
    ])
}
```

- [ ] **Step 2: Rewrite `render_card`**

Replace the entire existing `fn render_card` (the doc comment + body, from `/// Render a single ticket as a rounded card...` through its closing brace) with:

```rust
/// Render a single ticket. Unselected tickets are single-line `surface`-filled
/// bars; the selected ticket is a thick box bordered in the column accent.
fn render_card(
    frame: &mut Frame,
    theme: &Theme,
    area: Rect,
    ticket: &Ticket,
    selected: bool,
    level: Option<SignalLevel>,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let accent = theme.status_color(ticket.status);
    // `<pip>` (1) + ` #<id> ` then the title.
    let id_len = format!(" #{} ", ticket.id).chars().count();

    if selected {
        // Thick accent-bordered box: border (2) + horizontal padding (2) = 4.
        let title_max = (area.width as usize).saturating_sub(4 + 1 + id_len);
        let line = card_line(theme, ticket, level, title_max).style(
            Style::new()
                .fg(theme.text)
                .bg(theme.surface)
                .add_modifier(Modifier::BOLD),
        );
        let block = Block::bordered()
            .border_type(BorderType::Thick)
            .border_style(Style::new().fg(accent))
            .style(Style::new().bg(theme.surface))
            .padding(Padding::horizontal(1));
        frame.render_widget(Paragraph::new(line).block(block), area);
    } else {
        // Single-row filled bar with a 1-cell inset on each side.
        frame.render_widget(Block::default().style(Style::new().bg(theme.surface)), area);
        let bar = Rect {
            x: area.x + 1,
            y: area.y,
            width: area.width.saturating_sub(2),
            height: 1,
        };
        let title_max = (bar.width as usize).saturating_sub(1 + id_len);
        let line = card_line(theme, ticket, level, title_max).style(Style::new().bg(theme.surface));
        frame.render_widget(Paragraph::new(line), bar);
    }
}
```

- [ ] **Step 3: Rewrite `render_column`**

Replace the entire existing `fn render_column` (doc comment + body) with:

```rust
/// Render one Kanban column: a rounded bordered box containing a per-column
/// colored header, a dashed rule, then the tickets as filled bars (the selected
/// one as a thick-bordered box). The focused column's box border uses the column
/// accent; unfocused columns use the muted border color.
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

    let selected = if params.focused {
        Some(params.selected_row)
    } else {
        None
    };
    for placement in card_layout(tickets.len(), selected, body.height) {
        let card = Rect {
            x: body.x,
            y: body.y + placement.y,
            width: body.width,
            height: placement.height,
        };
        let ticket = tickets[placement.index];
        let is_selected = selected == Some(placement.index);
        let level = levels.get(&ticket.id).copied();
        render_card(frame, theme, card, ticket, is_selected, level);
    }
}
```

- [ ] **Step 4: Add a column gutter in `render_board`**

In `fn render_board`, find:

```rust
    let columns = Layout::horizontal([Constraint::Ratio(1, 4); 4]).split(board_area);
```

Replace with (equal-width columns + a 1-cell gutter, mirroring the web's column gap):

```rust
    let columns = Layout::horizontal([Constraint::Fill(1); 4])
        .spacing(1)
        .split(board_area);
```

- [ ] **Step 5: Remove the dead-code allowances and the old helpers**

a) Delete the four `#[allow(dead_code)]` attributes added in Task 1 (on `CardPlacement`, `card_height`, `scroll_start`, `card_layout`) — they are now used by `render_column`.

b) Delete the now-unused old helpers `fn visible_cards` and `fn first_visible` (doc comments + bodies) entirely.

- [ ] **Step 6: Update the existing tests that assumed the old card style**

In `mod tests`:

a) Delete the tests `first_visible_keeps_selection_on_screen` and `visible_cards_counts_whole_cards` (the helpers no longer exist).

b) Replace `renders_tickets_as_cards_with_borders` with:

```rust
#[test]
fn renders_board_with_column_boxes_and_a_selected_card() {
    // Default selection is column 0, row 0 -> the only ticket is selected.
    let app = app_with_theme(vec![ticket(1, Status::Todo)], "catppuccin");
    let buf = render(&app, &HashMap::new(), 80, 20);
    let text = buffer_text(&buf);
    assert!(text.contains("#1"), "expected ticket id in:\n{text}");
    assert!(text.contains("title1"), "expected title in:\n{text}");
    // Columns are rounded boxes.
    assert!(
        text.contains('╭') && text.contains('╰'),
        "expected rounded column-box corners in:\n{text}"
    );
    // The selected ticket is a thick-bordered box.
    assert!(
        text.contains('┏') && text.contains('┗'),
        "expected thick selected-card corners in:\n{text}"
    );
}
```

c) `selected_card_has_filled_background` stays valid as written (the selected card is still `surface`-filled). Leave it unchanged.

d) `in_progress_idle_bullet_keeps_default_color` stays valid: it focuses column 1 so the card is selected; the idle bullet inherits the selected line's `theme.text` fg. Leave it unchanged.

e) `column_title_shows_matches_over_total_when_filtering` stays valid: the header now reads `TODO · 1/2` (no leading space), and the test asserts `.contains("TODO · 1/2")`. Leave it unchanged.

- [ ] **Step 7: Add tests for the new look**

Add to `mod tests`:

```rust
#[test]
fn truncate_appends_ellipsis_only_when_shortened() {
    assert_eq!(truncate("hello", 10), "hello");
    assert_eq!(truncate("hello", 5), "hello");
    assert_eq!(truncate("hello", 4), "hel…");
    assert_eq!(truncate("hello", 1), "…");
    assert_eq!(truncate("hello", 0), "");
}

#[test]
fn columns_are_drawn_as_boxes() {
    let app = app_with_theme(vec![ticket(1, Status::Todo)], "catppuccin");
    let buf = render(&app, &HashMap::new(), 80, 20);
    let text = buffer_text(&buf);
    for corner in ['╭', '╮', '╰', '╯'] {
        assert!(text.contains(corner), "missing column corner {corner} in:\n{text}");
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
    assert_eq!(cell.symbol(), "T", "expected start of 'TODO' header:\n{}", buffer_text(&buf));
    assert_eq!(cell.fg, theme.todo, "header should use the Todo column color");
}

#[test]
fn dashed_rule_is_drawn_under_each_header() {
    let app = app_with_theme(vec![ticket(1, Status::Todo)], "catppuccin");
    let buf = render(&app, &HashMap::new(), 80, 20);
    assert!(buffer_text(&buf).contains('┄'), "expected a dashed header rule");
}

#[test]
fn unselected_ticket_is_a_filled_bar_without_a_border() {
    // Focus the (empty) In Progress column so the Todo ticket is unselected.
    let mut app = app_with_theme(vec![ticket(1, Status::Todo)], "catppuccin");
    app.selected_col = 1;
    let theme = crate::theme::Theme::by_name("catppuccin");
    let buf = render(&app, &HashMap::new(), 80, 20);
    // The bar fills its line with the surface color...
    let has_surface = (0..buf.area.height)
        .any(|y| (0..buf.area.width).any(|x| buf[Position::new(x, y)].bg == theme.surface));
    assert!(has_surface, "unselected ticket should be a surface-filled bar");
    // ...and there is no thick selected-card border anywhere.
    assert!(
        !buffer_text(&buf).contains('┏'),
        "an unselected ticket must not draw a thick border"
    );
}
```

- [ ] **Step 8: Format, lint, and run the full suite**

Run:
```bash
cd ../kamaji-worktrees/board-web-restyle
cargo fmt
cargo clippy --all-targets 2>&1 | tail -20
cargo test 2>&1 | tail -30
```
Expected: `cargo fmt` makes no further changes; `clippy` reports no warnings in `src/ui/board.rs`; all tests PASS (no references to `visible_cards`/`first_visible`/`CARD_HEIGHT` remain).

- [ ] **Step 9: Commit**

```bash
cd ../kamaji-worktrees/board-web-restyle
git add src/ui/board.rs
git commit -m "feat(ui): restyle board to match the web board preview

Bordered columns with per-column colored headers and dashed rules,
tickets as filled surface bars, and the selected ticket as a thick
accent-bordered box. Pure card_layout drives variable-height scrolling.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Visual verification

Confirm the restyle looks right in a real terminal before opening the PR.

**Files:** none (manual check).

- [ ] **Step 1: Build and eyeball the board**

Run: `cd ../kamaji-worktrees/board-web-restyle && cargo build 2>&1 | tail -5`
Expected: clean build.

- [ ] **Step 2: Launch and compare against the mockup**

Run `./target/debug/kamaji` (or the project's run skill) in a project that has a few tickets across columns. Confirm against `kamaji-web/public/index.html`'s board preview:
- Each column is a rounded box; the focused column's border is tinted with its accent color.
- Headers are uppercase, per-column colored, bold, with a dashed `┄` rule beneath.
- Tickets are filled bars; the selected one is a thick accent-bordered box.
- Moving the selection up/down keeps the selected card on screen, including in an overflowing column.
- The bottom board bar still shows `project: <name>` and the key hints.

If anything is visually off, note it and loop back to Task 2.

---

## Self-review notes

- **Spec coverage:** column boxes (§1) → Task 2 Step 3; per-column header + dashed rule (§2) → Task 2 Step 3; filled bars + preserved `bullet_color` (§3) → Task 2 Steps 1–2; thick selected card (§4) → Task 2 Step 2; variable-height layout/scroll (§5) → Task 1; board bar unchanged (§6) → untouched; testing (§7) → Task 1 Steps 1/3, Task 2 Steps 6–7.
- **Risks from spec:** title truncation on narrow widths → `truncate` + `saturating_sub`, tested in `truncate_appends_ellipsis_only_when_shortened`; selected card taller than body → `card_layout` clipping, tested in `card_layout_clips_selected_when_body_too_short`.
- **Type consistency:** `card_layout`/`scroll_start`/`card_height`/`CardPlacement{index,y,height}` are defined in Task 1 and consumed unchanged in Task 2; `card_line`/`truncate` are defined and used within Task 2.
- **One refinement beyond the spec:** focused-column border uses the column accent (documented in the Scope note above).
