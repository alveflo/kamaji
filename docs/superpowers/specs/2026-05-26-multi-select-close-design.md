# Multi-select close — design

## Problem

Closing tickets on the board is one-at-a-time: select a card, `m` → Done →
confirm. When several tickets are finished (or a column needs clearing), this is
tedious. We want to select multiple cards and close them all in one action.

"Close" in kamaji means moving a ticket to `Status::Done`, optionally cleaning
up its worktree/session (the existing `ConfirmDone` `y`/`n` choice).

## Scope

In scope: multi-selecting cards and bulk-closing them to Done.

Out of scope (YAGNI): generalized bulk *move* to arbitrary columns. Moving many
cards to In Progress would spawn many sessions at once — a different, riskier
feature. Batch operations here are limited to closing.

## Selection model

Add a selection set to `App`, independent of the cursor:

```rust
pub selected_ids: HashSet<i64>,
```

- The cursor (`selected_col`/`selected_row`) is unchanged; it still drives
  navigation and single-card actions.
- The set holds ticket ids, so it is filter-independent: a card hidden by an
  active search filter stays in the set and is still closed by a bulk close.
- The set is pruned on `reload()` so ids of deleted/closed tickets never linger.

## Keys (board)

| Key     | Action                                                              |
|---------|---------------------------------------------------------------------|
| `Space` | Toggle the focused card in/out of the selection set.                |
| `D`     | Close the selection (move all to Done) via one cleanup confirm.     |
| `Esc`   | Clear the selection if non-empty (else falls through to clear search). |

`D` (Shift+d) is chosen so lowercase `d` keeps its Delete meaning. When the
selection set is empty, `D` falls back to the single focused card, so it doubles
as a fast single-close.

`Esc` precedence: if a selection exists, the first `Esc` clears it; a second
`Esc` (selection now empty) clears any active search filter as before.

## Confirm modal

Generalize the existing variant:

```rust
// before
Modal::ConfirmDone { ticket_id: i64 }
// after
Modal::ConfirmDone { ticket_ids: Vec<i64> }
```

- The `m` → Move → Done path passes `vec![ticket_id]` (single-close unchanged).
- `D` passes the selection set (or the focused id when the set is empty).
- On `y`: for each id, `cleanup_ticket(id)` then set Done.
- On `n`: for each id, set Done only.
- On Esc: cancel, leaving everything as-is.
- After `y`/`n`, clear the selection set.

The confirm body adapts its count, e.g. "Close 3 tickets — clean up worktrees +
sessions? [y]es / [n]o / Esc" (singular wording for one ticket).

## Rendering

`render_card` gains a `multi_selected: bool` parameter:

- Multi-selected cards draw their border in `theme.accent` even when not under
  the cursor, so the selection is visible across columns.
- A leading `✓ ` span (in accent) precedes the session bullet on selected cards.
- A cursor-focused card that is also multi-selected keeps its surface fill and
  shows the check.

Status bar:

- When the selection is non-empty, show an `N selected` indicator.
- Add hints: `[space]select [D]close`.

Help modal gains the two new keys.

## Testing

- `App`: toggling adds/removes ids; `reload` prunes stale ids; clear empties it.
- `Engine`: `D` with a multi-selection opens `ConfirmDone` with all ids; `D`
  with an empty set targets the focused id; `y` closes + cleans every id and
  clears the set; `n` closes without cleanup; Esc cancels and keeps the set.
- `Esc` precedence: clears selection before search.
- UI: a multi-selected card renders the check and accent border; the status bar
  shows the `N selected` indicator.
</content>
