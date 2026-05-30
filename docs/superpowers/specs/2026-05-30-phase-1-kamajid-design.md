# kamaji — Phase 1: `kamajid` Daemon Design

- **Date:** 2026-05-30
- **Status:** Approved
- **Author:** Victor Alveflo
- **Parent spec:** `docs/superpowers/specs/2026-05-27-browser-first-pivot-design.md` (§8 Phase 1)
- **Precondition:** Phase 0 merged (`kamaji-core` library extracted; binary unchanged behavior).

## 1. Overview

Phase 1 stands up **`kamajid`** — the shared backend daemon described in the
browser-first pivot spec — as a new Cargo workspace member, exposing a localhost
HTTP API and an SSE event stream over the existing `kamaji-core` domain logic.
It also moves the **auto-review poll loop** into `kamaji-core` (so both the
daemon and the TUI use the same canonical implementation) and gives the daemon
ownership of the **`zellij web`** subprocess.

The TUI is deliberately **not touched as a client** in Phase 1: it still drives
`kamaji-core` directly, still runs its own in-process poll, still attaches
natively. The daemon runs alongside it without dependency in either direction.
Phase 2 will flip the TUI to be a client; Phase 3 adds the browser. Keeping
those out of Phase 1 lets us build and test the daemon and the API in isolation.

### Goals
- A runnable `kamajid` binary serving the full board command surface and a live
  event stream over localhost.
- A single canonical poll-loop implementation in `kamaji-core`, used by the
  daemon today and reusable by the TUI's existing call sites.
- Daemon-managed `zellij web` so future browser attaches are a single click.
- Zero behavior change for `kamaji` (the TUI binary): existing 152 binary +
  83 core = 235 tests still pass.

### Non-goals (Phase 1)
- TUI talking to the daemon (Phase 2).
- Daemon auto-spawn (Phase 2).
- Browser UI (Phase 3).
- Auth, TLS, remote bind (deferred; binds `127.0.0.1` only).

## 2. Decisions (from brainstorming)

| Question | Decision |
|----------|----------|
| Phase 1 scope | Daemon + HTTP API + SSE + poll-loop move + `zellij web` mgmt + minimal logging — all in |
| Poll-loop strategy | Extract a `PollLoop` runner into `kamaji-core`; daemon uses it; TUI keeps its existing in-process call (rewired through core) |
| Daemon binary | New `crates/kamajid/` workspace member (its own binary, its own Cargo.toml) |
| API style | REST + JSON |
| Event emission | Daemon layer emits events after calling `kamaji-core` commands; core stays a library of pure functions |
| SSE transport | `tokio::sync::broadcast` fan-out, lossy by design |
| Default port | `127.0.0.1:8755` (free; no clash with zellij web's `8082`) |
| `zellij web` start policy | Lazy — first `/tickets/:id/attach` ensures it's running |

## 3. Crate & module shape

Add `crates/kamajid/` as a new workspace member. `kamaji-core` gains two new
modules; the binary `kamaji` is rewired internally but its behavior is unchanged.

```
kamaji/                                  repo root (Cargo workspace)
├── Cargo.toml                           workspace members += "crates/kamajid"
├── crates/
│   ├── kamaji-core/
│   │   └── src/
│   │       ├── lib.rs                   pub mod events; pub mod poll;
│   │       ├── events.rs                NEW — Event enum, Serialize/Deserialize
│   │       ├── poll.rs                  NEW — PollLoop runner (extracted)
│   │       └── …existing modules…
│   ├── kamaji/                          (binary, unchanged behavior)
│   │   └── src/engine.rs                detect_tick now delegates to kamaji_core::poll
│   └── kamajid/                         NEW
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs                  CLI + tokio runtime + bind + tracing init
│           ├── lib.rs                   pub fn serve(addr, db, config) for tests
│           ├── state.rs                 AppState { db, config, tx, zellij_web }
│           ├── error.rs                 IntoResponse for anyhow / domain errors
│           ├── routes/
│           │   ├── mod.rs               router::build(state) -> Router
│           │   ├── healthz.rs
│           │   ├── projects.rs
│           │   ├── tickets.rs
│           │   ├── config.rs
│           │   ├── attach.rs
│           │   └── events.rs            SSE handler (subscribes to broadcast rx)
│           └── zellij_web.rs            start/stop + token-cache manager
└── docs/, .github/, install.sh, …      unchanged
```

### `kamaji-core::events`

A single enum, `Event`, serialized to JSON for SSE. Payloads are minimal:
identifiers + the fields that changed; clients re-fetch the full object if they
need it.

```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Event {
    TicketCreated(Ticket),
    TicketUpdated(Ticket),
    TicketMoved { id: i64, from: Status, to: Status, at: String },
    TicketDeleted { id: i64 },
    SessionStarted { ticket_id: i64, session_name: String },
    SessionIdle    { ticket_id: i64 },
    SessionExited  { ticket_id: i64, session_name: String },
}
```

(SSE wire format applies a different shape — see §5 — but the `Event` type is
the in-process source of truth.)

### `kamaji-core::poll`

Extracts today's `Engine::detect_tick` (in `crates/kamaji/src/engine.rs`,
~150 lines around line 289) into a self-contained runner. The runner owns the
per-ticket detection state that currently lives on `Engine`
(`auto_review_ids: HashSet<i64>`, `scrape_hash: HashMap<i64, Option<u64>>`,
`last_level: HashMap<i64, SignalLevel>`). Public API:

```rust
pub struct PollLoop { /* the three state maps above */ }

impl PollLoop {
    pub fn new() -> Self;
    pub fn rehydrate(&mut self, tickets: &[Ticket]);  // from auto_reviewed column
    pub fn tick(&mut self, db: &Db, config: &Config, state_dir: &Path)
        -> anyhow::Result<Vec<Event>>;
    pub fn forget_ticket(&mut self, id: i64);
}
```

The TUI's `Engine` keeps its existing field set but `detect_tick` becomes:

```rust
pub fn detect_tick(&mut self) -> Result<()> {
    let events = self.poll.tick(&self.db, &self.config, &self.state_dir)?;
    for ev in events { self.apply_event_to_ui(ev); }
    Ok(())
}
```

`apply_event_to_ui` does what `detect_tick`'s tail does today (update
`app.tickets`, set toasts, etc.). Net effect: same behavior, one canonical
implementation.

## 4. HTTP API (REST, JSON, no auth)

Daemon binds `127.0.0.1:8755` by default. All bodies are JSON.

```
GET    /healthz                                 200 { ok: true, version }
GET    /events                                  text/event-stream (see §5)

GET    /projects                                [Project]
POST   /projects                                Project   (create from body)
GET    /projects/:id                            Project
GET    /projects/:id/tickets                    [Ticket]

POST   /tickets                                 Ticket    (create from body)
GET    /tickets/:id                             Ticket
PATCH  /tickets/:id                             Ticket    (edit title/desc/prompt/agent)
POST   /tickets/:id/move        { target }      Ticket
POST   /tickets/:id/start                       SessionInfo
POST   /tickets/:id/attach                      AttachInfo (ensures zellij web)
POST   /tickets/:id/done        { cleanup }     Ticket
DELETE /tickets/:id                             204

GET    /config                                  Config
PATCH  /config                                  Config
```

`SessionInfo = { ticket_id, session_name }`
`AttachInfo = { session_name, web_url, token }`

### Handler shape

Each handler is a thin wrapper:

1. Deserialize body / path params.
2. Call the matching `kamaji-core` function inside `tokio::task::spawn_blocking`
   (rusqlite is sync).
3. On success, push the right `Event` variant to the broadcast channel.
4. Return JSON.

Keeping event emission at the daemon layer (not pushed into core function
signatures) avoids churning the `kamaji-core` public API for Phase 1. If Phase 3
demands richer event semantics, refactoring core to return events is a focused
follow-up — not a precondition.

### Error handling

`error.rs` implements `IntoResponse` for an internal `ApiError` that wraps
`anyhow::Error` and a small set of domain conditions (NotFound, Conflict,
BadRequest). HTTP statuses map: 404 / 409 / 400 / 500. Bodies are
`{ "error": "...", "kind": "not_found" }`.

## 5. SSE event stream (`GET /events`)

Each `Event` is sent as a named SSE event:

```
event: ticket.moved
data: {"id":5,"from":"in_progress","to":"review","at":"2026-05-30T10:23:45Z"}

event: session.idle
data: {"ticket_id":5}
```

The Rust `Event` type's `#[serde(tag = "type", content = "data")]` tagged form
is the in-process representation, but it is **not** what goes on the SSE wire.
The SSE serializer is a small helper that splits an `Event` into two pieces:

- `event:` line — dotted lowercase name (`ticket.created`, `ticket.moved`,
  `ticket.updated`, `ticket.deleted`, `session.started`, `session.idle`,
  `session.exited`) — easier for browser-side filtering than digging into a JSON
  `type` field, and conventional for SSE consumers (`addEventListener("ticket.moved", …)`).
- `data:` line — the inner payload only, serialized as JSON (i.e. just the
  struct-like contents of the variant, no `type` / `data` envelope).

Concretely: `Event::TicketMoved { id: 5, from: in_progress, to: review, at }`
becomes the two-line SSE record shown above; the JSON does not include the
`"type"` discriminator (the `event:` line already carries it).

### Fan-out

`AppState.tx: tokio::sync::broadcast::Sender<Event>` with a moderate buffer
(64). Each `/events` connection subscribes to a fresh `Receiver` and converts
incoming events into SSE frames.

**Lossy by design.** If a slow client lags and the broadcast drops events, the
client's stream surfaces `Lagged(n)`; the SSE handler closes the connection
cleanly. The client reconnects and re-syncs by re-fetching the affected
resources (`GET /projects/:id/tickets`). For a single-user localhost daemon
this is overwhelmingly safer than blocking the broadcaster on a slow consumer.

### Heartbeat / keepalive

axum's `Sse::keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))` to
defeat intermediary timeouts and so a dead-connection-on-the-client side is
detected promptly.

## 6. `zellij web` management

`kamajid::zellij_web::ZellijWeb` owns the subprocess and a cached token. It's a
**daemon concern, not a core concern** — `kamaji-core` knows nothing about
`zellij web`.

### Lazy ensure-running

`zellij web` is not started by `kamajid serve` itself. The **only** path that
triggers `ensure_running` is the `POST /tickets/:id/attach` route. (No
`/healthz` probe, no startup-time check, no implicit lift from any other
route.) The route handler calls `zellij_web.attach_info(&session_name)`, which
internally does:

1. If no cached token, run `zellij web --create-token` once, parse the printed
   token, store it in memory. Tokens persist across `zellij web` restarts
   (zellij stores them in its own DB) so a cached one usually keeps working.
2. Probe `http://127.0.0.1:8082/` — if not reachable, `tokio::process::Command::new("zellij").arg("web").spawn()` and hold the `Child`. Poll the socket until ready (timeout ~3s).
3. Return AttachInfo `{ session_name, web_url: "http://127.0.0.1:8082/<session>", token }`.

### Shutdown

On daemon `SIGTERM`/`SIGINT`, `Child::kill()` (SIGTERM equivalent) the held
`zellij web` process and wait briefly. If the daemon dies unexpectedly, the
`zellij web` server keeps running; the next daemon start finds it and reuses
it (the probe succeeds without spawning).

### Token cache invalidation

If a user manually wipes tokens (`zellij web --delete-token`), our cached one
becomes invalid. Detection: AttachInfo paths follow up with a health probe
using the cached token; on 401-equivalent failure, invalidate cache and
request a fresh token before returning. Exact 401 shape is a spike during
implementation.

### Per-client attach semantics

`AttachInfo`'s contents are used differently per client (Phase 2/3 detail, but
worth stating here):
- **Browser (Phase 3):** opens / iframes `web_url`; `token` flows through the
  zellij web login page.
- **TUI (Phase 2):** ignores `web_url`/`token`; uses `session_name` to do a
  native `zellij attach`.

In Phase 1 no real client exists — the route is exercised by integration tests.

## 7. Daemon CLI & config

```
kamajid serve [--bind 127.0.0.1:8755]
              [--log-format human|json]
              [--log-level off|error|warn|info|debug|trace]
kamajid --version
kamajid --help
```

`kamaji-core::config::Config` gains an optional `[daemon]` section read on
startup:

```toml
[daemon]
bind = "127.0.0.1:8755"        # default
log_format = "human"           # default
log_level = "info"             # default
```

CLI flags override config; config overrides defaults. Standard `RUST_LOG` /
`KAMAJID_LOG` env vars honored by `tracing_subscriber::EnvFilter`.

## 8. Logging & observability

- `tracing` for instrumentation; `tracing_subscriber` for output. Two
  formatters: `human` (ANSI single-line, default) and `json` (one line per
  event, ready for log shipping if Phase 4+ goes remote).
- A per-request middleware (`tower_http::trace::TraceLayer`) logs method,
  path, status, latency.
- Poll-loop ticks log at `debug` to avoid spam; emitted events log at `info`
  with the event type + entity id.
- `GET /healthz` returns `{ "ok": true, "version": env!("CARGO_PKG_VERSION") }`.
  No deep dependency checks (DB/zellij) in Phase 1 — overkill for localhost.

## 9. Async / DB strategy

`rusqlite` is sync; axum is async. **Wrap every `kamaji-core` call in
`tokio::task::spawn_blocking`.** Minimal new dependency surface, fine for a
single-user daemon. If contention becomes a problem in Phase 4 (multi-user,
remote), revisit with `tokio_rusqlite` or a connection pool — but YAGNI here.

`AppState.db: Arc<Mutex<Db>>` — a single connection, serialized writes via the
mutex inside `spawn_blocking`. SQLite WAL means concurrent readers stay non-
blocking; the mutex protects the in-memory `Db` wrapper, not the file.

## 10. Dependencies

`crates/kamajid/Cargo.toml`:

```toml
[dependencies]
kamaji-core = { path = "../kamaji-core" }
axum = { version = "0.7", features = ["macros"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "signal", "process"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["trace"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
anyhow = "1"
chrono = { version = "0.4", default-features = false, features = ["serde", "now"] }
futures = "0.3"   # for the SSE stream combinators

[dev-dependencies]
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "stream"] }
tempfile = "3"
```

`kamaji-core/Cargo.toml` gains:

```toml
chrono = { version = "0.4", default-features = false, features = ["serde", "now"] }
```

(Used by `events::TicketMoved.at` and for any future timestamp fields. The
existing `chrono`-free `created_at: String` columns stay; we serialize the new
fields with `chrono::Utc::now().to_rfc3339()`.)

## 11. Testing

### `kamaji-core` (unit)
- `events`: serde round-trip for every variant.
- `poll::tick`: extraction of today's `Engine::detect_tick` tests; same
  fixtures (tempdir state_dir, synthetic Claude idle markers, hand-built
  `Db` + `Ticket` rows). Existing 83 core tests + new ones for the runner.
- `Engine::detect_tick` test in the binary is updated to call the rewired
  path — observable behavior unchanged.

### `kamajid` (integration, `crates/kamajid/tests/`)

Helper:

```rust
struct TestDaemon { base_url: String, db_path: PathBuf, _join: JoinHandle<()> }
fn spawn() -> TestDaemon { /* tempdir DB + config; bind 127.0.0.1:0; tokio runtime */ }
```

Tests use plain `reqwest` for commands and `reqwest::Response::bytes_stream`
for SSE (a tiny inline SSE parser is enough; avoid a new dep). Coverage:

- One happy-path test per HTTP route: hit endpoint → assert response → assert
  the matching SSE event arrives within a short timeout (typically <100ms).
- Negative path per route: invalid body → 400 with `kind: "bad_request"`;
  missing entity → 404 with `kind: "not_found"`.
- Poll loop: drop a Claude idle marker file in the test state_dir, call
  `PollLoop::tick` directly OR set a tiny poll interval and wait, assert
  `session.idle` arrives on `/events`.
- Broadcast lag: subscribe with a deliberately slow consumer, drown the
  broadcaster with N+1 events, assert the connection closes cleanly (no
  daemon panic).
- `/healthz`: returns `{ "ok": true, "version": "..." }`.

### `zellij web`

Hard to test without zellij installed. Two-pronged:
- Unit-test the URL-building, token caching state machine, and 401-invalidate
  logic with the subprocess mocked behind a small trait.
- `#[ignore]`d end-to-end test (`cargo test -- --ignored`) for the live path:
  ensure `zellij web` starts, an HTTP probe returns 200, AttachInfo includes
  a token. Skipped in CI unless we add a zellij-installation step.

### CI
- The existing `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-targets --all-features` cover the new crate.
- Windows build job already runs `cargo test`; gate the `zellij web` test to `#[cfg(unix)]` since zellij web is unix-only today. Daemon itself compiles on Windows.

## 12. Staged-commit order (one Phase 1 plan, six bite-sized commits)

Each commit ends green; the daemon is independently runnable from step 2.

1. **Extract `events` + `poll` into `kamaji-core`;** rewire `Engine::detect_tick`. Pure refactor, zero behavior change. Existing 235 tests still pass; new unit tests for `poll::tick` and `events` round-trip.
2. **Scaffold `kamajid` crate** with `state.rs`, `error.rs`, `/healthz`, the CLI binary, logging init. `kamajid serve` runs and answers `GET /healthz`.
3. **HTTP command routes** (projects + tickets + config CRUD) + broadcast wiring + `/events` SSE. Each route emits its event after the core call.
4. **Run `PollLoop` in the daemon** as a background tokio task; hook emitted events into the broadcast.
5. **`zellij web` manager + `/tickets/:id/attach`** route, with the lazy ensure-running + token cache.
6. **Final polish:** tracing keepalive intervals, `/healthz` returns version, integration-test coverage filled in, smoke-run, push.

## 13. Risks & open items

- **Async vs sync DB.** `spawn_blocking` is the default; revisit only if measurable contention.
- **Broadcast capacity (64).** Picked by feel; tune if integration tests show false `Lagged` under realistic load.
- **`zellij web` token shape after manual invalidation.** Detection of the 401-equivalent needs a quick spike against zellij 0.43+.
- **Windows.** Daemon compiles; `zellij web` path is `#[cfg(unix)]`-gated. The Windows CI build job should stay green.
- **`Engine` rewire.** The TUI's behavior must be byte-identical to today. The poll-extraction commit is the riskiest and gets the most-explicit "verify all 235 tests pass" gate.

## 14. What stays out of Phase 1 (explicit)

- No TUI ↔ daemon coupling (Phase 2).
- No auto-spawn (Phase 2).
- No browser UI (Phase 3).
- No auth / TLS / remote bind (deferred — daemon binds `127.0.0.1` only).
- No deep `/healthz` checks (DB/zellij liveness) — overkill for localhost.
- No split of `engine.rs` (still ~2069 lines; that's a Phase 2+ concern when
  many of its responsibilities move to API calls).
