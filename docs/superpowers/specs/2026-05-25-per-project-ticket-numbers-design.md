# Per-project ticket numbers

## Problem

Tickets are displayed as `#{id}` where `id` is SQLite's global auto-increment
primary key. Numbers therefore run continuously across all projects: project A
might show `#1, #2`, then project B starts at `#3`. Users expect each project's
tickets to start at `#1`, like GitHub issues per repository.

## Goal

Each project's tickets are displayed starting at `#1` and counting up
independently of other projects.

## Approach: GitHub-issues pattern (display number alongside internal id)

Keep the global auto-increment `id` as the internal surrogate key. It continues
to back foreign keys, `get_ticket`/`delete_ticket` lookups, and the zellij
session/worktree names (`kamaji-<id>-<slug>`) — so those stay globally unique
and nothing about session naming changes.

Add a separate per-project sequential `number`, used **only** where a ticket is
shown to the user. This is fully backward compatible and avoids cross-project
session-name collisions (two projects each having a ticket `#1`).

Numbers are **never reused**: each project owns a monotonic counter, so deleting
the highest-numbered ticket and creating another yields the next number, not the
deleted one (matches GitHub semantics; avoids two historical tickets both "#3").

## Schema changes (`src/db.rs`)

- `tickets`: add `number INTEGER NOT NULL DEFAULT 0`. (`0` is the sentinel for a
  legacy, not-yet-numbered row.)
- `projects`: add `next_ticket_number INTEGER NOT NULL DEFAULT 1` — the project's
  monotonic counter (value to assign to the *next* ticket).

Both columns are added to the `SCHEMA` `CREATE TABLE` statements (for fresh DBs)
**and** via `add_column_if_missing` in `migrate` (for existing DBs), matching the
existing pattern used for `auto_reviewed`/`instrumented`.

## Migration / backfill

`add_column_if_missing` is changed to return whether it actually added the
column. The per-project backfill runs **once**, guarded on the `tickets.number`
column having just been added, so it can never reset an advanced counter on a
later startup:

1. Number existing tickets per project, ordered by `id` ascending:
   ```sql
   UPDATE tickets SET number = (
     SELECT COUNT(*) FROM tickets t2
     WHERE t2.project_id = tickets.project_id AND t2.id <= tickets.id);
   ```
2. Initialize each project's counter to one past its highest assigned number:
   ```sql
   UPDATE projects SET next_ticket_number = COALESCE(
     (SELECT MAX(number) FROM tickets WHERE tickets.project_id = projects.id), 0) + 1;
   ```

Fresh databases have the column from `SCHEMA`, so `add_column_if_missing` reports
"not added" and the backfill is skipped (there are no rows to number anyway).

## ID generation (`create_ticket`)

Wrap read-counter + insert + increment in a transaction (via
`Connection::unchecked_transaction`, which borrows `&self` and keeps the existing
`&self` method signatures; SQLite serializes writes so concurrent CLI/TUI
processes stay consistent):

```rust
let tx = self.conn.unchecked_transaction()?;
let number: i64 = tx.query_row(
    "SELECT next_ticket_number FROM projects WHERE id = ?1", [project_id], |r| r.get(0))?;
tx.execute("INSERT INTO tickets (project_id, number, title, description, initial_prompt, agent, status)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'todo')",
           params![project_id, number, title, description, initial_prompt, agent.as_str()])?;
let id = tx.last_insert_rowid();
tx.execute("UPDATE projects SET next_ticket_number = next_ticket_number + 1 WHERE id = ?1",
           [project_id])?;
tx.commit()?;
self.get_ticket(id)?.expect("just inserted")
```

## Model (`src/models.rs`)

Add `number: i64` to `Ticket`. Populate it in `row_to_ticket`
(`number: row.get("number")?`).

## Display changes (the only user-facing surfaces)

- `src/ui/board.rs:271` — board card `#{}` uses `ticket.number` (lookups keyed on
  `ticket.id`, e.g. `levels.get(&ticket.id)`, are unchanged).
- `src/cli.rs:171` — `Created ticket #{}` uses `ticket.number`.
- `src/engine.rs:282/288` — status notifications `#{...} → ...` show the ticket's
  `number` (read from the ticket already looked up for its status) instead of the
  internal id.

`src/slug.rs::ticket_name` stays on `id` — session/worktree names are unchanged.

## Testing

- **Migration backfill**: a pre-migration `tickets` table (no `number` column)
  with rows across two projects → after `migrate`, each project's rows are
  numbered `1..N` by id order, and counters are set to `N+1`. Idempotent: a
  second `migrate` does not renumber or reset counters.
- **Per-project start at #1**: two projects each created, first ticket of each
  has `number == 1`.
- **No reuse**: create `#1`, `#2`, delete `#2`, next create is `#3`.
- Update existing assertions that depend on the displayed number
  (`cli.rs` already expects `Created ticket #1` for a project's first ticket;
  verify it still holds) and any `Ticket { .. }` literals in tests to include
  `number`.
