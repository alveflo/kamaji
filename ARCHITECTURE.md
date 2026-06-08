# kamaji architecture

This is the **current-architecture** reference for kamaji: what the system is
today, in source. It is the doc to read first. The dated specs and plans under
`docs/superpowers/` are *historical design history* — the trail of how we got
here, not a description of the present (see [Relationship to
`docs/superpowers/`](#relationship-to-docssuperpowers)).

kamaji orchestrates AI agents as named [zellij](https://zellij.dev) sessions on
a per-project Kanban board. A single backend daemon owns all state and
git/zellij orchestration; a ratatui TUI and a web browser are both thin clients
of it. The browser is the first-class surface; the TUI is a leaner subset of
the same backend.

---

## At a glance

```
┌──────────────┐   HTTP commands + SSE events   ┌─────────────────────────────┐
│ ratatui TUI  │ ─────────────────────────────▶ │           kamajid           │
│  (kamaji)    │ ◀───────────────────────────── │   axum @ 127.0.0.1:8755     │
└──────────────┘     /events  (JSON SSE)         │                             │
┌──────────────┐   HTTP commands + SSE events    │  kamaji-core: DB, git,      │
│   Browser    │ ─────────────────────────────▶ │  zellij orchestration,      │
│  (Datastar)  │ ◀───────────────────────────── │  auto-review poll loop      │
└──────┬───────┘   /ui/events (Datastar SSE)      └──────────┬──────────────────┘
       │ iframe src = proxy origin                           │ spawns / ensures
       ▼                                                      ▼
┌──────────────────────────────┐  cookie-auth   ┌─────────────────────────────┐
│ zellij-web reverse proxy      │ ─────────────▶ │  zellij web @ 127.0.0.1:8082│
│ kamajid @ 127.0.0.1:8756      │   forward      │  (subprocess, detached)     │
└──────────────────────────────┘                └──────────┬──────────────────┘
                                                            ▼
                                              zellij sessions  kamaji-<N>-<slug>
```

Three crates, three ports, two transports (HTTP for commands, SSE for events),
one writer to the DB.

---

## Crates

The repo is a Cargo workspace (`Cargo.toml`, `resolver = "2"`) of three crates
under `crates/`:

| Crate | Kind | Responsibility |
|-------|------|----------------|
| **`kamaji-core`** | library | Pure domain logic. No UI, no transport. Owns the SQLite DB, shells out to `git`/`zellij`, generates zellij KDL layouts, renders agent command templates, and runs auto-review (idle-session) detection. Its public surface is **commands** (`create_ticket`, `move_ticket`, `start_session`, `attach_info`, …) plus a **query** surface for board state. |
| **`kamajid`** | binary (daemon) | Wraps `kamaji-core` behind an `axum` HTTP server. Owns the single DB connection (making "exactly one writer" true), the auto-review poll loop, the managed `zellij web` process, the authenticating reverse proxy, and an in-memory event broadcaster that fans board deltas to all clients. Renders the browser board as server-side HTML with `maud`. |
| **`kamaji`** | binary (the command the user runs) | Thin launcher + ratatui TUI host. Finds a live daemon (pidfile + health ping) or **auto-spawns** one detached, then connects. Renders the TUI as a daemon client: commands over HTTP, board state from the `/events` SSE stream. Subcommands are daemon API calls; one opens the browser. |

**Load-bearing property:** the UI layer never touches the DB or zellij
directly. Both the TUI and the browser go through the daemon. That is what makes
"one source of truth, live on both surfaces" real.

### Where things live

```
crates/
  kamaji-core/src/
    db.rs            SQLite-backed Kanban model (tickets, projects)
    models.rs        domain types
    config.rs        config schema + defaults (incl. daemon bind addr)
    events.rs        canonical Event enum + SSE name mapping
    paths.rs         runtime / cache / config dir resolution
    git.rs           worktree orchestration
    zellij*.rs       zellij CLI integration + KDL layout generation
    agent.rs         agent command templates
    session.rs       session lifecycle
    poll.rs          auto-review (idle-session) detection

  kamajid/src/
    main.rs          startup, bind-addr derivation, pid/addr files, proxy spawn
    lib.rs           router + serve()
    state.rs         AppState (db, config, broadcast tx, zellij managers)
    routes/
      healthz.rs     GET /healthz
      events.rs      GET /events       (JSON SSE — for the TUI)
      ui_events.rs   GET /ui/events    (Datastar SSE — for the browser)
      ui.rs          GET /, /ui/tickets/* (maud HTML + fragments)
      tickets.rs     ticket CRUD + session control
      assets.rs      GET /assets/* (rust-embed static files)
      pwa.rs         GET /manifest.webmanifest, /sw.js (installable-PWA wiring)
    assets/          embedded static files (CSS/JS, PWA manifest + icons + sw.js)
    views/           maud templates (page, board, card, modal, terminal, sidebar)
    zellij_web.rs    lazy `zellij web` server manager + token handling
    zellij_proxy.rs  authenticating reverse proxy (:8756)
    poll_task.rs     background auto-review loop (emits events)
    session_driver.rs session lifecycle seam (real + fake for tests)

  kamaji/src/
    main.rs          TUI entrypoint + run loop
    daemon.rs        daemon discovery, auto-spawn, health-check
    client.rs        blocking HTTP command client
    sse.rs           SSE listener thread + reconnect
    app/ engine/ ui/ theme/   TUI state and rendering
```

The browser board is an installable PWA. A `manifest.webmanifest` (served at
`/manifest.webmanifest`) plus a minimal service worker (`/sw.js`, root scope, no
caching — the live daemon is required, so there is no offline mode) make the
browser offer "Install". Both are embedded assets served by `routes/pwa.rs`; the
monogram icons live alongside the other static files in `crates/kamajid/src/assets/`
(regenerate the PNGs from the SVGs per `crates/kamajid/src/assets/icons.README.md`).

---

## Ports

All three bind to `127.0.0.1` by default (localhost-first; see
[Remote-future seams](#remote-future-seams)).

| Port | Bound by | Serves |
|------|----------|--------|
| **8755** | `kamajid` main listener | Board HTTP API + HTML page + both SSE streams. The board address; configurable via `[daemon] bind` in config (default `127.0.0.1:8755`). The bound address is written to the **addrfile** so clients can find it. |
| **8756** | `kamajid` proxy listener | Reverse proxy in front of `zellij web`. **Auto-derived** as *board port + 1* by `derive_proxy_addr()` in `kamajid/src/main.rs`. Owns a separate origin so its host-relative URLs (`/ws/*`, `/assets/*`) don't collide with board routes, and so it can set the auth cookie the iframe needs. |
| **8082** | `zellij web` (subprocess) | The actual browser terminal — `zellij`'s own web client. Launched lazily by `kamajid` on first attach; detached, so it persists across daemon restarts. Visiting `:8082/<session-name>` creates / attaches / resurrects that named zellij session. |

kamaji builds **no terminal emulator** — the terminal-in-browser problem is
entirely zellij web's job. Because every ticket session is named deterministically
(`kamaji-<N>-<slug>`), "attach in the browser" reduces to a URL.

---

## Command + event flow: commands down, events up

### Commands (HTTP)

Commands are HTTP requests against `kamajid`, which calls into `kamaji-core`,
mutates the DB, and then broadcasts an event. Examples (handlers in
`kamajid/src/routes/`):

- `POST /tickets`, `PATCH /tickets/:id`, `DELETE /tickets/:id`
- `POST /tickets/:id/move` (change column / status)
- `POST /tickets/:id/start` (create/resurrect the agent session)
- `POST /tickets/:id/done`, `POST /tickets/:id/attach`
- project CRUD, `GET`/`PATCH /config`

The TUI issues these via a blocking HTTP client (`kamaji/src/client.rs`); the
browser issues them via Datastar actions (form posts / fetches).

### Events (SSE)

State changes flow back out as a single broadcast (`tokio::sync::broadcast`)
fanned to two SSE endpoints. The canonical event type is the `Event` enum in
`kamaji-core/src/events.rs` (variants: `TicketCreated`, `TicketUpdated`,
`TicketMoved`, `TicketDeleted`, `SessionStarted`, `SessionIdle`,
`SessionExited`, `SessionSignal`), each with a dotted SSE name
(`ticket.created`, `session.idle`, …).

- **`GET /events` — JSON SSE, for the TUI.** Each event is framed as
  `event: <dotted-name>` + `data: <json>`. Lossy by design: a client that lags
  past the broadcast buffer drops events and re-fetches the board on reconnect.
- **`GET /ui/events` — Datastar SSE, for the browser.** Each event becomes a
  `datastar-patch-elements` frame carrying a re-rendered HTML fragment. The
  fragments are produced by the **same `maud` view functions** that render the
  initial page (`views/board.rs`, `views/card.rs`, …), so patches are
  byte-identical to first paint — no drift between initial render and live
  updates. A `TicketMoved` re-renders the affected columns; `TicketDeleted`
  removes the card; `SessionSignal` updates the card's working indicator.

This is the "LiveView feel" without LiveView: the server owns state and pushes
deltas; clients are dumb renderers. Move a card in the browser → POST →
server re-renders columns → the change arrives at every other tab *and* the TUI
over their respective SSE streams.

---

## Daemon lifecycle

`kamajid` is auto-spawned on demand by the `kamaji` launcher and outlives the
client that started it.

### Pidfile, addrfile, health

On startup `kamajid` writes two files into the runtime dir
(`<XDG_RUNTIME_DIR | XDG_CACHE_HOME | ~/.cache>/kamaji/`, resolved by
`kamaji-core::paths`):

- **`kamajid.pid`** — the daemon PID (also used as a startup lock; see below).
- **`kamajid.addr`** — the actually-bound address, so clients connect to the
  right port even if the default was overridden or `:0` was used.

`GET /healthz` returns `{ "ok": true, "version": <CARGO_PKG_VERSION> }` — a
cheap liveness probe with no deep dependencies. The TUI uses the version to warn
on client/daemon skew. On clean shutdown the pid/addr files are removed.

### Discovery + auto-spawn (`kamaji/src/daemon.rs`)

`ensure_daemon()`:

1. **Probe existing.** Read the pidfile; check the PID is alive (`kill(pid, 0)`
   on Unix); hit `/healthz`. On success, connect and return. On failure, treat
   the pid/addr files as stale and remove them.
2. **Acquire the start lock.** Create the pidfile with `O_CREAT | O_EXCL`. The
   winner of this atomic create spawns the daemon; a loser (got `AlreadyExists`)
   instead waits for the winner's `/healthz` to come up (bounded, default ~5s).
   If the winner crashes before health, the loser clears the stale files and
   retries once.
3. **Spawn detached.** `kamajid serve --bind <addr>` is launched fully
   detached — `setsid()` on Unix / `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`
   on Windows, stdio to null — then the launcher polls `/healthz` until ready
   and connects.

A `--daemon <addr>` flag short-circuits all of this and connects to an explicit
address.

> **Gotcha (env inheritance):** when the daemon is itself launched from inside a
> zellij session it inherits `ZELLIJ*` env vars, which makes
> `zellij … --create-background` no-op. Spawned zellij commands scrub `ZELLIJ*`
> from their environment to avoid this.

### Reboot persistence & autostart

Agent sessions survive a reboot without any explicit save step, because every
piece of state they need outlives the reboot:

- **zellij sessions** — zellij serializes each session (layout + the command in
  each pane) to its cache folder (`~/.cache/zellij`) once per second. After a
  reboot they appear in `zellij list-sessions` as `EXITED - attach to resurrect`.
  kamaji *depends* on this, so the config it generates for created sessions
  forces `session_serialization true` (alongside `web_sharing "on"`) in
  `kamaji-core::zellij_config` — a user who disabled serialization would
  otherwise silently lose persistence.
- **the ticket → session mapping** — the SQLite DB in `~/.local/share/kamaji/`.
- **the agent's work** — the git worktree on disk.

Everything volatile is regenerated: the pid/addr files in the runtime dir and
the single-use layout / web-config files under `$TMPDIR`.

Recovery is the *existing* attach path, not new machinery: `reconcile` keeps a
ticket's `session_name` as long as the name is still listed (an `EXITED` stub
counts), and opening the ticket runs `resurrect_session` — which relaunches the
agent's configured `resume` argv (`claude resume`, `codex resume --last`, …) in
the original worktree. The conversation continues.

The one thing that does **not** restart itself is the daemon (and therefore the
browser board on :8755). `make install-service` installs a release `kamajid` to
`~/.local/bin` and a **systemd user unit** (`packaging/systemd/kamajid.service`)
that `enable --now`s the daemon and enables lingering, so the board is back up
automatically after login — and after a cold boot, before any interactive login.
(`zellij web` still spawns lazily on first attach, so it needs no unit of its
own.)

---

## zellij web + the reverse proxy

The browser's terminal is `zellij`'s own web client; kamaji's job is to *manage*
it and make it embeddable.

### `zellij web` manager — `kamajid/src/zellij_web.rs`

- Lazily spawns the `zellij web` subprocess on first attach and polls the port
  until reachable (bounded). The subprocess is detached, so it survives daemon
  restarts.
- Mints and caches a login **token** (`zellij web --create-token`). Tokens
  persist in zellij's own store across server restarts.
- `attach_info(session_name)` returns the browser-open URL
  (`:8082/<session_name>`) plus the token.
- A `fake()` constructor returns canned values with no subprocess, used by CI
  and integration tests where no `zellij` binary is present.

### Authenticating reverse proxy — `kamajid/src/zellij_proxy.rs`

`zellij web` would otherwise show a per-browser token modal, and its
host-relative URLs collide with the board's. So `kamajid` runs a small reverse
proxy on the board port + 1 (`:8756`):

1. **Logs in once, server-side,** with the zellij web token
   (`POST <upstream>/command/login`), and caches the resulting `session_token`
   cookie.
2. **Injects that cookie** into every upstream request — both plain HTTP
   forwards and WebSocket handshakes.
3. **Pipes WebSockets frame-by-frame** to the upstream (`/ws/*` — zellij's
   browser client uses `/ws/terminal/<session>` and `/ws/control`).

Because the proxy owns a distinct same-origin origin, the board can embed a
session in an `<iframe>` whose `src` points at `:8756`, pre-authenticated by the
cached cookie — no token prompt. (If a session can't be iframed, the attach can
fall back to opening a tab.)

---

## Remote-future seams

kamaji is localhost-first but built so a remote/internet-facing mode is
*configuration*, not a re-architecture:

- **Bind address and auth are config, not assumptions.** The daemon binds
  `127.0.0.1` today; a `bind` setting flips it. There is no shared mutable state
  outside the daemon, so multi-client is already true — multi-user becomes an
  auth/identity layer on top.
- **zellij web already handles the hard part of remote terminals** (bind IP,
  TLS cert/key, `base_url` for reverse proxies, login tokens). "Remote sessions"
  is mostly zellij config kamaji passes through.

---

## Container mode

Native (daemon + agents on the host) remains the default and is unchanged. As an
opt-in alternative, a `kamaji up`/`down` launcher runs the whole daemon (board,
proxy, managed `zellij web`, all agent sessions) inside one container — the
sandbox boundary for "agents with root, host protected." It is a
transport/packaging layer over the same daemon: the board binds `0.0.0.0` (via
the image CMD, so the native config bind is untouched) and publishes 8755/8756
(8082 stays internal); project roots and worktree dirs are bind-mounted at
identical paths (so worktree links and XDG paths resolve on both sides); a state
marker in the runtime dir tells the client to connect to the container instead of
spawning a local daemon, and `kamaji status` shows which mode is active. See
`docs/superpowers/specs/2026-06-06-containerize-kamaji-design.md`.

---

## Build, run, test

```sh
make start      # build kamajid + start it in the background (board :8755, proxy :8756)
make restart    # rebuild + relaunch — the one you want after pulling new code
make stop       # stop the running daemon
make status     # is the daemon responding? (GET /healthz)
make logs       # follow /tmp/kamajid.log

cargo build                       # whole workspace
cargo test                        # whole workspace
cargo fmt && cargo clippy         # CI runs fmt + clippy + test on PRs
```

The launcher auto-spawns the daemon, so running the `kamaji` TUI does not
require `make start` first; `make start`/`restart` are for running (and
refreshing) the daemon that serves the **browser** board.

Testing strategy: `kamaji-core` is the most-tested layer (unit tests, with
zellij/git exercised via temp-dir KDL assertions — no real zellij needed).
`kamajid` has integration tests that boot the daemon on an ephemeral port, drive
commands, and assert both resulting state and the emitted SSE deltas. The view
layer is verified by rendering assertions.

---

## Relationship to `docs/superpowers/`

`docs/superpowers/specs/` and `docs/superpowers/plans/` are **historical design
history** — a dated, append-only record of brainstorms, specs, and
implementation plans, *partially superseded* as the system evolved. They explain
*why* kamaji is shaped the way it is, but they do **not** describe the system as
it stands today and should not be read as current truth.

The most important pivot is captured in
`docs/superpowers/specs/2026-05-27-browser-first-pivot-design.md`, which turned
the original single TUI binary into the three-crate, browser-first daemon
architecture described above (its §3–§5 are the raw material for this doc).

**This file (`ARCHITECTURE.md`) is the source of truth for the current
architecture.** When the architecture changes, update this file.
