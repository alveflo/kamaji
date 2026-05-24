# Ticket search / filter on the Kanban board — design

**Date:** 2026-05-24
**Status:** Approved, ready for implementation plan

## Goal

Let the user narrow the board to the tickets they care about by typing a query.
Matching cards stay visible across all four columns; non-matching cards are
hidden. The board remains fully interactive while a filter is applied.

## Scope

- Board view only. No persistence of the query across restarts.
- No per-column search; one query filters all columns at once.
- Match against the ticket **title** only.
- **Case-insensitive substring** matching (e.g. `log` matches `Add login`).

Out of scope (explicitly not built): fuzzy matching, searching description /
id / agent, saved filters, regex.

## Interaction model

Search is a lightweight board sub-mode, **not** a `Modal`. The state lives on
`App`, so the board stays interactive while a filter is applied. There are two
sub-states:

- **Editing** — the user is typing the query; the board re-filters live on every
  keystroke.
- **Applied (not editing)** — the filter persists and the user navigates the
  matching cards with the normal board keys.

### Keys

Handled in `Engine::on_board_key`. When editing, an input-capture branch runs
**before** the normal board hotkeys, so typed characters edit the query instead
of triggering `c` / `q` / etc.

| Key         | When     | Action                                                            |
|-------------|----------|-------------------------------------------------------------------|
| `/`         | board    | Start editing the query (re-edits the existing query if one set)  |
| any char    | editing  | Append to query, re-filter live, reclamp the cursor               |
| `Backspace` | editing  | Delete last char, re-filter, reclamp                              |
| `Enter`     | editing  | Commit: stop editing, keep the filter, return to board navigation |
| `Esc`       | editing  | Clear the query and exit search                                   |
| `Esc`       | applied  | Clear the active filter                                           |

A query that is empty after editing is equivalent to no filter.

## Filter logic

`App` gains search state:

```rust
#[derive(Debug, Clone, Default)]
pub struct Search {
    /// The current query. Empty means no filter.
    pub query: String,
    /// True while the user is typing the query (input is captured by search).
    pub editing: bool,
}
```

`App::column_tickets(status)` gains a search predicate and becomes the single
source of truth: a ticket is shown when its status matches the column **and**
its title contains the query (case-insensitive substring; an empty query
matches everything). Because navigation, selection, and rendering all go through
`column_tickets`, they automatically operate on the visible set.

A small helper expresses the predicate so it can be unit-tested in isolation,
e.g. `Search::matches(&self, ticket) -> bool`.

After any query change (append / backspace / clear) the cursor is reclamped via
the existing `clamp_row` path so `selected_row` never points past the filtered
column length. When a column filters down to empty, `selected_ticket()` returns
`None` and the existing `if let Some(t)` guards on `e` / `m` / `d` / `Enter`
make those keys no-op.

### Detection is unaffected

`gather_levels` and `reconcile` iterate `app.tickets` directly, not
`column_tickets`. They must remain that way: agents keep being monitored and
auto-moved even while a filter hides their cards. This is a correctness
requirement, not an incidental detail.

## Rendering

- **Column title:** while a filter is active, show `Todo (matches/total)`;
  otherwise the current `Todo (total)`.
- **Status bar:**
  - editing → `search: <query>_` (trailing cursor)
  - applied (non-empty query, not editing) → `filter: <query> — Esc to clear`
  - no filter → unchanged from today
- Add `[/]search` to the hints line and to the help modal.

## Testing (TDD)

Unit tests, written before implementation:

- **Predicate:** `Search::matches` — case-insensitive, substring, empty query
  matches all.
- **Filtering + counts:** `column_tickets` returns only matching tickets for a
  column; total vs. matches counts are correct.
- **Cursor reclamp:** narrowing the filter so the selected row falls outside the
  filtered column clamps `selected_row` to the new length (no panic, valid
  selection).
- **Key flow:** `/` enters editing; typing characters appends to the query and
  does **not** trigger `c` / `q`; `Enter` commits (filter persists, editing
  off); `Esc` while editing clears the query; `Esc` while applied clears the
  filter.
- **Detection independence:** a filter that hides an in-progress ticket does not
  stop it from being gathered / auto-moved (assert `gather_levels` /
  `detect_tick` still see the hidden ticket).

## Files touched

- `src/app.rs` — `Search` struct, `App.search` field, predicate, filtered
  `column_tickets`, reclamp on query change.
- `src/engine.rs` — search-editing input branch + `/` and `Esc` handling in
  `on_board_key`.
- `src/ui/board.rs` — per-column `matches/total` title, status-bar search/filter
  line, `[/]search` hint.
- `src/ui/modals.rs` — `/` line in the help modal.
