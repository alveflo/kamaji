# Phase 1b — `kamajid` Daemon: Scaffold + HTTP API + SSE Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up a runnable `kamajid` daemon — a new workspace crate that serves the kamaji board over a localhost JSON HTTP API and a live SSE event stream, backed by the `kamaji-core` library (DB + the `Event` type from Plan 1a).

**Architecture:** A new `crates/kamajid` binary built on `axum` 0.7 + `tokio`. An `AppState` holds the SQLite `Db` (behind a `Mutex`, accessed via `spawn_blocking` since rusqlite is sync) and a `tokio::sync::broadcast` channel of `kamaji_core::events::Event`. HTTP command handlers call `kamaji-core` and emit the matching `Event` onto the broadcast; the `/events` SSE handler subscribes and frames each event as `event:`/`data:` lines. This plan covers the **DB-and-broadcast-only** surface — no git/zellij/external processes — so every route is integration-testable in CI. The session-orchestration routes (`/start`, `/done` cleanup, `/attach`) and the auto-review poll background task are **Plan 1c**.

**Tech Stack:** Rust 2021, `axum` 0.7, `tokio` (multi-thread runtime), `tower-http` (tracing layer), `tokio-stream` (broadcast→stream), `tracing`/`tracing-subscriber`, `serde`/`serde_json`, `chrono`; dev: `reqwest` (async client for integration tests), `tempfile`.

**Parent spec:** `docs/superpowers/specs/2026-05-30-phase-1-kamajid-design.md` (§3 crate shape, §4 HTTP API, §5 SSE, §7 CLI/config, §8 logging, §9 async/DB, §10 deps, §11 testing). This plan implements the daemon scaffold + the non-orchestration routes; it explicitly defers §6 (`zellij web`) and the poll task to Plan 1c.

**Precondition:** Plan 1a merged (`kamaji-core` has serde models, `events::Event`, `poll::PollLoop`). On `main`, `cargo test --all-targets --all-features` reports 152 (kamaji) + 95 (kamaji-core) = 247 passing.

**Carried-forward notes from Plan 1a's final review (relevant to Plan 1c, recorded here so they aren't lost):** `PollLoop::tick(tickets, db, config, state_dir)` takes the ticket list explicitly — the daemon's poll task (Plan 1c) must `db.list_tickets(...)` fresh each tick; the daemon entry point is `tick`, not the `apply` test-seam. A move emits only `ticket.moved` and an edit only `ticket.updated` (no double-emit) — this plan's `/move` and `PATCH` routes honor that.

**Repo conventions (from `CLAUDE.md`):** all work on a branch in a worktree (the executing skill sets this up), never on `main`. Commit style mirrors history (`feat(kamajid): …`). Ship at the end with `gh pr create --fill --base main` → `gh pr merge --squash --auto --delete-branch` (the `--delete-branch` step errors from inside a worktree but the merge still lands — verify with `gh pr view` and clean up manually).

---

## Verification commands (run at every checkpoint)

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

These mirror `.github/workflows/ci.yml`. Per-binary counts:

```bash
cargo test --all-targets --all-features 2>&1 | grep -E '(test result:|running [0-9]+ tests|Running )'
```

The existing 247 tests must keep passing; this plan adds new `kamajid` tests on top.

---

## File Structure (after this plan)

```
Cargo.toml                                  workspace members += "crates/kamajid"
crates/kamaji-core/src/config.rs            MODIFIED — add optional [daemon] section
crates/kamajid/
├── Cargo.toml                              NEW
├── src/
│   ├── main.rs                             NEW — CLI parse, tracing init, open db, bind, serve
│   ├── lib.rs                              NEW — pub fn router(state)->Router, pub async fn serve(...)
│   ├── state.rs                            NEW — AppState { db, config, tx } + with_db()/emit()
│   ├── error.rs                            NEW — ApiError + IntoResponse
│   └── routes/
│       ├── mod.rs                          NEW — module declarations
│       ├── healthz.rs                      NEW — GET /healthz
│       ├── events.rs                       NEW — GET /events (SSE)
│       ├── projects.rs                     NEW — GET/POST /projects, GET /projects/:id, GET /projects/:id/tickets
│       ├── tickets.rs                      NEW — GET/POST /tickets, GET/PATCH/DELETE /tickets/:id, POST /tickets/:id/move
│       └── config.rs                       NEW — GET /config
└── tests/
    └── api.rs                              NEW — integration tests (boot daemon on 127.0.0.1:0)
```

Each route module owns one resource. `state.rs` owns the shared state + the DB-on-blocking-pool helper. `error.rs` owns the HTTP error mapping. This keeps every file small and single-purpose.

---

## Task 1: Scaffold the `kamajid` crate (`/healthz`, AppState, CLI, `[daemon]` config)

Stand up a runnable daemon that answers `GET /healthz`. This task wires the crate, the shared state, the binary entrypoint, and a backward-compatible `[daemon]` config section.

**Files:**
- Modify: `Cargo.toml` (workspace members)
- Modify: `crates/kamaji-core/src/config.rs`
- Create: `crates/kamajid/Cargo.toml`, `crates/kamajid/src/{main,lib,state,error}.rs`, `crates/kamajid/src/routes/{mod,healthz}.rs`, `crates/kamajid/tests/api.rs`

- [ ] **Step 1: Add the `[daemon]` config section to `kamaji-core`**

In `crates/kamaji-core/src/config.rs`, add a `DaemonConfig` struct and a field on `Config`. Place the struct after the `AutoReview` block (around line 68) and add helper defaults near the other `default_*` fns (around line 38):

```rust
fn default_bind() -> String {
    "127.0.0.1:8755".to_string()
}
fn default_log_format() -> String {
    "human".to_string()
}
fn default_log_level() -> String {
    "info".to_string()
}

/// Daemon (`kamajid`) settings. Entirely optional and defaulted, so configs
/// written before the daemon existed still load unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_log_format")]
    pub log_format: String,
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        DaemonConfig {
            bind: default_bind(),
            log_format: default_log_format(),
            log_level: default_log_level(),
        }
    }
}
```

Then add the field to `Config` (after `auto_review`, around line 93):

```rust
    #[serde(default)]
    pub auto_review: AutoReview,
    #[serde(default)]
    pub daemon: DaemonConfig,
```

And add `daemon: DaemonConfig::default(),` to `Config`'s `Default` impl (after `auto_review: AutoReview::default(),`).

- [ ] **Step 2: Write the failing config test**

Add to `crates/kamaji-core/src/config.rs`'s test module:

```rust
    #[test]
    fn daemon_section_defaults_and_loads_when_absent() {
        // A config.toml predating the daemon must load with daemon defaults.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "default_agent = \"claude\"\nbase_branch = \"auto\"\n\
             [agents.claude]\nwith_prompt = [\"claude\", \"{prompt}\"]\nno_prompt = [\"claude\"]\n\
             [agents.codex]\nwith_prompt = [\"codex\", \"{prompt}\"]\nno_prompt = [\"codex\"]\n\
             [agents.copilot]\nwith_prompt = [\"copilot\", \"{prompt}\"]\nno_prompt = [\"copilot\"]\n",
        )
        .unwrap();
        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded.daemon.bind, "127.0.0.1:8755");
        assert_eq!(loaded.daemon.log_format, "human");
        assert_eq!(loaded.daemon.log_level, "info");
    }
```

Run: `cargo test -p kamaji-core config::tests::daemon_section_defaults_and_loads_when_absent`
Expected: FAIL until Step 1's struct compiles, then PASS.

- [ ] **Step 3: Verify the config change**

```bash
cargo test -p kamaji-core config::tests
```
Expected: PASS (all config tests, incl. the new one). The `kamaji` TUI binary is unaffected — `daemon` is `#[serde(default)]`, so existing configs and `Config::default()` are unchanged in behavior.

- [ ] **Step 4: Add `kamajid` to the workspace**

Edit the root `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = [
    "crates/kamaji-core",
    "crates/kamaji",
    "crates/kamajid",
]
```

- [ ] **Step 5: Write `crates/kamajid/Cargo.toml`**

```toml
[package]
name = "kamajid"
version = "0.3.0"
edition = "2021"
publish = false

[[bin]]
name = "kamajid"
path = "src/main.rs"

[lib]
name = "kamajid"
path = "src/lib.rs"

[dependencies]
kamaji-core = { path = "../kamaji-core" }
axum = "0.7"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "sync", "signal", "time"] }
tower-http = { version = "0.6", features = ["trace"] }
tokio-stream = { version = "0.1", features = ["sync"] }
futures = "0.3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
anyhow = "1"
chrono = { version = "0.4", default-features = false, features = ["clock"] }

# Integration tests in tests/ compile as a SEPARATE crate that does NOT inherit
# this package's [dependencies] — it only sees `kamajid` plus these dev-deps.
# So every crate the tests name directly must be listed here, even where it
# duplicates a normal dependency above (Cargo unifies the versions).
[dev-dependencies]
kamaji-core = { path = "../kamaji-core" }
axum = "0.7"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "time"] }
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "stream"] }
serde_json = "1"
futures = "0.3"
tempfile = "3"
```

- [ ] **Step 6: Write `crates/kamajid/src/error.rs`**

```rust
//! HTTP error mapping. Domain/db failures become `ApiError`, which renders a
//! JSON body `{ "error": "...", "kind": "..." }` with the matching status.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// An error surfaced to an HTTP client.
pub enum ApiError {
    /// The requested entity does not exist → 404.
    NotFound,
    /// The request was malformed or violated a precondition → 400.
    BadRequest(String),
    /// An unexpected internal failure → 500 (details logged, not leaked).
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, kind, message) = match self {
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not_found", "not found".to_string()),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, "bad_request", m),
            ApiError::Internal(e) => {
                tracing::error!(error = %e, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal error".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": message, "kind": kind }))).into_response()
    }
}
```

- [ ] **Step 7: Write `crates/kamajid/src/state.rs`**

```rust
//! Shared daemon state: the SQLite handle (accessed on the blocking pool since
//! rusqlite is sync), the loaded config, and the event broadcast channel.

use std::sync::{Arc, Mutex};

use kamaji_core::config::Config;
use kamaji_core::db::Db;
use kamaji_core::events::Event;
use tokio::sync::broadcast;

use crate::error::ApiError;

/// Capacity of the per-daemon event broadcast. A slow SSE client that lags past
/// this drops events and reconnects (lossy by design — see the spec §5).
const EVENT_CHANNEL_CAPACITY: usize = 64;

#[derive(Clone)]
pub struct AppState {
    db: Arc<Mutex<Db>>,
    pub config: Arc<Config>,
    pub tx: broadcast::Sender<Event>,
}

impl AppState {
    pub fn new(db: Db, config: Config) -> Self {
        let (tx, _rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        AppState {
            db: Arc::new(Mutex::new(db)),
            config: Arc::new(config),
            tx,
        }
    }

    /// Run a DB operation on the blocking thread pool. rusqlite is synchronous,
    /// so we must not call it directly on an async worker.
    pub async fn with_db<T, F>(&self, f: F) -> Result<T, ApiError>
    where
        F: FnOnce(&Db) -> anyhow::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let db = db.lock().expect("db mutex poisoned");
            f(&db)
        })
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("db task panicked: {e}")))?
        .map_err(ApiError::Internal)
    }

    /// Broadcast an event to all SSE subscribers. Returns immediately; a send
    /// with no current subscribers is a no-op (not an error).
    pub fn emit(&self, event: Event) {
        let _ = self.tx.send(event);
    }
}
```

- [ ] **Step 8: Write `crates/kamajid/src/routes/healthz.rs` and `routes/mod.rs`**

`routes/healthz.rs`:

```rust
//! Liveness probe.

use axum::Json;
use serde_json::json;

/// `GET /healthz` → `{ "ok": true, "version": "..." }`. No deep dependency
/// checks — overkill for a localhost daemon (spec §8).
pub async fn healthz() -> Json<serde_json::Value> {
    Json(json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") }))
}
```

`routes/mod.rs`:

```rust
pub mod healthz;
```

(Other route modules are added in later tasks.)

- [ ] **Step 9: Write `crates/kamajid/src/lib.rs`**

```rust
//! The kamajid daemon: a localhost HTTP API + SSE event stream over
//! `kamaji-core`. `router` builds the axum app from an `AppState`; `serve` runs
//! it on a bound listener. The binary (`main.rs`) wires config, logging, and the
//! TCP bind around these.

pub mod error;
pub mod routes;
pub mod state;

use axum::routing::get;
use axum::Router;

use state::AppState;

/// Build the full router with all routes mounted and the shared state attached.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(routes::healthz::healthz))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

/// Serve the router on an already-bound listener until shutdown.
pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> anyhow::Result<()> {
    axum::serve(listener, router(state)).await?;
    Ok(())
}
```

- [ ] **Step 10: Write `crates/kamajid/src/main.rs`**

```rust
//! kamajid — the kamaji daemon. Parses minimal CLI args, initializes logging,
//! opens the shared SQLite DB, and serves the HTTP API on the configured bind
//! address.

use std::path::PathBuf;

use anyhow::{Context, Result};
use kamaji_core::config::{self, Config};
use kamaji_core::db::Db;
use kamaji_core::paths;
use tracing_subscriber::EnvFilter;

use kamajid::state::AppState;

fn db_path() -> Result<PathBuf> {
    Ok(paths::data_dir()
        .context("cannot determine data dir")?
        .join("kamaji.db"))
}

/// Minimal arg parse: `kamajid serve [--bind ADDR]`, plus `--help`/`--version`.
/// Other daemon settings come from the `[daemon]` config section.
struct Args {
    bind: Option<String>,
}

fn parse_args(config: &Config) -> Result<Args> {
    let mut bind = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "serve" => {}
            "--bind" => {
                bind = Some(it.next().context("--bind needs an address")?);
            }
            "--help" | "-h" => {
                println!("usage: kamajid serve [--bind ADDR]\n  default bind: {}", config.daemon.bind);
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("kamajid {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    Ok(Args { bind })
}

fn init_tracing(config: &Config) {
    let filter = EnvFilter::try_from_env("KAMAJID_LOG")
        .or_else(|_| EnvFilter::try_new(&config.daemon.log_level))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if config.daemon.log_format == "json" {
        builder.json().init();
    } else {
        builder.init();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = config::load_or_init()?;
    let args = parse_args(&config)?;
    init_tracing(&config);

    let bind = args.bind.unwrap_or_else(|| config.daemon.bind.clone());
    let db = Db::open(&db_path()?)?;
    let state = AppState::new(db, config);

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    tracing::info!(%bind, "kamajid listening");
    kamajid::serve(listener, state).await
}
```

- [ ] **Step 11: Write the `/healthz` integration test**

`crates/kamajid/tests/api.rs`:

```rust
//! Integration tests: boot the daemon on an ephemeral port with an in-memory
//! DB, drive it over HTTP with reqwest, and assert responses + SSE events.

use kamaji_core::config::Config;
use kamaji_core::db::Db;
use kamajid::state::AppState;

/// Boot a daemon on 127.0.0.1:0 with a fresh in-memory DB. Returns the base URL
/// and the `AppState` (so a test can also inspect/seed the DB or the channel).
async fn spawn() -> (String, AppState) {
    let state = AppState::new(Db::open_in_memory().unwrap(), Config::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = kamajid::router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), state)
}

#[tokio::test]
async fn healthz_reports_ok_and_version() {
    let (base, _state) = spawn().await;
    let resp = reqwest::get(format!("{base}/healthz")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
}
```

- [ ] **Step 12: Build, test, smoke**

```bash
cargo build --workspace
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E '(test result:|Running )'
```

Expected: all green. `kamaji-core` gains 1 test (248 core+tui prior totals shift: kamaji 152, kamaji-core 96), `kamajid` has 1 test. Smoke the binary:

```bash
cargo build --release
XDG_DATA_HOME=$(mktemp -d) XDG_CONFIG_HOME=$(mktemp -d) ./target/release/kamajid serve --bind 127.0.0.1:8799 &
sleep 1
curl -s http://127.0.0.1:8799/healthz
kill %1 2>/dev/null || true
```

Expected: `{"ok":true,"version":"0.3.0"}` printed. (Then the background daemon is killed.)

- [ ] **Step 13: Commit**

```bash
git add -A
git commit -m "feat(kamajid): scaffold daemon crate with /healthz

New crates/kamajid axum binary: AppState (db on the blocking pool +
event broadcast channel), router/serve, a minimal serve CLI, tracing
init, and a GET /healthz probe. Adds a backward-compatible [daemon]
config section to kamaji-core. Phase 1b step 1."
```

---

## Task 2: Error wiring + read routes (projects, tickets, config)

Add the read side of the API: list/get projects, list a project's tickets, get a ticket, and read the config. These exercise the `with_db` blocking-pool helper and the `ApiError` mapping end to end.

**Files:**
- Create: `crates/kamajid/src/routes/projects.rs`, `crates/kamajid/src/routes/config.rs`
- Modify: `crates/kamajid/src/routes/tickets.rs` (create with read handlers), `crates/kamajid/src/routes/mod.rs`, `crates/kamajid/src/lib.rs` (mount routes), `crates/kamajid/tests/api.rs`

- [ ] **Step 1: Write `crates/kamajid/src/routes/projects.rs` (read handlers)**

```rust
//! Project resource routes.

use axum::extract::{Path, State};
use axum::Json;
use kamaji_core::models::Project;

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /projects` → all projects.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<Project>>, ApiError> {
    let projects = state.with_db(|db| db.list_projects()).await?;
    Ok(Json(projects))
}

/// `GET /projects/:id` → one project, or 404.
pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Project>, ApiError> {
    let project = state
        .with_db(move |db| db.get_project(id))
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(project))
}
```

- [ ] **Step 2: Write `crates/kamajid/src/routes/tickets.rs` (read handlers)**

```rust
//! Ticket resource routes.

use axum::extract::{Path, State};
use axum::Json;
use kamaji_core::models::Ticket;

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /projects/:id/tickets` → the project's tickets, ordered.
pub async fn list_for_project(
    State(state): State<AppState>,
    Path(project_id): Path<i64>,
) -> Result<Json<Vec<Ticket>>, ApiError> {
    let tickets = state
        .with_db(move |db| db.list_tickets(project_id))
        .await?;
    Ok(Json(tickets))
}

/// `GET /tickets/:id` → one ticket, or 404.
pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Ticket>, ApiError> {
    let ticket = state
        .with_db(move |db| db.get_ticket(id))
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(ticket))
}
```

- [ ] **Step 3: Write `crates/kamajid/src/routes/config.rs`**

```rust
//! Read the daemon's loaded configuration. (Mutation is deferred to a later
//! phase; the browser does not edit config in Phase 1.)

use axum::extract::State;
use axum::Json;
use kamaji_core::config::Config;

use crate::state::AppState;

/// `GET /config` → the currently loaded config.
pub async fn get_config(State(state): State<AppState>) -> Json<Config> {
    Json((*state.config).clone())
}
```

- [ ] **Step 4: Update `routes/mod.rs`**

```rust
pub mod config;
pub mod healthz;
pub mod projects;
pub mod tickets;
```

- [ ] **Step 5: Mount the routes in `lib.rs`**

Replace the `router` function in `crates/kamajid/src/lib.rs` with:

```rust
/// Build the full router with all routes mounted and the shared state attached.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(routes::healthz::healthz))
        .route("/config", get(routes::config::get_config))
        .route("/projects", get(routes::projects::list))
        .route("/projects/:id", get(routes::projects::get_one))
        .route(
            "/projects/:id/tickets",
            get(routes::tickets::list_for_project),
        )
        .route("/tickets/:id", get(routes::tickets::get_one))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}
```

- [ ] **Step 6: Write the read-route integration tests**

Append to `crates/kamajid/tests/api.rs`:

```rust
#[tokio::test]
async fn lists_and_gets_projects_and_tickets() {
    let (base, state) = spawn().await;
    // Seed directly through the DB the daemon owns.
    let (pid, tid) = state
        .with_db(|db| {
            let p = db.create_project("acme", std::path::Path::new("/tmp/acme"), None)?;
            let t = db.create_ticket(p.id, "Add login", "desc", Some("do it"), kamaji_core::models::Agent::Claude)?;
            Ok((p.id, t.id))
        })
        .await
        .unwrap();

    let client = reqwest::Client::new();

    let projects: serde_json::Value = client
        .get(format!("{base}/projects"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(projects.as_array().unwrap().len(), 1);
    assert_eq!(projects[0]["name"], "acme");

    let project: serde_json::Value = client
        .get(format!("{base}/projects/{pid}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(project["id"], pid);

    let tickets: serde_json::Value = client
        .get(format!("{base}/projects/{pid}/tickets"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tickets.as_array().unwrap().len(), 1);
    assert_eq!(tickets[0]["title"], "Add login");
    assert_eq!(tickets[0]["agent"], "claude");
    assert_eq!(tickets[0]["status"], "todo");

    let ticket: serde_json::Value = client
        .get(format!("{base}/tickets/{tid}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ticket["id"], tid);
}

#[tokio::test]
async fn missing_ticket_is_404() {
    let (base, _state) = spawn().await;
    let resp = reqwest::get(format!("{base}/tickets/999")).await.unwrap();
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["kind"], "not_found");
}

#[tokio::test]
async fn config_is_readable() {
    let (base, _state) = spawn().await;
    let cfg: serde_json::Value = reqwest::get(format!("{base}/config"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(cfg["default_agent"], "claude");
    assert_eq!(cfg["daemon"]["bind"], "127.0.0.1:8755");
}
```

- [ ] **Step 7: Build, test**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test -p kamajid 2>&1 | grep -E 'test result:'
cargo test --all-targets --all-features 2>&1 | grep -E 'test result:'
```

Expected: `kamajid` now has 4 tests (healthz + 3 new), all green; the whole suite green.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(kamajid): error type + read routes

ApiError -> JSON {error, kind} with status mapping; the with_db
blocking-pool helper; GET /projects, /projects/:id,
/projects/:id/tickets, /tickets/:id, and /config. Integration tests
boot the daemon on an ephemeral port and assert responses + 404
shape. Phase 1b step 2."
```

---

## Task 3: Write routes (create/edit/delete tickets, create projects, move) + event emission

Add the mutating routes. Each emits the matching `Event` onto the broadcast after the DB write. (The SSE consumer that delivers them is Task 4; emitting onto a channel with no subscribers is a harmless no-op, so these can be built and response-tested now.)

**Files:**
- Modify: `crates/kamajid/src/routes/projects.rs`, `crates/kamajid/src/routes/tickets.rs`, `crates/kamajid/src/lib.rs`, `crates/kamajid/tests/api.rs`

- [ ] **Step 1: Add the project create handler to `projects.rs`**

Append to `crates/kamajid/src/routes/projects.rs`:

```rust
use axum::http::StatusCode;
use kamaji_core::models::Agent;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct CreateProject {
    pub name: String,
    pub root_dir: std::path::PathBuf,
    #[serde(default)]
    pub default_agent: Option<Agent>,
}

/// `POST /projects` → create a project. (No project event type exists in the
/// taxonomy, so nothing is broadcast.)
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateProject>,
) -> Result<(StatusCode, Json<Project>), ApiError> {
    if body.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name must not be empty".into()));
    }
    let project = state
        .with_db(move |db| db.create_project(&body.name, &body.root_dir, body.default_agent))
        .await?;
    Ok((StatusCode::CREATED, Json(project)))
}
```

- [ ] **Step 2: Add the ticket write handlers to `tickets.rs`**

Append to `crates/kamajid/src/routes/tickets.rs`:

```rust
use axum::http::StatusCode;
use kamaji_core::events::Event;
use kamaji_core::models::{Agent, Status};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct CreateTicket {
    pub project_id: i64,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub initial_prompt: Option<String>,
    pub agent: Agent,
}

/// `POST /tickets` → create a ticket in Todo. Emits `ticket.created`.
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateTicket>,
) -> Result<(StatusCode, Json<Ticket>), ApiError> {
    if body.title.trim().is_empty() {
        return Err(ApiError::BadRequest("title must not be empty".into()));
    }
    let ticket = state
        .with_db(move |db| {
            db.create_ticket(
                body.project_id,
                &body.title,
                &body.description,
                body.initial_prompt.as_deref(),
                body.agent,
            )
        })
        .await?;
    state.emit(Event::TicketCreated(ticket.clone()));
    Ok((StatusCode::CREATED, Json(ticket)))
}

#[derive(Deserialize)]
pub struct UpdateTicket {
    pub title: String,
    pub description: String,
}

/// `PATCH /tickets/:id` → edit title/description. Emits `ticket.updated`.
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateTicket>,
) -> Result<Json<Ticket>, ApiError> {
    let ticket = state
        .with_db(move |db| {
            if db.get_ticket(id)?.is_none() {
                return Ok(None);
            }
            db.update_ticket_fields(id, &body.title, &body.description)?;
            db.get_ticket(id)
        })
        .await?
        .ok_or(ApiError::NotFound)?;
    state.emit(Event::TicketUpdated(ticket.clone()));
    Ok(Json(ticket))
}

#[derive(Deserialize)]
pub struct MoveTicket {
    pub target: Status,
}

/// `POST /tickets/:id/move` → set the ticket's column. A manual move clears
/// auto-review provenance (so a human-placed card is not auto-dragged back).
/// Emits `ticket.moved` only when the column actually changes. This does NOT
/// start or stop any session — that is the `/start` route (Plan 1c).
pub async fn move_ticket(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<MoveTicket>,
) -> Result<Json<Ticket>, ApiError> {
    let target = body.target;
    let moved = state
        .with_db(move |db| {
            let Some(current) = db.get_ticket(id)? else {
                return Ok(None);
            };
            let from = current.status;
            db.set_ticket_auto_reviewed(id, false)?;
            db.set_ticket_status(id, target)?;
            let updated = db.get_ticket(id)?.expect("ticket exists; just updated");
            Ok(Some((from, updated)))
        })
        .await?;
    let (from, ticket) = moved.ok_or(ApiError::NotFound)?;
    if from != target {
        state.emit(Event::TicketMoved {
            id,
            from,
            to: target,
            at: chrono::Utc::now().to_rfc3339(),
        });
    }
    Ok(Json(ticket))
}

/// `DELETE /tickets/:id` → remove the ticket from the board. Emits
/// `ticket.deleted`. NOTE: this does not tear down any worktree/zellij session
/// the ticket may have — session cleanup is a Plan 1c concern (`/done`).
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let existed = state
        .with_db(move |db| {
            if db.get_ticket(id)?.is_none() {
                return Ok(false);
            }
            db.delete_ticket(id)?;
            Ok(true)
        })
        .await?;
    if !existed {
        return Err(ApiError::NotFound);
    }
    state.emit(Event::TicketDeleted { id });
    Ok(StatusCode::NO_CONTENT)
}
```

- [ ] **Step 3: Mount the write routes in `lib.rs`**

Update `router` in `crates/kamajid/src/lib.rs` to add `.post(...)` / `.patch(...)` / `.delete(...)` on the existing routes and the new ones. Replace the `router` body with:

```rust
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(routes::healthz::healthz))
        .route("/config", get(routes::config::get_config))
        .route(
            "/projects",
            get(routes::projects::list).post(routes::projects::create),
        )
        .route("/projects/:id", get(routes::projects::get_one))
        .route(
            "/projects/:id/tickets",
            get(routes::tickets::list_for_project),
        )
        .route("/tickets", axum::routing::post(routes::tickets::create))
        .route(
            "/tickets/:id",
            get(routes::tickets::get_one)
                .patch(routes::tickets::update)
                .delete(routes::tickets::delete),
        )
        .route(
            "/tickets/:id/move",
            axum::routing::post(routes::tickets::move_ticket),
        )
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}
```

- [ ] **Step 4: Write the write-route integration tests**

Append to `crates/kamajid/tests/api.rs`:

```rust
#[tokio::test]
async fn create_edit_move_delete_ticket_lifecycle() {
    let (base, state) = spawn().await;
    let pid = state
        .with_db(|db| Ok(db.create_project("p", std::path::Path::new("/tmp/p"), None)?.id))
        .await
        .unwrap();
    let client = reqwest::Client::new();

    // Create.
    let resp = client
        .post(format!("{base}/tickets"))
        .json(&serde_json::json!({
            "project_id": pid, "title": "Add SSO", "agent": "claude"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let created: serde_json::Value = resp.json().await.unwrap();
    let tid = created["id"].as_i64().unwrap();
    assert_eq!(created["status"], "todo");

    // Edit.
    let edited: serde_json::Value = client
        .patch(format!("{base}/tickets/{tid}"))
        .json(&serde_json::json!({ "title": "Add SAML", "description": "scope it" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(edited["title"], "Add SAML");
    assert_eq!(edited["description"], "scope it");

    // Move.
    let moved: serde_json::Value = client
        .post(format!("{base}/tickets/{tid}/move"))
        .json(&serde_json::json!({ "target": "in_progress" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(moved["status"], "in_progress");

    // Delete.
    let resp = client
        .delete(format!("{base}/tickets/{tid}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
    let resp = client.get(format!("{base}/tickets/{tid}")).send().await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn create_ticket_rejects_empty_title() {
    let (base, state) = spawn().await;
    let pid = state
        .with_db(|db| Ok(db.create_project("p", std::path::Path::new("/tmp/p"), None)?.id))
        .await
        .unwrap();
    let resp = reqwest::Client::new()
        .post(format!("{base}/tickets"))
        .json(&serde_json::json!({ "project_id": pid, "title": "  ", "agent": "claude" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["kind"], "bad_request");
}
```

- [ ] **Step 5: Build, test**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test -p kamajid 2>&1 | grep -E 'test result:'
cargo test --all-targets --all-features 2>&1 | grep -E 'test result:'
```

Expected: `kamajid` now has 6 tests; whole suite green.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(kamajid): write routes with event emission

POST /projects, POST /tickets, PATCH/DELETE /tickets/:id, and
POST /tickets/:id/move. Each ticket mutation broadcasts its Event
(created/updated/moved/deleted); a move emits ticket.moved only on
an actual column change and clears auto-review provenance. Move and
delete are DB-only here — session orchestration is Plan 1c. Phase 1b
step 3."
```

---

## Task 4: `/events` SSE stream + end-to-end command→event tests

Add the SSE endpoint that turns broadcast `Event`s into `event:`/`data:` frames, and prove the full path: a command over HTTP produces the matching SSE event on a connected client.

**Files:**
- Create: `crates/kamajid/src/routes/events.rs`
- Modify: `crates/kamajid/src/routes/mod.rs`, `crates/kamajid/src/lib.rs`, `crates/kamajid/tests/api.rs`

- [ ] **Step 1: Write `crates/kamajid/src/routes/events.rs`**

```rust
//! `GET /events` — Server-Sent Events. Subscribes to the daemon's broadcast and
//! frames each `Event` as a named SSE record: the `event:` line is the dotted
//! `sse_name()`, the `data:` line is the event payload as JSON (the inner
//! `data` of the tagged representation, without the `type` envelope).

use std::convert::Infallible;

use axum::extract::State;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::stream::{Stream, StreamExt};
use kamaji_core::events::Event;
use tokio_stream::wrappers::BroadcastStream;

use crate::state::AppState;

/// Extract the payload (the inner `data`) from a tagged `Event` for the SSE
/// `data:` line. The enum serializes as `{"type":..,"data":..}`; we send only
/// the `data` object, because the `event:` line already carries the name.
fn payload_json(event: &Event) -> String {
    let full = serde_json::to_value(event).unwrap_or(serde_json::Value::Null);
    let data = full.get("data").cloned().unwrap_or(serde_json::Value::Null);
    serde_json::to_string(&data).unwrap_or_else(|_| "null".to_string())
}

fn to_sse(event: &Event) -> SseEvent {
    SseEvent::default()
        .event(event.sse_name())
        .data(payload_json(event))
}

/// `GET /events` → an SSE stream of board deltas. Lossy by design: a client that
/// lags past the channel capacity sees its stream end (the `Lagged` item is
/// dropped) and should reconnect + re-fetch.
pub async fn events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = state.tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| async move {
        match result {
            Ok(event) => Some(Ok(to_sse(&event))),
            // Lagged: client fell behind. Drop the marker; the stream continues
            // with whatever is still buffered (client re-syncs via a re-fetch).
            Err(_) => None,
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
}
```

- [ ] **Step 2: Declare the module and mount the route**

In `crates/kamajid/src/routes/mod.rs` add `pub mod events;` (alphabetical):

```rust
pub mod config;
pub mod events;
pub mod healthz;
pub mod projects;
pub mod tickets;
```

In `crates/kamajid/src/lib.rs`, add the route inside `router` (after `/healthz`):

```rust
        .route("/events", get(routes::events::events))
```

- [ ] **Step 3: Add an SSE test helper to `tests/api.rs`**

Append a helper split into two phases so the SSE subscription is provably active **before** the test triggers a command (avoiding a subscribe-vs-emit race): `connect_events` awaits the response (the axum handler calls `tx.subscribe()` while building the response, so once the headers are received — i.e. once `.send().await` returns — the subscription is live), returning the byte stream; `read_named_event` then reads frames until the wanted event or a timeout. (`futures` and `tokio` with the `time` feature are already in `[dev-dependencies]` from Task 1.)

```rust
use futures::StreamExt;

/// Open `/events` and return the live byte stream (box-pinned so it is `Unpin`,
/// which `StreamExt::next` requires). When this returns, the server-side
/// broadcast subscription is already active, so any command emitted afterwards
/// is guaranteed to be delivered on this stream.
type ByteStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>;

async fn connect_events(base: &str) -> ByteStream {
    let resp = reqwest::Client::new()
        .get(format!("{base}/events"))
        .send()
        .await
        .unwrap();
    Box::pin(resp.bytes_stream())
}

/// Read SSE records from `stream` until one whose `event:` name equals `want`,
/// returning `(name, parsed_data_json)`. Times out after ~2s to avoid hanging CI.
async fn read_named_event<S>(stream: &mut S, want: &str) -> (String, serde_json::Value)
where
    S: futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin,
{
    let mut buf = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let chunk = tokio::time::timeout_at(deadline, stream.next())
            .await
            .expect("timed out waiting for SSE event")
            .expect("SSE stream ended")
            .expect("SSE chunk error");
        buf.push_str(&String::from_utf8_lossy(&chunk));
        // SSE records are separated by a blank line. Parse complete records.
        while let Some(idx) = buf.find("\n\n") {
            let record: String = buf.drain(..idx + 2).collect();
            let mut name = None;
            let mut data = None;
            for line in record.lines() {
                if let Some(v) = line.strip_prefix("event:") {
                    name = Some(v.trim().to_string());
                } else if let Some(v) = line.strip_prefix("data:") {
                    data = Some(v.trim().to_string());
                }
            }
            if let (Some(name), Some(data)) = (name, data) {
                if name == want {
                    return (name, serde_json::from_str(&data).unwrap());
                }
            }
        }
    }
}
```

Note: `bytes::Bytes` is reqwest's stream item type. Add `bytes = "1"` to `crates/kamajid/Cargo.toml` `[dev-dependencies]` so the test can name it:

```toml
bytes = "1"
```

- [ ] **Step 4: Write the end-to-end event tests**

Append to `crates/kamajid/tests/api.rs`:

```rust
#[tokio::test]
async fn creating_a_ticket_emits_ticket_created_on_sse() {
    let (base, state) = spawn().await;
    let pid = state
        .with_db(|db| Ok(db.create_project("p", std::path::Path::new("/tmp/p"), None)?.id))
        .await
        .unwrap();

    // Connect FIRST (subscription is live once this returns), then command, then read.
    let mut stream = connect_events(&base).await;

    reqwest::Client::new()
        .post(format!("{base}/tickets"))
        .json(&serde_json::json!({ "project_id": pid, "title": "Streamed", "agent": "claude" }))
        .send()
        .await
        .unwrap();

    let (name, data) = read_named_event(&mut stream, "ticket.created").await;
    assert_eq!(name, "ticket.created");
    assert_eq!(data["title"], "Streamed");
    assert_eq!(data["status"], "todo");
}

#[tokio::test]
async fn moving_a_ticket_emits_ticket_moved_on_sse() {
    let (base, state) = spawn().await;
    let tid = state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
            Ok(t.id)
        })
        .await
        .unwrap();

    let mut stream = connect_events(&base).await;

    reqwest::Client::new()
        .post(format!("{base}/tickets/{tid}/move"))
        .json(&serde_json::json!({ "target": "in_progress" }))
        .send()
        .await
        .unwrap();

    let (name, data) = read_named_event(&mut stream, "ticket.moved").await;
    assert_eq!(name, "ticket.moved");
    assert_eq!(data["id"], tid);
    assert_eq!(data["from"], "todo");
    assert_eq!(data["to"], "in_progress");
}
```

- [ ] **Step 5: Build, test**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test -p kamajid 2>&1 | grep -E 'test result:'
cargo test --all-targets --all-features 2>&1 | grep -E 'test result:'
```

Expected: `kamajid` now has 8 tests; whole suite green. (If an SSE test is flaky on a slow machine, the 100ms subscribe delay + 2s deadline give margin; do NOT weaken assertions to paper over a real ordering bug.)

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(kamajid): /events SSE stream

GET /events frames broadcast Events as event:/data: records (dotted
sse_name + the payload JSON). Lossy broadcast: a lagging client's
stream ends and it re-syncs. End-to-end tests prove a POST command
arrives as the matching SSE event on a connected client. Phase 1b
step 4."
```

---

## Task 5: Ship

- [ ] **Step 1: Final full verification**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E '(test result:|Running )'
```

Expected: all green — kamaji 152, kamaji-core 96, kamajid 8.

- [ ] **Step 2: Manual smoke (the daemon serves the board)**

```bash
cargo build --release
DATA=$(mktemp -d); CFG=$(mktemp -d)
XDG_DATA_HOME=$DATA XDG_CONFIG_HOME=$CFG ./target/release/kamajid serve --bind 127.0.0.1:8799 &
sleep 1
echo "--- healthz ---"; curl -s http://127.0.0.1:8799/healthz
echo; echo "--- create project ---"
curl -s -X POST http://127.0.0.1:8799/projects -H 'content-type: application/json' \
  -d '{"name":"demo","root_dir":"/tmp/demo"}'
echo; echo "--- list projects ---"; curl -s http://127.0.0.1:8799/projects
kill %1 2>/dev/null || true
```

Expected: healthz ok; a created project echoed back; the list contains it.

- [ ] **Step 3: Push and open the PR**

```bash
git push -u origin "$(git branch --show-current)"
gh pr create --fill --base main
```

- [ ] **Step 4: Auto-merge with branch delete**

```bash
gh pr merge --squash --auto --delete-branch
```

Per the known worktree gotcha, the post-merge local cleanup may error from inside the worktree; the merge still lands. Wait for CI to go green (CI is not a required check, so gate manually to avoid landing red), then verify:

```bash
gh pr view --json state -q .state
```

Once `MERGED`, clean up from the primary worktree at `/home/victor/dev/kamaji`:

```bash
cd /home/victor/dev/kamaji
git checkout main && git pull --ff-only
git worktree remove ../kamaji-worktrees/<branch>
git branch -d <branch>
git push origin --delete <branch> 2>/dev/null || true
git fetch --prune origin
```

---

## Self-review checklist

- **Spec coverage (Phase 1 spec §4 routes):** `/healthz` (T1), `/events` (T4), `GET/POST /projects` + `GET /projects/:id` + `GET /projects/:id/tickets` (T2/T3), `POST /tickets` + `GET/PATCH/DELETE /tickets/:id` + `POST /tickets/:id/move` (T2/T3), `GET /config` (T2). **Deferred to Plan 1c (documented):** `POST /tickets/:id/start`, `POST /tickets/:id/attach`, `POST /tickets/:id/done`, `PATCH /config`, and the poll background task + `zellij web` management.
- **Type consistency:** `AppState` API (`new`, `with_db`, `emit`, `config`, `tx`) used identically across all route modules and tests. `Event` variants/`sse_name`/payload framing consistent. Route handler signatures follow axum 0.7 (`State` first, body/`Json` last; `:id` path params).
- **No placeholders:** every code step is complete.
- **Event taxonomy honored:** create→`ticket.created`, edit→`ticket.updated`, move→`ticket.moved` (only on real change), delete→`ticket.deleted`. No double-emit. Session events (`session.started/idle/exited`) come with the orchestration/poll work in Plan 1c.

## What this plan deliberately does NOT do (→ Plan 1c)

- `POST /tickets/:id/start` (worktree + zellij session creation), `POST /tickets/:id/done` with cleanup, `POST /tickets/:id/attach` (zellij web). These need git/zellij and so are grouped where their testing strategy (temp git repos, `#[ignore]`d zellij e2e) lives.
- The `PollLoop` background task (auto-review in the daemon → `session.idle`/`ticket.moved`) and the `reconcile` extraction (→ `session.exited`).
- `zellij web` lifecycle management + the auth token.
- `PATCH /config` (config mutation; no Phase 1 UI needs it yet).
- Daemon auto-spawn and the TUI-as-client flip (those are Phase 2).
