# Start background sessions from the `ticket create` CLI

**Date:** 2026-05-25
**Status:** Approved (design)

## Problem

The TUI can start an agent's Zellij session in the **background** (detached)
when a ticket is created — the create form has a "Start in background" toggle,
default on (see `2026-05-23-auto-background-session-on-create-design.md`). The
CLI cannot. `kamaji ticket create` only inserts a plain **Todo** row via
`cli::run_create_ticket`; it never touches worktrees, layouts, or zellij. Work
only begins later when a human presses `Enter` on the card in the TUI.

We want the CLI to optionally start the session too, so a ticket created from
the shell can begin working immediately without opening the board.

## Decisions

- **Scope:** `kamaji ticket create` only. No new `ticket start` command.
- **Opt-in flag:** `--background` / `-b`, default **off**. Without it, behavior
  is byte-for-byte today's (plain Todo card, no session). This keeps any
  existing scripts that call `ticket create` unaffected.
- **Failure policy:** if `--background` is passed but the session cannot start
  (root is not a git repo, zellij missing, launch fails), the **ticket is still
  created** and left in **Todo**, a reason is printed to **stderr**, and the
  process **exits non-zero** so scripts can detect that the session did not
  start. Creation itself never hard-fails.

## Non-goals

- Changing the TUI create toggle or how foreground start (`Enter`) works.
- Starting sessions for already-existing tickets from the CLI.
- Throttling many rapid background launches (out of scope, as in the TUI spec).

## Mechanism

Reuse the existing detached-launch path. `zellij::create_session_background`
runs `zellij --layout <file> attach --create-background <name>`, which needs no
TTY and returns immediately — so a one-shot CLI command can call it directly,
unlike foreground attach which needs the real terminal.

## Approach: extract the shared session machinery

Today `prepare_session` and the DB-commit sequence
(`set_ticket_session` + `set_ticket_instrumented` +
`set_ticket_status(InProgress)`) are private methods on `Engine`, which requires
an `App`/board the CLI does not have. Pull the pure parts into a new module so
both the engine and the CLI call one implementation. (Alternatives considered:
constructing an `Engine` inside the CLI — drags board-only state into a one-shot
command and the launch still lives in `main.rs`; or duplicating the prep logic —
two copies that drift. Both rejected.)

## Code design

Follows the existing "decide here, do IO in `main`" split: the prep + DB writes
are testable without launching zellij; the actual launch happens in `main.rs`.

### `src/session.rs` (new)

- Move `Prepared { name, layout_path, worktree, instrumented }` here.
- `prepare_session(project: &Project, config: &Config, state_dir: &Path, ticket: &Ticket) -> Result<Prepared>`:
  a verbatim move of the current `Engine::prepare_session` body — git-repo
  check, base-branch resolution, worktree add, agent argv build, Claude
  instrumentation (marker reset + `--settings` injection), bar style resolution,
  layout render + temp-file write. Writes **no** DB columns.
- Private `layout_file(name, contents) -> Result<PathBuf>` (moved from `Engine`,
  keeps its `AtomicU64` counter + temp dir).
- `commit_session(db: &Db, ticket_id: i64, p: &Prepared) -> Result<()>`: the
  three DB writes (session columns, instrumented flag, status → In Progress).

### `src/engine.rs`

- `Engine::prepare_session` becomes a thin wrapper:
  `session::prepare_session(&self.app.project, &self.config, &self.state_dir, ticket)`.
- `start_session` and the `submit_form` background branch call
  `session::commit_session(&self.db, id, &p)` instead of inlining the three
  writes.
- Net TUI behavior unchanged — covered by the existing engine tests.

### `src/cli.rs`

- `CreateTicketArgs` gains `background: bool`.
- Parser accepts `--background` / `-b` as a value-less flag (sets `true`);
  absent → `false`. Add it to `USAGE`.
- `run_create_ticket` returns a richer outcome instead of a bare `String`:

  ```rust
  pub struct LaunchSpec {
      pub ticket_id: i64,   // for teardown if the launch fails
      pub name: String,
      pub layout_path: PathBuf,
      pub cwd: PathBuf,
  }

  pub struct CreateOutcome {
      pub message: String,            // stdout summary
      pub launch: Option<LaunchSpec>, // Some => caller must launch zellij
      pub background_failed: bool,    // prepare failed; exit non-zero
  }
  ```

  Flow:
  1. Resolve project + agent + title + prompt and create the ticket row (as
     today).
  2. If `!args.background` → return `{message, launch: None, background_failed: false}`.
  3. Else call `session::prepare_session`:
     - **Ok(p)** → `session::commit_session(db, id, &p)`, then return
       `launch: Some(LaunchSpec { p.name, p.layout_path, p.worktree })`.
     - **Err(e)** → leave the ticket in Todo, fold the reason into `message`,
       return `launch: None, background_failed: true`.

  Keeping the launch out of `run_create_ticket` mirrors the engine's
  decide/do-IO split, so CLI tests run in a real git tempdir and assert on DB
  state + the returned `LaunchSpec` **without** spawning zellij.

### `src/main.rs` (`CreateTicket` arm)

After `run_create_ticket`:

1. Print `outcome.message` to stdout.
2. If `outcome.launch` is `Some(spec)`: call
   `zellij::create_session_background(&spec.name, &spec.layout_path, &spec.cwd)`.
   - **Ok** → print `Started '<name>' in the background`.
   - **Err(e)** → `eprintln!` the reason; tear down like the TUI does
     (`zellij::terminate_session(&spec.name)`, `db.clear_ticket_session(id)`,
     remove the idle marker via `detect::marker_path`) and **revert status to
     Todo** so the card is left clean. (The CLI can revert status cleanly,
     unlike the TUI's `reconcile` path, which leaves status stuck In Progress.)
     `std::process::exit(1)`.
3. Else if `outcome.background_failed`: `eprintln!` the warning, `exit(1)`.
4. Else exit 0.

Teardown on a failed launch needs the ticket id (to clear columns + revert
status) and the session `name` (to terminate + locate the idle marker under
`detect::default_state_dir()`); `LaunchSpec` carries both.

## Error handling summary

- Without `--background`: unchanged; exit 0 with the create message.
- With `--background`, prepare fails: ticket created (Todo), reason on stderr,
  exit 1.
- With `--background`, launch fails: ticket created, session columns cleared +
  status reverted to Todo, reason on stderr, exit 1. The worktree may remain on
  disk; this is harmless and recoverable — a later start reuses it
  (`prepare_session` skips `add_worktree` when the worktree already exists).

## Testing (TDD)

CLI tests use a real git tempdir but never launch real zellij — they assert on
the returned `CreateOutcome`/`LaunchSpec` and DB state, exactly like the engine
tests.

- **Parser:** `-b` and `--background` set `background = true`; absent → `false`.
- **Background off:** `run_create_ticket` makes a Todo card with no session
  (today's behavior); `launch` is `None`, `background_failed` false.
- **Background on, real git root:** ticket → In Progress, `session_name` set,
  worktree created, layout file exists and (for Claude) injects `--settings`;
  `launch` is `Some` with the expected name/cwd/ticket_id.
- **Background on, non-git root:** ticket created + left in Todo, `launch` is
  `None`, `background_failed` true.
- **Engine regression:** existing engine tests must still pass after the
  extraction (foreground `start_session`, background create toggle, etc.).

`zellij::create_session_background` itself stays un-unit-tested (it shells out),
consistent with the other launch wrappers.

## Risks / open items

- Worktree left on disk after a failed launch (harmless + recoverable, above).
- Many rapid `ticket create --background` calls spawn many agents at once; out
  of scope to throttle, same as the TUI.
</content>
</invoke>
