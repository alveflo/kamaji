# Status-aware ticket bullet colors + "Needs attention" rename

**Date:** 2026-05-23
**Status:** Approved (design)

## Goal

Make the Kanban board communicate ticket state through the card bullet color,
and rename the "Review" column to "Needs attention":

1. Rename the **Review** column to **Needs attention**.
2. Tickets in the Needs attention column get an **orange** bullet (always).
3. Tickets in the **In Progress** column get a **green** bullet when their agent
   is actively working, and keep the current color when the agent is "stale".

"Working" vs "stale" reuses the existing `SignalLevel` the auto-review feature
already computes each poll tick — no new detection mechanism:

- `SignalLevel::Active` → working → green.
- `Idle` / `Unknown` / no signal → stale → current color.

This dovetails with auto-review: an In Progress agent that goes idle is already
auto-moved to Needs attention, so In Progress bullets are normally green and
idle work surfaces as orange in Needs attention. The "stale In Progress" branch
covers the cases auto-review doesn't move a card (detection disabled, baseline
tick, manually-placed card).

## Current state

- Columns are the `Status` enum (`Todo`, `InProgress`, `Review`, `Done`) in
  `src/models.rs`; the displayed label comes from `Status::title()`.
- The bullet is drawn in `render_card()` in `src/ui/board.rs`: `●` if the ticket
  has a session, `○` otherwise. Its color is inherited from the card's
  `text_style` — `Color::Gray` (unselected) or black-on-cyan (selected).
- Activity lives in `Engine::last_level: HashMap<i64, SignalLevel>`, recomputed
  every `detect_tick`. The renderer is currently called as
  `ui::render(frame, &engine.app)` and only sees `&App` — it has no access to
  `last_level`.

## Design

### 1. Rename the column

In `src/models.rs`, `Status::title()`:

```rust
Status::Review => "Needs attention",
```

The move modal renders `status.title()`, so it updates automatically. Also
update the two auto-move toast strings in `src/engine.rs` that hard-code
"Review" / "In Progress" so the messages match the visible column names:

- `"#{id} → Needs attention (agent idle)"`
- `"#{id} → In Progress (agent active)"` (already correct)

**Note:** "Needs attention (N)" is wider than "Review (N)"; on a narrow terminal
(~80 cols across 4 columns) ratatui truncates the column title. Accepted.

### 2. Thread activity into the renderer

Pass the signal map explicitly down the render path (keeps `Engine` as the
single source of truth; no state duplicated into `App`):

- `src/main.rs`: `ui::render(frame, &engine.app, &engine.last_level)`
- `src/ui/mod.rs`: `render(frame, app, levels: &HashMap<i64, SignalLevel>)`
- `src/ui/board.rs`: `render_board`, `render_column` take
  `levels: &HashMap<i64, SignalLevel>`; `render_column` looks up
  `levels.get(&ticket.id).copied()` and passes `Option<SignalLevel>` into
  `render_card`.

`board.rs` imports `crate::detect::SignalLevel`.

### 3. Bullet color rule

A small, pure helper in `board.rs`:

```rust
/// fg color to apply to a ticket's bullet, or None to inherit the card's
/// existing text style (the current behavior).
fn bullet_color(status: Status, level: Option<SignalLevel>) -> Option<Color> {
    match status {
        Status::Review => Some(ORANGE),
        Status::InProgress if level == Some(SignalLevel::Active) => Some(Color::Green),
        _ => None,
    }
}
```

| Ticket                                   | Bullet color                          |
|------------------------------------------|---------------------------------------|
| Needs attention (Review) column          | orange (always)                       |
| In Progress + `Active`                   | green                                 |
| In Progress + Idle / Unknown / no signal | current color (stale)                 |
| Todo / Done                              | current color (unchanged)             |

In `render_card`, the marker becomes its own `Span`. When `bullet_color`
returns `Some(c)`, patch **only** the marker's `fg` with `Style::new().fg(c)`
(span style patches over the line's `text_style`, so a selected card keeps its
cyan background + bold and only the bullet glyph changes color). When it returns
`None`, the marker keeps today's `text_style` exactly. The `●`/`○` glyph choice
(session presence) is unchanged.

### Cosmetic decisions (approved)

- **Orange** = `Color::Rgb(255, 165, 0)` (true orange; requires a truecolor
  terminal — effectively all modern ones). Named-color fallback would be
  `Color::Yellow`.
- The status color shows on the bullet **even when the card is selected** (the
  bullet sits colored on the cyan selection bar).

## Testing

`src/ui/board.rs` already renders to a `TestBackend` buffer and inspects cells.

- Update the existing test `render`/`render_board` helpers for the new `levels`
  parameter (pass an empty map where activity is irrelevant).
- Add tests asserting bullet fg color in the rendered buffer:
  - a Needs attention (Review) card's bullet cell has the orange fg;
  - an In Progress card with `Active` has a green bullet;
  - an In Progress card with `Idle` (or absent) does **not** get a green bullet
    (keeps the stale color).
- A direct unit test of `bullet_color` for each `(status, level)` case.
- The rename is covered by asserting the rendered column title contains
  "Needs attention".

## Out of scope

- Time-based staleness / last-activity timestamps (explicitly rejected).
- Any change to the auto-review move logic itself.
- Bullet colors for Todo/Done columns.
