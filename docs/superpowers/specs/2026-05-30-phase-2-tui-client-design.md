# kamaji — Phase 2: TUI as a `kamajid` Client + Daemon Auto-Spawn

- **Date:** 2026-05-30
- **Status:** Approved (design)
- **Author:** Victor Alveflo
- **Parent spec:** `docs/superpowers/specs/2026-05-27-browser-first-pivot-design.md` (§8 Phase 2)
- **Precursor spec:** `docs/superpowers/specs/2026-05-30-phase-1-kamajid-design.md` (the daemon this phase clients)
- **Precondition:** Phase 1 (incl. 1a–1e) merged — `kamajid` builds and runs, serving the full board command surface (`GET /healthz`, `GET /events`, projects/tickets/config routes, `/tickets/:id/{move,start,done,attach}`) over `kamaji-core`, with the poll loop running inside the daemon and emitting `ticket.*`/`session.*` events. The TUI is still in-process (drives `kamaji-core` directly via `Engine`).

> **Finalization note.** This spec was drafted autonomously (the owner delegated the spec→implementation decision). The three judgment calls in "§13 Owner decisions" were resolved with the recommended answers and are baked into the Decisions table and body below.

## 1. Overview

Phase 2 flips the ratatui TUI (`crates/kamaji`) from driving `kamaji-core`
directly to being a **thin HTTP+SSE client of `kamajid`**. After this phase the
daemon is the single writer of the SQLite DB and the only place git/zellij
orchestration and the auto-review poll loop run; the TUI renders board state it
fetched over HTTP, stays live via the `/events` SSE stream, and turns every user
command into an HTTP call. The TUI still performs **native attach** itself
(`zellij attach <session>` inline in the real terminal) — the daemon only
creates the session in the background and names it.

Running `kamaji` must transparently **ensure a daemon is up**: probe for a live
one (pidfile + `GET /healthz`); if absent, spawn `kamajid serve` detached (it
outlives the TUI), wait for health, then connect. Two `kamaji` processes started
at once must not both spawn a daemon — the loser of a pidfile race connects to
the winner's daemon.

The large, risky part is **retiring the now-duplicated orchestration in
`crates/kamaji/src/engine.rs`**: today `Engine` owns the DB, runs the poll loop
(`detect_tick`), and performs create/move/start/done/cleanup/reconcile against
`kamaji-core`. All of that now lives in the daemon. `Engine` becomes a thin
client-state holder + command dispatcher; the UI-side `App`/rendering
(`app.rs`, `ui/`, `theme.rs`, modals, pickers) stays. This is staged as several
independently mergeable steps, each leaving the TUI working.

### Goals
- `kamaji` (the TUI) talks to `kamajid` for **all** board reads, writes, and
  orchestration; it never opens the DB or runs the poll loop.
- **Auto-spawn:** `kamaji` ensures a daemon (pidfile + health probe; detached
  spawn; race-safe), then connects — zero manual `kamajid serve` step.
- **Live board:** the TUI re-renders from `/events` SSE deltas (`ticket.*`,
  `session.*`); two TUIs against one daemon stay in sync.
- **Native attach preserved:** attach still suspends ratatui and execs
  `zellij attach <session>` (today's `run_zellij` mechanism), unchanged in feel.
- Robust edges: daemon unreachable, daemon dies mid-run, SSE drop/lag,
  version skew — each surfaced clearly, with reconnect + re-fetch where it makes
  sense.
- One writer to the DB — retire the "concurrent instances unsupported" caveat.

### Non-goals (Phase 2)
- Browser UI / Datastar / `zellij web` attach from the TUI (Phase 3).
- Auth, TLS, remote bind, multi-user (deferred; localhost only).
- New daemon *features*. Phase 2 is client-side. Where the TUI needs something
  the API doesn't yet expose, we add the **smallest possible** daemon endpoint
  and call it out explicitly (§4.6, §11). We prefer composing existing routes.
- Splitting `engine.rs` into many files for its own sake — we delete the
  orchestration paths and shrink it; a cosmetic module split is optional cleanup.

## 2. Decisions

| Question | Decision |
|----------|----------|
| Auto-spawn default | **On by default.** `kamaji` always ensures a daemon. Escape hatches: `--daemon <ADDR>` (use an existing daemon, never spawn) and `--no-spawn` (fail if none is up). |
| Client transport | **`reqwest` blocking client** for commands (the TUI loop is sync) + a **background SSE listener thread** that feeds events to the UI through a channel — mirroring the existing update-check thread in `main.rs`. |
| SSE → UI plumbing | SSE thread pushes decoded `Event`s into a shared channel (`std::sync::mpsc` or `Arc<Mutex<VecDeque<Event>>>`), drained each loop iteration before `terminal.draw` — exactly how `update_status: Arc<Mutex<Option<String>>>` is polled today. |
| Board state source | The client keeps an in-memory `Vec<Ticket>` per project, seeded by `GET /projects/:id/tickets`, then mutated by SSE deltas; on any lag/reconnect it **re-fetches** the whole list (cheap; lossy-by-design daemon). |
| Pidfile location | `<XDG_RUNTIME_DIR or cache_dir>/kamaji/kamajid.pid` + a sidecar `kamajid.addr` holding the bound address. Reuse/extend `kamaji_core::paths`. |
| Auto-spawn race | Exclusive-create the pidfile (`O_CREAT \| O_EXCL` / `create_new`) as the lock; winner spawns + writes addr, losers spin on the health probe until the winner is ready, then connect. |
| Daemon address discovery | Read `kamajid.addr` written by the daemon at bind time; fall back to config `[daemon] bind` (default `127.0.0.1:8755`). |
| `Engine` fate | Becomes `Engine = { client: DaemonClient, app: App, project, …ui-only state }`. All DB/zellij/poll code deleted. Keeps `on_key` → `Effect` and modal logic, but handlers call the client. |
| Poll loop in the TUI | **Removed.** No `detect_tick`, no `PollLoop` in the binary; the daemon polls. The TUI learns of auto-review moves via `session.idle`/`ticket.moved` SSE events. |
| `reconcile()` in the TUI | **Removed.** Reconciliation is the daemon's job. The TUI re-fetches after attach instead. |
| CLI subcommand | `kamaji ticket create …` (`cli.rs`) becomes a daemon call too (ensure-daemon → `POST /tickets` → optional `POST /tickets/:id/start`). |
| Config edits | Routed through **`PATCH /config`** (a small new daemon endpoint) so config stays single-writer (theme, default agent, worktree location). |
| Resume fidelity | **Simplified:** a ticket with a live `session_name` → native `Attach`; a daemon-side conversation-*resume* endpoint is a tracked follow-up, not built in Phase 2 (§6.2). |
| Version skew | Client compares its version against `/healthz` `version`; mismatch → non-fatal warning toast; an incompatible body shape → clear "incompatible daemon; stop it and re-run" error. |

## 3. Architecture: the TUI as a daemon client

### 3.1 Process model (after Phase 2)

```
  kamaji (TUI process)                         kamajid (daemon process, detached)
  ┌───────────────────────────┐                ┌──────────────────────────────┐
  │ main loop (sync, ratatui)  │  blocking HTTP │ axum @127.0.0.1:8755          │
  │  ├─ DaemonClient (reqwest) │ ─────────────▶ │  routes/*  → kamaji-core      │
  │  │   cmds: create/move/... │ ◀───────────── │  (DB, git, zellij, poll loop) │
  │  ├─ SSE listener thread    │  GET /events   │  broadcast<Event> ─┐          │
  │  │                         │ ◀═════════════ │  ────────────────── ┘ (SSE)   │
  │  └─ App / ui rendering     │                └──────────────┬───────────────┘
  │     native attach: ────────┼─ exec zellij attach <session> ┘ (PTY, local TTY)
  └───────────────────────────┘
```

- The TUI never opens `kamaji.db` and never runs the poll loop.
- The daemon process is started by the first `kamaji` that finds none alive and
  **keeps running after every TUI exits** (detached).
- Native attach is TUI-local: the daemon's `/start` creates and names the
  session; the TUI execs `zellij attach <name>` against the real terminal.

### 3.2 New/changed files in `crates/kamaji/src`

```
crates/kamaji/src/
├── main.rs        CHANGED — ensure-daemon before TUI; pass DaemonClient down;
│                  start SSE thread; event loop drains SSE + draws (no detect_tick)
├── client.rs      NEW — DaemonClient: blocking reqwest wrapper over the kamajid API
├── daemon.rs      NEW — auto-spawn: pidfile lock, detached spawn, health wait, addr
├── sse.rs         NEW — background SSE listener thread → channel of Event
├── engine.rs      SHRUNK — Engine is now {client, app, …}; handlers call client;
│                  DB/zellij/poll/reconcile code deleted
├── cli.rs         CHANGED — `ticket create` dispatches via DaemonClient
├── app.rs         UNCHANGED (UI state)
├── ui/            UNCHANGED (rendering)
├── picker.rs      CHANGED — project list/create via DaemonClient (no `&Db`)
├── dir_select.rs  UNCHANGED
├── theme.rs       UNCHANGED
└── update.rs      UNCHANGED (self-update path is unrelated)
```

The DTO types crossing the wire (`Ticket`, `Project`, `Status`, `Agent`,
`Config`, `Event`) already live in `kamaji-core` and already
`Serialize`/`Deserialize`. The client deserializes straight into those types —
**no parallel DTO layer.**

## 4. The client module (`client.rs`)

A small, synchronous wrapper over the daemon's REST API using
`reqwest::blocking::Client`. The TUI loop is synchronous and must not host a
tokio runtime in its hot path; blocking calls are simplest and the daemon is on
localhost (sub-millisecond). Each method maps 1:1 to a Phase-1 route.

### 4.1 Construction

```rust
pub struct DaemonClient {
    http: reqwest::blocking::Client,
    base: String,            // e.g. "http://127.0.0.1:8755"
}

impl DaemonClient {
    pub fn connect(base: String) -> Result<Self>;   // builds client; pings /healthz
    pub fn base(&self) -> &str;                       // for the SSE thread's URL
}
```

`connect` sets a short request timeout (e.g. 5s) and performs one `GET /healthz`
to confirm liveness and capture the daemon version for the skew check (§9).

### 4.2 Read methods

| Method | Route | Returns |
|--------|-------|---------|
| `list_projects()` | `GET /projects` | `Vec<Project>` |
| `get_project(id)` | `GET /projects/:id` | `Project` |
| `list_tickets(project_id)` | `GET /projects/:id/tickets` | `Vec<Ticket>` |
| `get_ticket(id)` | `GET /tickets/:id` | `Ticket` |
| `get_config()` | `GET /config` | `Config` |

### 4.3 Command methods

| Method | Route | Body / Result |
|--------|-------|---------------|
| `create_project(name, root_dir, default_agent)` | `POST /projects` | → `Project` (201) |
| `create_ticket(project_id, title, desc, prompt, agent)` | `POST /tickets` | → `Ticket` (201) |
| `update_ticket(id, …)` | `PATCH /tickets/:id` | → `Ticket` |
| `move_ticket(id, target)` | `POST /tickets/:id/move` `{target}` | → `Ticket` |
| `start_ticket(id)` | `POST /tickets/:id/start` | → `Ticket` (emits `session.started`) |
| `done_ticket(id, cleanup)` | `POST /tickets/:id/done` `{cleanup}` | → `Ticket` |
| `delete_ticket(id)` | `DELETE /tickets/:id` | → `()` (204) |
| `attach_info(id)` | `POST /tickets/:id/attach` | → `AttachInfo` (TUI ignores web_url/token) |
| `update_config(cfg)` | `PATCH /config` (new, §11) | → `Config` |
| `main_session(project_id)` | `POST /projects/:id/main-session` (new, §11) | → `{ session_name }` |

These mirror the routes mounted in `crates/kamajid/src/lib.rs::router` plus the
two §11 additions.

### 4.4 Error mapping

The daemon returns `{ "error": …, "kind": "not_found" | "bad_request" |
"internal" }` (`crates/kamajid/src/error.rs`). The client maps:

```rust
pub enum ClientError {
    NotFound,
    BadRequest(String),          // surfaced verbatim as a toast (user-facing reason)
    Server(String),              // 500: generic "daemon error" toast + log
    Unreachable(reqwest::Error), // connection refused / timeout → triggers reconnect (§9.2)
    Decode(String),              // unexpected body shape (version skew?) → §9.4
}
```

`BadRequest` strings are meaningful (e.g. "ticket already has a session; stop it
first", "title must not be empty") and go straight into `App::set_error`.
`Unreachable` is the signal the UI uses to re-probe/respawn the daemon (§9.2).

### 4.5 Threading model

- **Commands** run inline in the event loop via `reqwest::blocking` (a few-ms
  localhost round-trip is imperceptible; no command queue).
- **Events** arrive on a **separate SSE thread** (§5); never block the loop.

This is the shape of today's update-check thread in `main.rs` (a detached
`std::thread::spawn` writing into shared state the loop polls) — generalized from
one string slot to an event channel.

### 4.6 Endpoints we need vs. have

Every TUI command maps onto an existing route with one composition nuance and
two tiny gaps:

1. **Move-to-In-Progress is composite.** Today `Engine::apply_move(InProgress)`
   sets status *and* attaches/resumes (or starts a session). The daemon's
   `POST /move` deliberately only changes the column ("This does NOT start or
   stop any session"). So the TUI composes:
   - ticket already has `session_name` → `move_ticket(InProgress)` then native
     `Attach` using that name.
   - ticket has no session → `start_ticket(id)` (daemon creates worktree +
     session, moves to In Progress) then native `Attach` to the returned name.
   No new endpoint needed.

2. **"main session" (the `s` key).** Today the TUI starts/attaches a per-project
   workspace session not tied to a ticket (`Engine::main_session_effect`,
   `slug::main_session_name`, `session::prepare_main_session`). The daemon has no
   route for this. **New endpoint** `POST /projects/:id/main-session` →
   `{ session_name }` wraps `session::prepare_main_session` +
   `create_session_background`, idempotent if the session is live (§11).

3. **`PATCH /config`** for config edits (theme/default-agent/worktree-location)
   so config stays single-writer (§11).

## 5. SSE listener (`sse.rs`)

A background thread subscribes to `GET /events` and feeds decoded events to the
UI. The SSE wire format is trivial (`event:`/`data:` lines separated by blank
lines, as the daemon emits in `crates/kamajid/src/routes/events.rs`); the
simplest robust approach is `reqwest::blocking::Client` with a streaming
response and a hand-rolled SSE line parser (the Phase-1 integration tests
already parse this format — reuse that shape).

```rust
pub fn spawn(base: String, tx: Sender<SseMsg>) -> JoinHandle<()>;

pub enum SseMsg {
    Event(kamaji_core::events::Event),  // a decoded board delta
    Connected,                           // (re)connected — UI should re-fetch
    Disconnected,                        // stream ended/errored — UI shows "reconnecting"
}
```

Reconstructing the `Event` from the wire: the daemon splits the tagged enum into
an `event:` name + bare `data:` payload (no `type` field). A tiny
`events::from_sse(name, data) -> Option<Event>` helper lives next to
`Event::sse_name()` in `kamaji-core::events` (the inverse of `sse_name`), so the
mapping is defined once and unit-tested for round-trip against the daemon's
framing.

### 5.1 Reconnect & re-sync

The daemon broadcast is **lossy by design** (`events.rs` drops `Lagged`
clients; capacity 64). The listener therefore:

1. On connect, emits `SseMsg::Connected`. The UI re-fetches
   `list_tickets(current_project)` — closing any gap from before/at connect.
2. Streams events as `SseMsg::Event`.
3. On stream end/error, emits `SseMsg::Disconnected`, then retries with backoff
   (250ms → 2s, capped), emitting `Connected` (→ another re-fetch) on success.

Because re-fetch is the source of truth after any reconnect, the TUI never
reasons about *which* events it missed — it reloads the affected project.

### 5.2 Applying events to the UI

Each loop iteration drains the channel and applies deltas to the in-memory
ticket list for the *current* project (events for other projects are ignored):

| Event | UI action |
|-------|-----------|
| `ticket.created` | if `project_id` matches → insert into `app.tickets` |
| `ticket.updated` | replace the ticket |
| `ticket.moved` | update `status`; if an auto-review move (to Review), toast "#id → Needs attention (agent idle)" — reproduces today's `handle_poll_events` toast, now event-driven |
| `ticket.deleted` | remove from `app.tickets`; `prune_selection` |
| `session.started` | update `session_name` (re-fetch the one ticket if needed) |
| `session.idle` | informational; the matching `ticket.moved` carries the column change |
| `session.exited` | clear the session indicator (re-fetch the ticket) |

The simple default applier for an id-only event calls `get_ticket(id)` (one
cheap localhost GET) and splices the fresh row in. After applying,
`app.reclamp()`/`app.prune_selection()` keep the cursor valid — the same calls
`Engine::reload` makes today.

## 6. Native attach stays client-side

Attach is unchanged in mechanism — the existing `run_zellij` flow in `main.rs`
(suspend via `ratatui::restore()`, run the zellij command against the inherited
TTY, `ratatui::init()` to resume, clear the "Bye from Zellij!" banner). Phase 2
only changes **how the TUI learns the session name** and removes the post-attach
`reconcile()`.

### 6.1 Getting the session name + the collapsed `Effect`

- **Start path:** `start_ticket(id)` returns the updated `Ticket`, whose
  `session_name` is set (the daemon's `/start` records it before returning).
- **Re-enter path:** the name is in the ticket the TUI already holds (populated
  via `list_tickets`/SSE); if absent, `get_ticket(id)`.

The `Effect` enum stays the driver between `Engine::on_key` and the loop, but
variants that carried a `layout_path` for client-side session *creation*
(`RunSession`, `RunSessionBackground`, `ResumeSession`) collapse — the daemon
creates sessions now:

```rust
pub enum Effect {
    None,
    SwitchProject,
    SelfUpdate { version: String },
    Attach { name: String },   // native `zellij attach <name>`
}
```

`main.rs` keeps one zellij branch:
`Effect::Attach { name } => run_zellij(terminal, |_| zellij::attach_session(&name))`.
After attach, instead of `engine.reconcile()`, the TUI does a lightweight
`refresh_current_project()` (a `list_tickets` re-fetch). The detach-banner
clearing is kept.

### 6.2 Resume semantics (simplified)

Today the TUI distinguishes attach vs. *resume* (recreate an exited/resurrectable
session from the resume layout) in `Engine::enter_session`. That logic is
session lifecycle and belongs to the daemon. **Decision:** if the ticket has a
`session_name`, the TUI does a plain native `Attach` — zellij itself resurrects
a resurrectable session on attach. The richer "recreate from the resume layout so
the agent resumes its conversation" behavior is a **tracked follow-up** (a
daemon-side resume endpoint), filed as an issue so Phase 2 stays client-side.
This preserves today's *common* path (attach to a live session) exactly; the
rarer post-reboot resume is no worse than a plain attach and is tracked.

## 7. Daemon auto-spawn (`daemon.rs`)

`kamaji` ensures a daemon before constructing the TUI:

```
ensure_daemon(config) -> Result<DaemonClient>:
  1. addr = read pidfile+addrfile; if a daemon is alive there → connect, return.
  2. else acquire the pidfile lock (atomic create_new):
       - WON  → spawn `kamajid serve --bind <addr>` detached;
                wait for /healthz; write addr file; return client.
       - LOST → someone else is starting it; poll /healthz on the expected
                addr with a timeout; connect to the winner, return.
  3. on total failure → a clear fatal error before the TUI ever initializes.
```

### 7.1 Files & locations

Under `<runtime>/kamaji/` where `<runtime>` is `$XDG_RUNTIME_DIR` if set, else
`kamaji_core::paths::cache_dir()` (add a `paths::runtime_dir()` helper that falls
back to cache):

- `kamajid.pid` — lock + daemon PID (created atomically; holds PID text;
  existence is the lock; content lets us liveness-check the PID).
- `kamajid.addr` — the bound address, written by the daemon *after* a successful
  bind (so clients learn the real port) and removed on clean shutdown.

The daemon writing `addr`/`pid` is a tiny daemon-side change (§11).

### 7.2 Liveness probe

A pidfile is "live" iff: the PID it names exists *and* `GET /healthz` returns 200
with `{ok:true}`. Either failing ⇒ **stale** (daemon crashed without cleanup):
the prober removes the stale pidfile/addrfile and proceeds to lock-acquire. We
trust neither PID-exists alone (PIDs get reused) nor health alone (lets us
reclaim a wedged daemon).

### 7.3 The race: lock-on-pidfile, lose-and-connect

1. Both find no live daemon.
2. Both attempt `OpenOptions::new().write(true).create_new(true).open(pidfile)`
   (atomic; exactly one succeeds — the lock).
3. **Winner** spawns `kamajid serve` detached, waits for `/healthz`, writes the
   daemon's real PID + addr.
4. **Loser** gets `AlreadyExists`; does **not** spawn; polls `/healthz` at the
   expected addr (bounded ~5s), then connects.
5. Edge: winner crashes between lock and spawn → stale pidfile → loser's poll
   times out → loser re-runs `ensure_daemon` once (now detects stale, clears it,
   becomes the winner). A bounded retry count (e.g. 2) prevents livelock.

### 7.4 Detached spawn

- Unix: `Command::new(kamajid_path).arg("serve").arg("--bind").arg(addr)`, stdio
  → `/dev/null` (or a daemon log under the data dir), new session
  (`setsid`-equivalent) so it isn't killed when the terminal closes; don't
  `wait()`.
- Locate the binary: prefer a sibling `kamajid` next to the running `kamaji`
  (`std::env::current_exe()` → same dir), since the install ships both together;
  fall back to `PATH`; else fatal "kamajid not found".
- Windows: `DETACHED_PROCESS`/`CREATE_NEW_PROCESS_GROUP` flags. Auto-spawn is
  cross-platform (zellij-web paths stay unix-gated).

### 7.5 Health wait

Poll `GET /healthz` every ~50ms up to ~5s. Success → write addrfile if needed,
return the client. Timeout → kill the just-spawned child (if we hold its handle),
surface a fatal "daemon failed to become healthy (see <logpath>)".

## 8. Retiring `Engine`: phased migration

End state: `Engine = { client, app, project }` plus UI-only helpers. **Deleted**
(daemon owns them): `db`, `poll`, `state_dir`, `prepare_session`,
`start_session`, `enter_session`, `apply_move`'s DB writes, `cleanup_ticket`,
`reconcile`, `detect_tick`, `handle_poll_events`, `forget_ticket_state`,
`main_session_effect` (→ a client call). **Kept:** `on_key`/`on_board_key` and
every modal handler (the keymap is UI), rewired so each former DB/zellij call is
a client call.

The migration ships as independently mergeable steps, each green on `main` and
each leaving the TUI working — daemon up *underneath* the in-process TUI first,
then peel orchestration away incrementally (no big-bang rewrite):

### Step 2a — Client + auto-spawn scaffolding, daemon optional
Add `client.rs`, `daemon.rs`, `sse.rs`. `main.rs` calls `ensure_daemon` and
builds a `DaemonClient`, **but the TUI still drives `Engine`-on-core** (the
daemon runs alongside, as in Phase 1). Start the SSE thread and *log* received
events (not yet applied). Lands + tests auto-spawn, the pidfile race, health
wait, SSE decoding in isolation — zero board behavior change.
*Verify:* `kamaji` spawns/reuses a daemon (pidfile present, `/healthz` green);
two `kamaji` → one daemon; SSE log shows deltas as you act.

### Step 2b — Reads come from the daemon
Switch the picker and board seeding to the client (`list_projects`,
`create_project`, `list_tickets`, `get_config`). The TUI no longer opens the DB
for *reads*. `Engine` still does writes against core temporarily (dual-path for
one step; same DB file, so reads-via-daemon reflect writes-via-core). Wire SSE
application (§5.2) so the board updates live.
*Verify:* board renders from `GET /projects/:id/tickets`; a second `kamaji`'s
change appears live in the first via SSE.

### Step 2c — Writes go through the daemon; delete orchestration from `Engine`
The big one, de-risked because reads/SSE already work. Rewire every mutation
handler to call the client and delete the core-driven implementations:
- `submit_form` create/edit → `create_ticket`/`update_ticket` (+ optional
  `start_ticket`).
- move handlers → `move_ticket`; move-to-In-Progress composes
  `move_ticket`/`start_ticket` + `Attach` (§4.6, §6).
- `ConfirmDone` → `done_ticket(id, cleanup)`; `ConfirmDelete` → `delete_ticket(id)`.
- `s` (main session) → `POST /projects/:id/main-session` + `Attach`.
- config edits (theme/default-agent/worktree-location) → `PATCH /config`.
Remove `Engine::db`, `reconcile`, `detect_tick`, `handle_poll_events`,
`PollLoop` usage, `state_dir`; the in-loop `detect_tick` call and `last_tick`
timer in `run_board`. Collapse `Effect` to the §6.1 set; update `main.rs`'s match.
*Verify:* full manual smoke — create/edit/move/start/attach/done/delete all work
through the daemon; auto-review still moves cards (via SSE from the daemon poll);
two TUIs stay in sync; killing the daemon mid-run surfaces + respawns (§9).

### Step 2d — CLI subcommand through the daemon + cleanup
`cli.rs::run_create_ticket` and `main.rs`'s `CreateTicket` arm dispatch via
`ensure_daemon` + `DaemonClient`. Delete now-dead binary-crate imports (`Db`,
`zellij` [except `attach_session`], `detect`, `session`, `git`, `PollLoop`).
Optional cosmetic: split the slimmed `engine.rs`.
*Verify:* `kamaji ticket create … [--start]` creates (and starts) via the
daemon; no `crates/kamaji` path opens the DB or shells zellij except native
`attach_session`.

## 9. Error handling & edges

### 9.1 Daemon unreachable at startup
`ensure_daemon` returns a fatal error *before* `ratatui::init()`, printed plainly
to stderr with the most useful cause ("kamajid not found" / "daemon failed to
become healthy (see <logpath>)" / "could not bind/connect"). Exit non-zero. No
half-initialized terminal.

### 9.2 Daemon dies while the TUI runs
Detected via `ClientError::Unreachable` on a command or repeated SSE
`Disconnected`. Response: a sticky "daemon unreachable — reconnecting…" status;
re-run `ensure_daemon` (bounded retries + backoff) to reuse a recovered daemon or
respawn a stale one; swap in the new `DaemonClient`, restart the SSE thread,
re-fetch the current project. If reconnection keeps failing past a deadline, the
failed command surfaces its toast and the board stays usable read-stale; never
crash on a blip.

### 9.3 SSE stream drops / lag
Handled structurally (§5.1): every `Connected` triggers a full re-fetch, so a
`Lagged` drop self-heals without tracking missed events.

### 9.4 Version skew
`connect` records `/healthz` `version`; a mismatch → one-time non-fatal warning
toast. A `ClientError::Decode` from an incompatible body → clear "incompatible
daemon version; stop the daemon and re-run." No negotiation protocol in Phase 2.

### 9.5 Stale/foreign port
A foreign process on `127.0.0.1:8755` → daemon bind fails → `ensure_daemon`
surfaces "address in use" with the configured-bind hint. A stale pidfile pointing
at a dead daemon → §7.2's probe reclaims it.

## 10. Testing strategy

### `client.rs`
Boot a real `kamajid` on `127.0.0.1:0` in-process (reuse Phase 1's `serve` +
ephemeral bind + tempdir DB), then drive `DaemonClient`: one happy-path test per
method; negative tests asserting `NotFound`/`BadRequest` mapping from `{kind}`.

### `daemon.rs` (auto-spawn) — highest-value new tests
- Stale pidfile reclaim (pidfile naming a dead PID → treated as stale).
- Race: two threads on the lock-acquire → exactly one wins, the other takes the
  "lost → connect" path (mock spawn+health; the atomic `create_new` is under test).
- Health-wait timeout against a dead port → bounded error, not a hang.
- `#[cfg(unix)]` integration test that actually spawns the built `kamajid`
  detached, waits for health, hits `/healthz`, kills it (gated like Phase 1's
  `#[ignore]` live tests).

### `sse.rs`
- `events::from_sse(name, data)` round-trips with `Event::sse_name` + the
  daemon's `payload_json` framing (a shared `kamaji-core` test): every variant,
  daemon-frame → client-decode yields the original.
- Listener against the test daemon: subscribe, perform a command, assert the
  matching `SseMsg::Event`; drop the daemon → `Disconnected` then (after restart)
  `Connected`.

### `engine.rs`
Existing tests that asserted DB/worktree side effects are rewritten to assert the
**client call** made (a fake `DaemonClient`, or against the in-process test daemon
asserting board state via `list_tickets`). Keymap → `Effect` tests stay,
retargeted to the collapsed `Effect` set.

### Manual smoke (the phase's acceptance, parent spec §8)
Two TUIs stay in sync; kill the daemon → reconnect/respawn; attach execs native
zellij and returns cleanly; auto-review moves an idle agent's card to Review live
with no in-TUI poll loop.

### CI
Unchanged philosophy; new modules covered by `cargo test`. Keep the Windows
build green (auto-spawn is cross-platform; gate unix-only detach detail).

## 11. Small daemon additions this phase requires (explicit)

Three minimal daemon-side changes the implementation plan must budget:

1. **Daemon writes `kamajid.pid` + `kamajid.addr`** on successful bind (removed
   on clean shutdown), under the runtime dir — so the client can do
   liveness/stale detection and learn the real bound address. (~20 lines in
   `crates/kamajid/src/main.rs`.)
2. **`POST /projects/:id/main-session`** → `{ session_name }` — wraps
   `session::prepare_main_session` + `create_session_background`, idempotent if
   the session is live. Replaces the TUI's `main_session_effect` (§4.6).
3. **`PATCH /config`** — persist config edits through the single writer. (This
   was deferred from Phase 1e; Phase 2 needs it.)

Anything beyond these (a daemon-side **resume** endpoint, richer event payloads)
is deferred and filed as follow-up issues.

## 12. What stays out of Phase 2 (explicit)

- Browser UI / Datastar / TUI-via-zellij-web (Phase 3). The TUI uses native
  attach only; `AttachInfo`'s `web_url`/`token` are ignored by the TUI client.
- Auth, TLS, remote bind, multi-user (daemon stays `127.0.0.1`).
- New daemon features beyond the three §11 additions.
- Daemon-side conversation-resume (§6.2 follow-up).
- A negotiation/versioning protocol beyond §9.4.
- Cosmetic `engine.rs` module split (optional).

## 13. Owner decisions (resolved)

The owner delegated these to the spec author; resolved as follows (baked in above):

1. **Auto-spawn default-on** with `--daemon <addr>` and `--no-spawn` escape
   hatches. (Adopted.)
2. **Add `PATCH /config`** and route TUI config edits through it (single-writer
   purity; lets the daemon hot-reload affected behavior). (Adopted.)
3. **Resume simplification** — live-session attach is exact; post-reboot
   conversation-resume degrades to a plain attach and is a tracked follow-up
   (daemon-side resume endpoint). (Adopted.)
