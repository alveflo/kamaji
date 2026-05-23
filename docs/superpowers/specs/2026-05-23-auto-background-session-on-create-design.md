# Auto-start a background Zellij session on ticket creation

**Date:** 2026-05-23
**Status:** Approved (design)

## Problem

Today, creating a ticket produces a plain **Todo** card with no worktree, no
session, and no running agent. Work only starts when the user presses `Enter`
(or moves the card to In Progress), which creates the worktree + session and
**attaches** the user to it in the foreground.

We want a faster path: when the user creates a ticket, kamaji should immediately
spin up the agent's Zellij session **in the background** — detached, so the user
stays on the board and the agent starts working right away.

## Goals

- On ticket creation, optionally start the agent in a **detached** Zellij
  session without pulling the user into it.
- The user keeps working on the board; they can `Enter` the ticket later to
  attach to the already-running session.
- Reuse the existing worktree/layout/instrumentation machinery so a
  background-started session is identical to a foreground-started one (idle
  detection, auto-move to "Needs attention", cleanup, etc. all keep working).

## Non-goals

- Changing how foreground start (`Enter` / move to In Progress) works.
- Any new background-session lifecycle UI beyond the existing board affordances.
- Spawning sessions for tickets that already exist (this is a creation-time
  feature only).

## Mechanism: detached session creation in zellij

Verified against zellij **0.43.1**. A single command, which needs no TTY and
does not require releasing kamaji's terminal:

```
zellij --layout <layout-file> attach --create-background <name>
```

The top-level `--layout` makes the layout the session's *initial* tab, and
`attach --create-background` creates the session **detached** (returns
immediately; the user is *not* attached). The result has exactly **one** tab —
the agent — so a later `attach` lands directly on it.

Empirically confirmed: the command embedded in the layout actually executes in
the detached session, and the session contains a single tab.

### Alternative considered and rejected

Running the normal attaching invocation (`zellij -s NAME -n LAYOUT`) under
`setsid`/headless to "detach" it. Rejected: zellij needs a real TTY to attach
and would hang or error without one. The `--create-background` path is
purpose-built for exactly this.

### Single tab (resolved)

An earlier two-step approach (`attach --create-background` then
`action new-tab --layout`) left a stray empty default tab in front of the agent
tab, so attaching landed on a blank shell. Folding the layout into session
creation via the top-level `--layout` flag removes that tab entirely: the
session is born with the agent layout as its only tab.

## Behavior

- The **create** form gets a new **"Start in background"** toggle field,
  **default on**. It is shown only in create mode, never in edit mode.
- Submitting with the toggle **on**:
  1. Create the ticket row.
  2. Prepare its worktree + layout + agent argv + instrumentation (the existing
     `start_session` machinery).
  3. Launch the detached session.
  4. Record `session_name` + `instrumented` and move the ticket to **In
     Progress**.
  5. Idle detection then tracks it exactly like any other live session.
- Submitting with the toggle **off**: today's behavior — a plain **Todo** card,
  no worktree, no session.
- **Graceful failure:** if the session cannot be started (project root is not a
  git repo, zellij missing, or a launch command fails), the ticket is *still
  created* and left in **Todo** with an error toast. Creation never hard-fails.
  This keeps default-on safe in non-git / no-zellij environments.

## Code design

Follows the existing "engine decides, main loop performs IO" pattern (cf.
`Effect::RunSession` / `Effect::Attach`, and `detect_tick_with` vs
`detect_tick`).

### `engine.rs`

- **Extract** a `prepare_session(&Ticket) -> Result<Prepared>` helper from the
  current `start_session`, where
  `Prepared { name: String, layout_path: PathBuf, worktree: PathBuf, instrumented: bool }`.
  It performs: git-repo check, base-branch resolution, worktree add, agent argv
  build, Claude instrumentation (marker reset + `--settings` injection), bar
  style resolution, layout render + temp file write. It does **not** write the
  session/status DB columns.
- **Foreground** `start_session` becomes: call `prepare_session`, then write the
  DB columns (`set_ticket_session`, `set_ticket_instrumented`,
  `set_ticket_status(InProgress)`), reload, return `Effect::RunSession`. (Net
  behavior unchanged.)
- **Background start**: in `submit_form`'s create branch, when the toggle is on,
  call `prepare_session`. On `Ok`, write the same DB columns + move to In
  Progress, reload, and return `Effect::RunSessionBackground { name,
  layout_path, cwd }` (`cwd` = worktree path). On `Err`, set an error toast and
  leave the ticket in Todo (return `Effect::None`).
- The DB columns are written before the launch completes — the same trust model
  `RunSession` already uses. If the launch later fails, the main loop reconciles
  (see below).

### `Effect`

- Add `RunSessionBackground { name: String, layout_path: PathBuf, cwd: PathBuf }`.

### `zellij.rs`

- Add `create_session_background(name: &str, layout_path: &Path, cwd: &Path) -> Result<()>`:
  runs the single `zellij --layout … attach --create-background …` command above
  with `cwd` as the working directory, using `.output()` (not `.status()`) so
  zellij's stdout/stderr are captured rather than painted onto the live TUI (same
  rationale as `dump_screen`). Returns `Err` if the command fails to spawn or
  exits non-zero. Thin and untested, like the existing
  `create_session` / `attach_session`.

### `main.rs`

- Handle `Effect::RunSessionBackground` **without** releasing the terminal: call
  `zellij::create_session_background(...)`. On success, toast
  `#<id> started in background`. On failure, toast the error and call
  `engine.reconcile()`, which clears the dangling session columns for the
  session that never came up.
  - Accepted limitation: after a failed launch the ticket's *status* may remain
    In Progress (reconcile clears `session_name` but not status). It is
    recoverable — the card has no session, so `Enter` will start a fresh one.

### `app.rs` (form)

- `FormField` gains a `Background` variant.
- `TicketForm` gains `start_in_background: bool`, initialized `true` in
  `new_create`. `from_ticket` (edit) leaves it irrelevant.
- `fields()` includes `Background` only in create mode (after `Agent`).
- Toggling: Space / Left / Right on the `Background` field flips the bool.
- `input_char` / `backspace` ignore the `Background` field.

### `ui/modals.rs`

- Render the toggle in create mode as e.g. `Start in background: [x]` / `[ ]`,
  highlighted when active.
- Update the form hint line to mention the toggle.

### Help

- Update the `?` help so `c` / `Enter` descriptions reflect that creation can
  auto-start a background session.

## Testing (TDD)

Engine-level tests use a real git tempdir but do **not** launch real zellij —
they assert on the returned `Effect` and DB state, exactly like the existing
`start_session_creates_worktree_and_effect` test.

- **Toggle on, git repo present:** submitting the create form returns
  `Effect::RunSessionBackground` with the expected session name; the ticket is
  In Progress with `session_name` set and (for Claude) `instrumented` true; the
  layout file exists and injects `--settings`.
- **Toggle off:** submitting creates a Todo card with no session (today's
  behavior).
- **Toggle on, non-git root:** the ticket is created and left in Todo, a toast
  is set, and the returned effect is `None` (graceful failure).
- **Form behavior:** `Background` field is present in create mode and absent in
  edit mode; the toggle flips on Space/Left/Right and starts `true`.

`zellij::create_session_background` itself is not unit-tested (it shells out to
zellij), consistent with the other launch wrappers.

## Risks / open items

- Spare default tab cosmetics (handled or accepted, see above).
- Status-stuck-In-Progress after a failed launch (recoverable, see above).
- Many rapid creations spawn many background agents at once; out of scope to
  throttle here, but worth noting.
