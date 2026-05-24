# Board restyle to match the web "board preview" — design

## Goal

Restyle the kamaji Kanban board TUI so it visually matches the "board preview"
section of the marketing site (`kamaji-web/public/index.html` lines 74–105 and
`kamaji-web/public/styles.css` lines 251–290).

This is a **rendering-only** change. No keybindings, search, signal detection,
or data flow change — only how the board is painted.

## Source of truth (the web mockup)

The web board preview is a four-column Kanban inside a terminal-window card:

- Each **column** is a rounded bordered box with a subtle dark fill and a
  per-column **colored, uppercase header** (Todo=grey, In Progress=blue,
  Review=yellow/peach, Done=green) underlined by a **dashed** rule.
- Each **ticket** is a solid `surface`-filled single-line bar: a pip
  (`○` empty / `●` full) + `#<id>` + title. No per-ticket border.
- A bottom **board bar**: `project: <name>` in the accent color on the left,
  keybinding hints in a muted color.

The CSS palette is literally Catppuccin Mocha, which already matches
`src/theme.rs`, so **no palette changes are needed**.

## Decisions (from brainstorming)

- **Bordered columns, no faux titlebar.** We wrap each column in a border but do
  not add the web's traffic-light window chrome — in a TUI the terminal already
  is the window. (Chosen over a max-fidelity faux titlebar and over a
  recolor-only no-boxes approach.)
- **Filled ticket bars; accent border on the selected card.** Unselected tickets
  are single-line `surface`-filled bars; the focused column's selected ticket
  expands into a thick accent-bordered box. (Chosen over a brighter-background
  selection and over keeping the current outlined cards.)

## Scope

- **In scope:** `src/ui/board.rs` (board + column + card rendering, the
  card-layout/scroll helpers, and the board tests).
- **Out of scope:** `src/theme.rs` (palette unchanged), modals
  (`src/ui/modals.rs`), and all non-UI code. No behavior changes.

## Design

### 1. Column containers

Each of the four columns is rendered inside a rounded bordered `Block`
(`BorderType::Rounded`, border color `theme.border`) with horizontal padding of
1. The column background stays `theme.base` (same as the board): the web's column
fill is a ~15% black overlay that we cannot reproduce without introducing a new
"panel" shade to every theme, and a border-only box already delineates columns
cleanly against the base background. Tickets use `theme.surface`, so they still
pop inside the box.

### 2. Column header

Inside each column box:

- **Header line:** `STATUS · n` (uppercase), drawn in the per-column color
  (`theme.status_color(status)`) **always** — not only when focused — and bold.
  The count logic is unchanged: `matches/total` while a search filter is active,
  otherwise the total.
- **Rule line:** a **dashed** horizontal rule (`┄` repeated to the inner width)
  in `theme.border`, mirroring the web's dashed underline. (Replaces the current
  solid `─` rule.)

### 3. Tickets as filled bars

Each unselected ticket is a single full-width line filled with `theme.surface`:

- Content: `<pip> #<id> <title>`, where the title is truncated with `…` to fit
  the inner width.
- `#<id>` keeps the column status-accent color (a per-column swatch, as today);
  the title is `theme.text`.
- The whole line's background is `theme.surface` so it reads as a solid chip
  even where the text is short (render a `surface`-styled fill across the bar
  rect, then the text line on top).
- **Pip coloring keeps the existing `bullet_color` logic**: `●` if the ticket has
  a session else `○`; Review → `theme.attention`, In Progress + `Active` signal
  → `theme.active`, otherwise inherit. This is driven by real session signal and
  is richer than the web's static mapping, so it is preserved.

A single blank gap line separates stacked bars; the column's base background
shows through the gap so adjacent bars read as distinct chips.

### 4. Selected ticket

The selected card (only the focused column has one) expands into a three-line
box:

- `BorderType::Thick`, border color = the column's accent
  (`theme.status_color(status)`).
- Fill `theme.surface`, text bold.
- Same content line as the filled bar.

### 5. Card layout & scrolling

Cards now have **variable height**: the selected card is 3 rows, every other card
is 1 row, with a 1-row gap between cards. The current
`visible_cards`/`first_visible` helpers assume a fixed card height and must be
replaced by a small layout routine that:

- Assigns each card a height (`3` if selected, else `1`).
- Stacks cards top-to-bottom with a 1-row gap, within the column body height.
- Computes a scroll offset so the selected card stays fully visible (anchoring it
  to the bottom of the view once it scrolls past the first page, as today).

Unfocused columns have no selected card, so every card is a uniform 1-row bar —
the simple, common case.

The routine returns, for the visible window, the list of `(ticket_index,
y_offset, height)` to draw, so `render_column` stays a thin loop. It is pure and
unit-testable independent of `Frame`.

### 6. Board bar

Unchanged. The existing status line already matches the web's bottom bar:
`project: <name>` in the accent color, search/filter state, an optional status
message, and muted keybinding hints.

## Components / units

- `render_board` — paints background, splits board/status areas, lays out the 4
  columns, draws the status line. (Largely unchanged.)
- `render_column` — draws the column box, header line, dashed rule, then loops
  over the laid-out visible cards. (Reworked to draw a box + use the new layout
  helper.)
- `render_card` — draws one ticket: a 1-row filled bar, or a 3-row thick-bordered
  box when selected. (Reworked from the current outlined-card-with-strip.)
- `card_layout` (new, pure) — given card count, selected index (or none), and
  body height, returns the visible `(index, y, height)` placements + handles
  scrolling. Replaces `visible_cards` + `first_visible`.
- `bullet_color` — unchanged.

## Testing

Update the existing board tests that assume the old card style, and add coverage
for the new look:

- **Update** `renders_tickets_as_cards_with_borders`: unselected tickets are now
  borderless bars; assert the **column box** rounded corners (`╭`/`╰`) and that
  the selected card has a **thick** border (`┏`/`┗`). Keep the id/title checks.
- **Update** `selected_card_has_filled_background`: still valid (selected card is
  `surface`-filled); confirm against the thick-bordered selected card.
- **Keep** the `bullet_color` tests, the search-count test
  (`column_title_shows_matches_over_total_when_filtering` — header still reads
  `TODO · 1/2`), and the status-bar tests.
- **Add**: an unselected ticket's content line has `theme.surface` background;
  every column draws a border; a column header is drawn in its per-column color;
  the dashed rule (`┄`) is present.
- **Replace** `visible_cards`/`first_visible` tests with `card_layout` tests:
  no-selection uniform case fits N bars; the selected (tall) card stays visible
  when the list overflows; degenerate inputs (empty list, zero height) don't
  panic.

## Risks / notes

- **Variable-height layout** is the trickiest part; the pure `card_layout` helper
  isolates and tests it away from rendering.
- **Narrow terminals:** column inner width = `width - 2 (border) - 2 (padding)`.
  Title truncation must handle very small widths (and the degenerate case where
  no content fits) without panicking.
- **Min height:** a column body shorter than 3 rows cannot show a selected
  bordered card fully; clip gracefully (draw what fits), as the current code does
  for clipped cards.
