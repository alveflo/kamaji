# Session cleanup modal — design

**Date:** 2026-06-06
**Status:** Approved, ready for implementation

## Goal

Let a user clean up zellij sessions from the browser board. A "Sessions"
button in the topbar opens a modal listing all active `kamaji-*` zellij
sessions with a checkbox per session; the user selects sessions to delete and,
after an in-modal confirmation, the daemon tears them down and the board
updates live.

## Scope decisions (agreed)

- **Which sessions:** all `kamaji-*` sessions zellij reports — ticket-linked,
  orphan, and main/project workspaces. Main workspaces are listed and
  selectable but visually flagged as risky.
- **Teardown:** smart per-session. Ticket-linked sessions get the full
  `session::cleanup_ticket()` (kill + delete session, remove git worktree,
  delete branch, clear DB columns). Orphan and main sessions get only
  `zellij::terminate_session()` (kill + delete).
- **Confirmation:** an in-modal confirm step before the destructive call. No
  second server round-trip — the modal footer swaps to a confirm bar.

## Architecture & data flow

Four thin pieces layered on existing plumbing:

1. **List source** — at modal-open time the daemon calls
   `zellij::list_sessions()`, parses session names, keeps `kamaji-*`, and
   classifies each by cross-referencing the DB.
2. **Modal view** — `GET /ui/sessions/manage` returns a maud fragment morphed
   into `#modal` via Datastar `@get`, same pattern as the new-ticket modal.
3. **Delete endpoint** — `POST /sessions/delete`, body `{"names": [...]}`,
   tears down per the smart rule and emits `Event::SessionExited` per success.
4. **Trigger button** — a "Sessions" button in the topbar next to "+ New".

Flow: button → `@get /ui/sessions/manage` → modal renders snapshot → user
checks boxes → Delete → in-modal confirm → inline `fetch()` POST
`/sessions/delete` → daemon tears down + emits SSE → on success the modal
clears and the board reflects vanished sessions.

## Components

### A. `kamaji-core` — session classification helper

New read-only function (in `session.rs`) so the view and tests share one parse
path:

```rust
pub enum SessionKind {
    Ticket { id: i64, title: String, status: String },
    Main,
    Orphan,
}
pub struct SessionEntry { pub name: String, pub kind: SessionKind }

pub fn classify_sessions(db: &Db, list_output: &str) -> Vec<SessionEntry>
```

Behavior:
- Parse `list_sessions()` output: first whitespace-delimited token per line is
  the session name; dedupe live vs resurrectable/exited entries by name.
- Keep only names starting with `kamaji-`.
- Classify each name:
  - matches a ticket's `session_name` → `Ticket { id, title, status }`
  - matches `kamaji-main-<project-id>` naming → `Main`
  - otherwise → `Orphan`

Pure and unit-testable with a fake list string + in-memory DB.

### B. `kamajid` — modal view (`views/sessions.rs`)

Reuses shared modal chrome (`modal-head/body/foot/close/title`). Body is a row
list; each row has a checkbox `value="<session-name>"` and a kind label:

```
[ ] kamaji-12-fix-auth     ticket #12 · "Fix auth" · in-review
[ ] kamaji-main-abc123     project workspace  ⚠
[ ] kamaji-7-old-thing     orphan
```

- Main rows render with a warning class and start unchecked.
- Footer: Cancel + "Delete selected (N)"; Delete disabled when nothing checked.
- Empty state: "No active kamaji sessions." If zellij is unreachable, render
  the empty state with a note "Could not reach zellij."

### C. Confirm step (in-modal)

Clicking "Delete selected" swaps the footer to a confirm bar: *"Delete N
session(s)? Ticket sessions also remove their git worktree and branch."* with
Confirm / Back. Confirm fires the `fetch()`. One modal, no extra endpoint.

### D. Delete endpoint (`routes/sessions.rs`, wired in `lib.rs`)

`POST /sessions/delete`, JSON `{ "names": ["..."] }`. For each name the server
**re-classifies from the live DB + zellij list** (never trusts the client's
notion of kind):
- ticket-linked → `session::cleanup_ticket()`
- orphan / main → `zellij::terminate_session()`

Collects per-name results, emits `Event::SessionExited` for each success,
returns `{ "deleted": N, "failed": [{ "name", "reason" }, ...] }`.

### E. Trigger button (`page.rs`)

Topbar button beside "+ New": `data-on:click="@get('/ui/sessions/manage')"`,
labeled "Sessions".

## Error handling

- **zellij unreachable** (`list_sessions()` → `None`): modal shows empty state
  with a "Could not reach zellij." note; never errors out.
- **Empty/missing `names`** → `ApiError::BadRequest`.
- **Per-session failure is non-fatal:** `terminate_session()` is already
  best-effort; `cleanup_ticket()` steps can fail (e.g. dirty worktree). Collect
  failures per-name and return them in `{ deleted, failed }` rather than
  aborting the batch. Client shows "Deleted N, M failed" if any failed; clears
  the modal on full success.
- **Race — session already gone:** treated as success (goal state is "gone").
  Periodic `reconcile` clears any stale DB rows.
- **Non-`kamaji-*` name** in the request → skipped/rejected.

## Testing

- **Unit (kamaji-core):** `classify_sessions()` against a fixed
  `list-sessions` string + in-memory DB — ticket-linked, main, orphan, dedupe
  of live-vs-resurrectable, non-kamaji filtering.
- **Integration (kamajid):** router with a test `AppState` and a stubbed zellij
  boundary: bad body → 400; ticket-linked name → cleanup path +
  `SessionExited` emitted; orphan name → terminate path; partial failure →
  `{ deleted, failed }` shape. Follow the stubbing pattern in existing
  `tickets.rs` tests.
- **View:** render test asserting the modal fragment has a row per session with
  the correct kind labels (if view-test conventions exist).

Per the repo TDD convention, each unit's test is written first.

## Out of scope (YAGNI)

- Live-updating the session list while the modal is open (snapshot at open time
  is enough; the board itself updates after deletion via SSE).
- Renaming / attaching to sessions from this modal — delete only.
- Cleaning up non-`kamaji-*` sessions on the machine.
