# Phase 2: TUI as a kamajid Client — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Flip the ratatui TUI (`crates/kamaji`) from driving `kamaji-core` directly to being a thin HTTP+SSE client of `kamajid`, auto-spawning the daemon when needed, while keeping native zellij attach client-side.

**Architecture:** `kamaji` ensures a detached `kamajid` is healthy (pidfile lock + `/healthz` probe), then constructs a blocking `reqwest` `DaemonClient` for every board read/write and a background SSE thread that pushes decoded `kamaji_core::events::Event`s into an `mpsc` channel the sync event loop drains each frame. The daemon is the single DB writer and the only place git/zellij orchestration and the auto-review poll loop run; the TUI re-renders from `/events` deltas and re-fetches the project on any reconnect. Three tiny daemon additions land first (pid/addr files on bind, `POST /projects/:id/main-session`, `PATCH /config`) because the client depends on them.

**Tech Stack:** Rust 2021, `reqwest` blocking client (TUI), axum 0.7 + tokio (daemon), `serde` DTOs already living in `kamaji-core`, `thiserror`-style `ApiError` framing, SSE `event:`/`data:` framing from `routes/events.rs`.

---

## File Structure

**`kamaji-core` (shared):**
- `crates/kamaji-core/src/events.rs` — **MODIFY**: add `events::from_sse(name, data) -> Option<Event>`, the inverse of `Event::sse_name()` + the daemon's `payload_json` framing. Round-trip unit-tested.
- `crates/kamaji-core/src/paths.rs` — **MODIFY**: add `runtime_dir()` (`$XDG_RUNTIME_DIR` else `cache_dir()`), and `kamaji-core/Cargo.toml` keeps existing deps.

**`kamajid` (daemon — three §11 additions, sequenced first):**
- `crates/kamajid/src/main.rs` — **MODIFY**: after a successful bind, write `kamajid.pid` (PID) + `kamajid.addr` (bound addr) under `runtime_dir()`; remove both on clean shutdown.
- `crates/kamajid/src/routes/projects.rs` — **MODIFY**: add `POST /projects/:id/main-session` handler → `{ session_name }`, wrapping `session::prepare_main_session` + `zellij::create_session_background`, idempotent when the session is already live.
- `crates/kamajid/src/routes/config.rs` — **MODIFY**: add `PATCH /config` handler that persists config edits (theme / default_agent / worktree_base) through the single writer.
- `crates/kamajid/src/lib.rs` — **MODIFY**: mount the two new routes.
- `crates/kamajid/src/state.rs` — **MODIFY**: hold the loaded config behind a writable handle so `PATCH /config` can hot-swap it (see Task 2-pre-C).

**`kamaji` (TUI client):**
- `crates/kamaji/src/client.rs` — **NEW**: `DaemonClient` blocking reqwest wrapper + `ClientError`.
- `crates/kamaji/src/daemon.rs` — **NEW**: `ensure_daemon()` — pidfile lock, detached spawn, health wait, addr discovery.
- `crates/kamaji/src/sse.rs` — **NEW**: background SSE listener thread → `mpsc::Sender<SseMsg>`.
- `crates/kamaji/src/main.rs` — **MODIFY**: `ensure_daemon` before TUI; pass `DaemonClient` down; start SSE thread; drain SSE + draw; collapse `Effect` match; no `detect_tick`.
- `crates/kamaji/src/engine.rs` — **SHRINK**: `Engine = { client, app, project, … ui-only }`; handlers call the client; delete DB/zellij/poll/reconcile.
- `crates/kamaji/src/picker.rs` — **MODIFY**: project list/create via `DaemonClient` (no `&Db`).
- `crates/kamaji/src/cli.rs` — **MODIFY**: `ticket create` dispatches via `ensure_daemon` + `DaemonClient`.
- `crates/kamaji/Cargo.toml` — **MODIFY**: add `reqwest = { version = "0.12", default-features = false, features = ["json", "blocking", "rustls-tls"] }`.
- `crates/kamaji/src/{app.rs,ui/*,theme.rs,update.rs,dir_select.rs}` — **UNCHANGED**.

---

## Step 2-pre — Daemon additions (client depends on these)

### Task 2-pre-A: `paths::runtime_dir()` in core

**Files:**
- Modify: `crates/kamaji-core/src/paths.rs`

- [ ] **Write failing test.** Append to the `tests` mod in `crates/kamaji-core/src/paths.rs` (inside `#[cfg(all(test, not(windows)))]`):
  ```rust
  #[test]
  fn runtime_dir_prefers_xdg_runtime_dir() {
      let got = resolve_runtime(
          Some(OsStr::new("/run/user/1000")),
          Some(PathBuf::from("/home/u/.cache/kamaji")),
      );
      assert_eq!(got, Some(PathBuf::from("/run/user/1000/kamaji")));
  }

  #[test]
  fn runtime_dir_falls_back_to_cache_when_no_xdg_runtime() {
      let got = resolve_runtime(None, Some(PathBuf::from("/home/u/.cache/kamaji")));
      assert_eq!(got, Some(PathBuf::from("/home/u/.cache/kamaji")));
  }

  #[test]
  fn runtime_dir_relative_xdg_runtime_is_ignored() {
      let got = resolve_runtime(
          Some(OsStr::new("rel/run")),
          Some(PathBuf::from("/home/u/.cache/kamaji")),
      );
      assert_eq!(got, Some(PathBuf::from("/home/u/.cache/kamaji")));
  }
  ```
- [ ] **Run it (expect FAIL — `resolve_runtime` undefined):**
  `cargo test -p kamaji-core paths::tests::runtime_dir` → expect a compile error (function not found).
- [ ] **Minimal implementation.** Add to `crates/kamaji-core/src/paths.rs` (after `cache_dir`):
  ```rust
  /// `<runtime>/kamaji` for ephemeral runtime files (pidfile, addr). Uses
  /// `$XDG_RUNTIME_DIR` when set to an absolute path, otherwise falls back to
  /// `cache_dir()`. The `kamaji` leaf is appended to an `$XDG_RUNTIME_DIR` base;
  /// the cache fallback already carries it.
  #[cfg(not(windows))]
  pub fn runtime_dir() -> Option<PathBuf> {
      resolve_runtime(std::env::var_os("XDG_RUNTIME_DIR").as_deref(), cache_dir())
  }

  #[cfg(windows)]
  pub fn runtime_dir() -> Option<PathBuf> {
      cache_dir()
  }

  #[cfg(not(windows))]
  fn resolve_runtime(
      xdg_runtime: Option<&std::ffi::OsStr>,
      cache: Option<PathBuf>,
  ) -> Option<PathBuf> {
      xdg_runtime
          .map(PathBuf::from)
          .filter(|p| p.is_absolute())
          .map(|p| p.join(APP))
          .or(cache)
  }
  ```
- [ ] **Run it (expect PASS):** `cargo test -p kamaji-core paths::tests::runtime_dir` → 3 passed.
- [ ] **Commit:** `git commit -am "feat(core): add paths::runtime_dir for daemon pid/addr files"`

### Task 2-pre-B: daemon writes `kamajid.pid` + `kamajid.addr` on bind

**Files:**
- Modify: `crates/kamajid/src/main.rs`

- [ ] **Add a runtime-files helper + writes.** In `crates/kamajid/src/main.rs`, after the `db_path` fn add:
  ```rust
  fn runtime_paths() -> Result<(PathBuf, PathBuf)> {
      let dir = paths::runtime_dir().context("cannot determine runtime dir")?;
      std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
      Ok((dir.join("kamajid.pid"), dir.join("kamajid.addr")))
  }
  ```
  In `main`, after the successful `TcpListener::bind`, capture the real bound address and write both files (overwrite any stale ones — `ensure_daemon` on the client side guarantees we only reach here as the lock winner or a manual `serve`):
  ```rust
  let local = listener.local_addr().with_context(|| "reading bound address")?;
  let (pidfile, addrfile) = runtime_paths()?;
  std::fs::write(&pidfile, std::process::id().to_string())
      .with_context(|| format!("writing {}", pidfile.display()))?;
  std::fs::write(&addrfile, local.to_string())
      .with_context(|| format!("writing {}", addrfile.display()))?;
  tracing::info!(%local, pid = std::process::id(), "wrote pid/addr files");

  let cleanup = (pidfile.clone(), addrfile.clone());
  let result = kamajid::serve(listener, state).await;
  let _ = std::fs::remove_file(&cleanup.0);
  let _ = std::fs::remove_file(&cleanup.1);
  result
  ```
  Replace the final `kamajid::serve(listener, state).await` line with the block above. Add `use std::path::PathBuf;` if not present (it is) and ensure `paths` is imported (it is).
- [ ] **Write an integration test (gated, real spawn).** Create `crates/kamajid/tests/runtime_files.rs`:
  ```rust
  //! Verifies the daemon writes pid/addr files on bind and removes them on exit.
  //! Spawns the built `kamajid` binary detached on an ephemeral port. Gated
  //! `#[ignore]` like the other live tests: run with `--ignored`.

  #[test]
  #[ignore = "spawns the kamajid binary; run manually with --ignored"]
  fn writes_pid_and_addr_files_on_bind() {
      use std::time::{Duration, Instant};
      let tmp = tempfile::tempdir().unwrap();
      let bin = env!("CARGO_BIN_EXE_kamajid");
      let mut child = std::process::Command::new(bin)
          .args(["serve", "--bind", "127.0.0.1:0"])
          .env("XDG_RUNTIME_DIR", tmp.path())
          .env("XDG_DATA_HOME", tmp.path())
          .env("XDG_CONFIG_HOME", tmp.path())
          .spawn()
          .unwrap();
      let pidfile = tmp.path().join("kamaji").join("kamajid.pid");
      let addrfile = tmp.path().join("kamaji").join("kamajid.addr");
      let deadline = Instant::now() + Duration::from_secs(5);
      while Instant::now() < deadline && !(pidfile.exists() && addrfile.exists()) {
          std::thread::sleep(Duration::from_millis(50));
      }
      assert!(pidfile.exists(), "pidfile should be written on bind");
      assert!(addrfile.exists(), "addrfile should be written on bind");
      let addr = std::fs::read_to_string(&addrfile).unwrap();
      assert!(addr.starts_with("127.0.0.1:"), "addr was {addr:?}");
      child.kill().unwrap();
      child.wait().unwrap();
  }
  ```
  Add `tempfile = "3"` to `crates/kamajid/Cargo.toml` `[dev-dependencies]` (it is not present yet — verify and add).
- [ ] **Run it (expect PASS):** `cargo test -p kamajid --test runtime_files -- --ignored` → 1 passed. Also confirm the default build is green: `cargo test -p kamajid` (the ignored test is skipped).
- [ ] **Commit:** `git commit -am "feat(kamajid): write pid/addr files on bind, remove on shutdown"`

### Task 2-pre-C: `PATCH /config`

**Files:**
- Modify: `crates/kamajid/src/state.rs`, `crates/kamajid/src/routes/config.rs`, `crates/kamajid/src/lib.rs`

- [ ] **Make config writable in state.** In `crates/kamajid/src/state.rs`, change `pub config: Arc<Config>` to an `RwLock` so `PATCH` can replace it while readers (poll task, routes) still clone:
  ```rust
  use tokio::sync::RwLock as TokioRwLock; // add to imports
  ```
  Replace the field and constructor:
  ```rust
  pub config: Arc<TokioRwLock<Config>>,
  ```
  In `new`: `config: Arc::new(TokioRwLock::new(config)),`. Add a sync snapshot accessor used by the poll task and existing `(*state.config).clone()` callsites. Since `spawn_poll_task` reads `state.config.poll_interval()` synchronously and routes do `(*state.config).clone()`, add:
  ```rust
  /// A cloned snapshot of the current config. Cheap; taken per request/round so
  /// a PATCH is observed on the next read.
  pub fn config_snapshot(&self) -> Config {
      self.config.blocking_read().clone()
  }
  ```
  Then update the three existing read sites: in `main.rs` use `state.config_snapshot().poll_interval()`; in `routes/tickets.rs::start` replace `(*state.config).clone()` with `state.config_snapshot()`; in `routes/config.rs::get_config` and `poll_task.rs`'s `task_state.config` usages replace with `config_snapshot()` / `task_state.config_snapshot()`. (Grep `state.config` and `.config` to find each — there are 3 callsites total: `main.rs`, `routes/tickets.rs`, `poll_task.rs`, plus `routes/config.rs`.)

  > Note for the implementer: `blocking_read()` must run on a blocking thread. The poll task already runs `config` access inside `spawn_blocking`; `start` runs at the top of an async handler — wrap that one `config_snapshot()` in the existing `spawn_blocking` it already uses, or switch it to `self.config.read().await`. Simpler: add **both** `pub async fn config_async(&self) -> Config { self.config.read().await.clone() }` and use the async form in async route bodies, the `blocking_read` form only inside `spawn_blocking` closures. Use the async form in `routes/config.rs` and `routes/tickets.rs::start`; use `config_snapshot()` (blocking) only inside `poll_task`'s `spawn_blocking`.

- [ ] **Write a failing test.** Add to `crates/kamajid/tests/api.rs`:
  ```rust
  #[tokio::test]
  async fn patch_config_persists_theme_and_agent() {
      let (base, _state) = spawn().await;
      let resp = reqwest::Client::new()
          .patch(format!("{base}/config"))
          .json(&serde_json::json!({ "theme": "nord", "default_agent": "codex" }))
          .send()
          .await
          .unwrap();
      assert_eq!(resp.status(), 200);
      let cfg: serde_json::Value = resp.json().await.unwrap();
      assert_eq!(cfg["theme"], "nord");
      assert_eq!(cfg["default_agent"], "codex");
      // A subsequent GET reflects the change (single writer updated in place).
      let cfg2: serde_json::Value = reqwest::get(format!("{base}/config")).await.unwrap().json().await.unwrap();
      assert_eq!(cfg2["theme"], "nord");
  }
  ```
- [ ] **Run it (expect FAIL — 405 Method Not Allowed):** `cargo test -p kamajid --test api patch_config_persists_theme_and_agent` → fails (route not mounted).
- [ ] **Implement the handler.** In `crates/kamajid/src/routes/config.rs`:
  ```rust
  use axum::extract::State;
  use axum::Json;
  use kamaji_core::config::Config;
  use kamaji_core::models::Agent;
  use serde::Deserialize;

  use crate::error::ApiError;
  use crate::state::AppState;

  pub async fn get_config(State(state): State<AppState>) -> Json<Config> {
      Json(state.config_async().await)
  }

  /// Partial config edit: any present field replaces its current value; omitted
  /// fields are kept. Only the TUI-editable fields are accepted (theme,
  /// default_agent, worktree_base). Persisted to config.toml + held in memory.
  #[derive(Deserialize)]
  pub struct PatchConfig {
      #[serde(default)]
      pub theme: Option<String>,
      #[serde(default)]
      pub default_agent: Option<String>,
      #[serde(default)]
      pub worktree_base: Option<String>,
  }

  pub async fn patch_config(
      State(state): State<AppState>,
      Json(body): Json<PatchConfig>,
  ) -> Result<Json<Config>, ApiError> {
      if let Some(ref a) = body.default_agent {
          a.parse::<Agent>()
              .map_err(|e| ApiError::BadRequest(format!("invalid default_agent: {e}")))?;
      }
      let mut guard = state.config.write().await;
      if let Some(t) = body.theme { guard.theme = t; }
      if let Some(a) = body.default_agent { guard.default_agent = a; }
      if let Some(w) = body.worktree_base { guard.worktree_base = Some(w); }
      let updated = guard.clone();
      drop(guard);
      let path = kamaji_core::config::config_path()
          .map_err(ApiError::Internal)?;
      let to_save = updated.clone();
      tokio::task::spawn_blocking(move || kamaji_core::config::save_to(&path, &to_save))
          .await
          .map_err(|e| ApiError::Internal(anyhow::anyhow!("config save task panicked: {e}")))?
          .map_err(ApiError::Internal)?;
      Ok(Json(updated))
  }
  ```
  Expose `pub config` write access: `state.config` is the `Arc<TokioRwLock<Config>>` field — make the field `pub` (it already is) so the handler can `.write()`. Mount in `lib.rs`: change the `/config` route to `.route("/config", get(routes::config::get_config).patch(routes::config::patch_config))`.
- [ ] **Run it (expect PASS):** `cargo test -p kamajid --test api patch_config_persists_theme_and_agent` → 1 passed. Then `cargo test -p kamajid` (full daemon suite) green.
- [ ] **Commit:** `git commit -am "feat(kamajid): add PATCH /config single-writer config edits"`

### Task 2-pre-D: `POST /projects/:id/main-session`

**Files:**
- Modify: `crates/kamajid/src/routes/projects.rs`, `crates/kamajid/src/lib.rs`

- [ ] **Write a failing test.** Add to `crates/kamajid/tests/api.rs` (gated on zellij, mirroring existing live-session tests in that file):
  ```rust
  #[tokio::test]
  async fn main_session_returns_404_for_missing_project() {
      let (base, _state) = spawn().await;
      let resp = reqwest::Client::new()
          .post(format!("{base}/projects/999/main-session"))
          .send()
          .await
          .unwrap();
      assert_eq!(resp.status(), 404);
  }

  #[tokio::test]
  #[ignore = "spawns a real zellij session; run with --ignored"]
  async fn main_session_creates_and_is_idempotent() {
      use crate::support::{committed_repo, zellij_available};
      if !zellij_available() { return; }
      let (base, state) = spawn().await;
      let repo = committed_repo();
      let pid = state
          .with_db({
              let root = repo.path().to_path_buf();
              move |db| Ok(db.create_project("p", &root, None)?.id)
          })
          .await
          .unwrap();
      let resp = reqwest::Client::new()
          .post(format!("{base}/projects/{pid}/main-session"))
          .send()
          .await
          .unwrap();
      assert_eq!(resp.status(), 200);
      let body: serde_json::Value = resp.json().await.unwrap();
      assert_eq!(body["session_name"], format!("kamaji-main-{pid}"));
      kamaji_core::zellij::terminate_session(&format!("kamaji-main-{pid}"));
  }
  ```
- [ ] **Run it (expect FAIL):** `cargo test -p kamajid --test api main_session_returns_404_for_missing_project` → fails (route not mounted).
- [ ] **Implement the handler.** In `crates/kamajid/src/routes/projects.rs` add:
  ```rust
  use serde::Serialize;
  use kamaji_core::{session, slug, zellij};

  #[derive(Serialize)]
  pub struct MainSession {
      pub session_name: String,
  }

  /// `POST /projects/:id/main-session` → start (or reuse) the project's main
  /// workspace session — not tied to any ticket — and return its name. Idempotent:
  /// if zellij already lists the session, no new one is spawned. 404 if the
  /// project is missing; 500 if layout prep or the zellij spawn fails.
  pub async fn main_session(
      State(state): State<AppState>,
      Path(id): Path<i64>,
  ) -> Result<Json<MainSession>, ApiError> {
      let project = state
          .with_db(move |db| db.get_project(id))
          .await?
          .ok_or(ApiError::NotFound)?;
      let config = state.config_async().await;
      let name = slug::main_session_name(project.id);
      let already_live = tokio::task::spawn_blocking({
          let name = name.clone();
          move || zellij::list_sessions().map(|l| zellij::session_in_list(&l, &name)).unwrap_or(false)
      })
      .await
      .map_err(|e| ApiError::Internal(anyhow::anyhow!("list-sessions task panicked: {e}")))?;
      if already_live {
          return Ok(Json(MainSession { session_name: name }));
      }
      let prepared = tokio::task::spawn_blocking(move || {
          session::prepare_main_session(&project, &config)
      })
      .await
      .map_err(|e| ApiError::Internal(anyhow::anyhow!("prepare task panicked: {e}")))?
      .map_err(|e| ApiError::Internal(anyhow::anyhow!("prepare main session failed: {e}")))?;
      let cwd = std::env::temp_dir(); // create_session_background ignores cwd for a shell layout that sets its own cwd
      let layout = prepared.layout_path.clone();
      let name2 = prepared.name.clone();
      tokio::task::spawn_blocking(move || {
          zellij::create_session_background(&name2, &layout, &cwd)
      })
      .await
      .map_err(|e| ApiError::Internal(anyhow::anyhow!("spawn task panicked: {e}")))?
      .map_err(|e| ApiError::Internal(anyhow::anyhow!("starting main session failed: {e}")))?;
      Ok(Json(MainSession { session_name: prepared.name }))
  }
  ```
  > Implementer note: confirm `zellij::create_session_background(name, layout_path, cwd)` — the `cwd` arg is the working dir for the spawn command, while the layout KDL already pins the pane `cwd` to the project root (see `render_shell_layout`). Passing `temp_dir()` is harmless; if `create_session_background` requires an existing dir, `temp_dir()` always exists.

  Mount in `lib.rs`:
  ```rust
  .route(
      "/projects/:id/main-session",
      axum::routing::post(routes::projects::main_session),
  )
  ```
- [ ] **Run it (expect PASS):** `cargo test -p kamajid --test api main_session_returns_404_for_missing_project` → passed; `cargo test -p kamajid --test api main_session_creates_and_is_idempotent -- --ignored` → passed when zellij present. Full `cargo test -p kamajid` green.
- [ ] **Commit:** `git commit -am "feat(kamajid): add POST /projects/:id/main-session"`

---

## Step 2a — Client + auto-spawn scaffolding (daemon optional, board unchanged)

Adds `client.rs`, `daemon.rs`, `sse.rs`; `main.rs` ensures a daemon and starts the SSE thread (logging events only). The TUI still drives `Engine`-on-core. Zero board behavior change.

### Task 2a-1: add `reqwest` blocking dep + `client.rs` skeleton with `connect` + `ClientError`

**Files:**
- Modify: `crates/kamaji/Cargo.toml`
- Create: `crates/kamaji/src/client.rs`
- Modify: `crates/kamaji/src/main.rs` (add `mod client;`)

- [ ] **Add the dependency.** In `crates/kamaji/Cargo.toml` `[dependencies]` add:
  ```toml
  reqwest = { version = "0.12", default-features = false, features = ["json", "blocking", "rustls-tls"] }
  ```
- [ ] **Write a failing test (in-crate unit test against a real daemon).** Create `crates/kamaji/src/client.rs` with the type, `connect`, and a test module that boots a real `kamajid` via a tiny tokio runtime. Add to `crates/kamaji/Cargo.toml` `[dev-dependencies]`:
  ```toml
  kamajid = { path = "../kamajid" }
  tokio = { version = "1", features = ["rt-multi-thread", "macros", "net"] }
  ```
  First write only the failing test + minimal struct:
  ```rust
  //! Blocking HTTP client over the kamajid REST API. The TUI loop is sync, so
  //! commands are `reqwest::blocking` round-trips to localhost (sub-ms).

  use kamaji_core::config::Config;
  use kamaji_core::models::{Agent, Project, Status, Ticket};

  #[derive(Debug)]
  pub enum ClientError {
      NotFound,
      BadRequest(String),
      Server(String),
      Unreachable(reqwest::Error),
      Decode(String),
  }

  pub type Result<T> = std::result::Result<T, ClientError>;

  pub struct DaemonClient {
      http: reqwest::blocking::Client,
      base: String,
      version: String,
  }

  impl DaemonClient {
      /// Build a client for `base` (e.g. "http://127.0.0.1:8755") and ping
      /// `/healthz` to confirm liveness and capture the daemon version.
      pub fn connect(base: String) -> Result<Self> {
          let http = reqwest::blocking::Client::builder()
              .timeout(std::time::Duration::from_secs(5))
              .build()
              .map_err(ClientError::Unreachable)?;
          let resp = http
              .get(format!("{base}/healthz"))
              .send()
              .map_err(ClientError::Unreachable)?;
          let body: serde_json::Value = resp.json().map_err(|e| ClientError::Decode(e.to_string()))?;
          let version = body
              .get("version")
              .and_then(|v| v.as_str())
              .unwrap_or_default()
              .to_string();
          Ok(DaemonClient { http, base, version })
      }

      pub fn base(&self) -> &str { &self.base }
      pub fn version(&self) -> &str { &self.version }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      /// Boot a real kamajid on 127.0.0.1:0, returning its base URL. The runtime
      /// is leaked for the test's lifetime so the server keeps serving.
      fn spawn_daemon() -> String {
          use kamajid::state::AppState;
          let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
          let (tx, rx) = std::sync::mpsc::channel();
          std::thread::spawn(move || {
              rt.block_on(async move {
                  let state = AppState::new(kamaji_core::db::Db::open_in_memory().unwrap(), Config::default());
                  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                  let addr = listener.local_addr().unwrap();
                  tx.send(format!("http://{addr}")).unwrap();
                  axum_serve(listener, state).await;
              });
          });
          rx.recv().unwrap()
      }

      async fn axum_serve(listener: tokio::net::TcpListener, state: kamajid::state::AppState) {
          let app = kamajid::router(state);
          axum::serve(listener, app).await.unwrap();
      }

      #[test]
      fn connect_pings_healthz_and_captures_version() {
          let base = spawn_daemon();
          let client = DaemonClient::connect(base.clone()).unwrap();
          assert_eq!(client.base(), base);
          assert_eq!(client.version(), env!("CARGO_PKG_VERSION"));
      }
  }
  ```
  Add `mod client;` to `crates/kamaji/src/main.rs`.
  > Implementer note: `axum` must be a dev-dependency of `kamaji` for the test's `axum::serve`. Add `axum = "0.7"` to `[dev-dependencies]`. The `kamajid` lib re-exports `router`/`state`/`serve`; prefer `kamajid::serve(listener, state).await` over the inline `axum_serve` if it compiles cleanly (it returns `anyhow::Result`; `.unwrap()` it).
- [ ] **Run it (expect FAIL first as a compile/red, then green after deps resolve):** `cargo test -p kamaji client::tests::connect_pings_healthz_and_captures_version` → expect PASS once deps compile. (If the very first run is a compile error because deps aren't fetched, that is the "red"; the same command after the code above is the "green".)
- [ ] **Commit:** `git commit -am "feat(tui): add DaemonClient::connect + ClientError scaffold"`

### Task 2a-2: client read methods

**Files:**
- Modify: `crates/kamaji/src/client.rs`

- [ ] **Write failing tests.** Add to the `tests` mod:
  ```rust
  fn seed_project_and_ticket(base: &str) -> (i64, i64) {
      // Use the client itself once create methods exist; for now seed via HTTP.
      let http = reqwest::blocking::Client::new();
      let p: serde_json::Value = http.post(format!("{base}/projects"))
          .json(&serde_json::json!({ "name": "acme", "root_dir": "/tmp/acme" }))
          .send().unwrap().json().unwrap();
      let pid = p["id"].as_i64().unwrap();
      let t: serde_json::Value = http.post(format!("{base}/tickets"))
          .json(&serde_json::json!({ "project_id": pid, "title": "Add login", "agent": "claude" }))
          .send().unwrap().json().unwrap();
      (pid, t["id"].as_i64().unwrap())
  }

  #[test]
  fn read_methods_round_trip() {
      let base = spawn_daemon();
      let client = DaemonClient::connect(base.clone()).unwrap();
      let (pid, tid) = seed_project_and_ticket(&base);
      assert_eq!(client.list_projects().unwrap().len(), 1);
      assert_eq!(client.get_project(pid).unwrap().name, "acme");
      let tickets = client.list_tickets(pid).unwrap();
      assert_eq!(tickets.len(), 1);
      assert_eq!(tickets[0].title, "Add login");
      assert_eq!(client.get_ticket(tid).unwrap().status, Status::Todo);
      assert_eq!(client.get_config().unwrap().default_agent, "claude");
  }

  #[test]
  fn get_ticket_missing_maps_not_found() {
      let base = spawn_daemon();
      let client = DaemonClient::connect(base).unwrap();
      assert!(matches!(client.get_ticket(999), Err(ClientError::NotFound)));
  }
  ```
- [ ] **Run them (expect FAIL — methods undefined):** `cargo test -p kamaji client::tests::read_methods_round_trip` → compile error.
- [ ] **Implement reads + a shared response helper.** Add to `impl DaemonClient`:
  ```rust
  /// Map a finished response into a deserialized `T` or a `ClientError`. 2xx →
  /// decode body; 404 → NotFound; 400 → BadRequest(reason); else Server.
  fn parse<T: serde::de::DeserializeOwned>(resp: reqwest::blocking::Response) -> Result<T> {
      let status = resp.status();
      if status.is_success() {
          return resp.json().map_err(|e| ClientError::Decode(e.to_string()));
      }
      let body: serde_json::Value = resp.json().unwrap_or(serde_json::Value::Null);
      let reason = body.get("error").and_then(|v| v.as_str()).unwrap_or("").to_string();
      match status.as_u16() {
          404 => Err(ClientError::NotFound),
          400 => Err(ClientError::BadRequest(reason)),
          _ => Err(ClientError::Server(reason)),
      }
  }

  fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
      let resp = self.http.get(format!("{}{path}", self.base)).send().map_err(ClientError::Unreachable)?;
      Self::parse(resp)
  }

  pub fn list_projects(&self) -> Result<Vec<Project>> { self.get_json("/projects") }
  pub fn get_project(&self, id: i64) -> Result<Project> { self.get_json(&format!("/projects/{id}")) }
  pub fn list_tickets(&self, project_id: i64) -> Result<Vec<Ticket>> {
      self.get_json(&format!("/projects/{project_id}/tickets"))
  }
  pub fn get_ticket(&self, id: i64) -> Result<Ticket> { self.get_json(&format!("/tickets/{id}")) }
  pub fn get_config(&self) -> Result<Config> { self.get_json("/config") }
  ```
- [ ] **Run them (expect PASS):** `cargo test -p kamaji client::tests` → all passed.
- [ ] **Commit:** `git commit -am "feat(tui): DaemonClient read methods (projects/tickets/config)"`

### Task 2a-3: client command methods

**Files:**
- Modify: `crates/kamaji/src/client.rs`

- [ ] **Write failing tests.** Add:
  ```rust
  #[test]
  fn create_project_and_ticket_via_client() {
      let base = spawn_daemon();
      let client = DaemonClient::connect(base).unwrap();
      let p = client.create_project("acme", std::path::Path::new("/tmp/acme"), None).unwrap();
      let t = client.create_ticket(p.id, "Add login", "desc", Some("go"), Agent::Claude).unwrap();
      assert_eq!(t.title, "Add login");
      let edited = client.update_ticket(t.id, "Renamed", Some("d2"), None, None).unwrap();
      assert_eq!(edited.title, "Renamed");
      let moved = client.move_ticket(t.id, Status::Review).unwrap();
      assert_eq!(moved.status, Status::Review);
      let done = client.done_ticket(t.id, false).unwrap();
      assert_eq!(done.status, Status::Done);
      client.delete_ticket(t.id).unwrap();
      assert!(matches!(client.get_ticket(t.id), Err(ClientError::NotFound)));
  }

  #[test]
  fn create_ticket_empty_title_is_bad_request() {
      let base = spawn_daemon();
      let client = DaemonClient::connect(base).unwrap();
      let p = client.create_project("p", std::path::Path::new("/tmp/p"), None).unwrap();
      let err = client.create_ticket(p.id, "  ", "", None, Agent::Claude).unwrap_err();
      assert!(matches!(err, ClientError::BadRequest(_)));
  }

  #[test]
  fn update_config_via_client() {
      let base = spawn_daemon();
      let client = DaemonClient::connect(base).unwrap();
      let cfg = client.update_config(Some("nord"), Some("codex"), None).unwrap();
      assert_eq!(cfg.theme, "nord");
      assert_eq!(cfg.default_agent, "codex");
  }
  ```
  > Note: `update_config` writes the real `config.toml` (the daemon's `config_path()`), so this test mutates the user's config dir unless `XDG_CONFIG_HOME` is overridden. Set `XDG_CONFIG_HOME` to a tempdir at the top of `update_config_via_client` via `std::env::set_var` before `spawn_daemon`, OR mark this test `#[ignore]` and add a cheaper assertion that `update_config` issues a PATCH (the daemon test in 2-pre-C already covers persistence). Prefer the tempdir override.
- [ ] **Run them (expect FAIL):** `cargo test -p kamaji client::tests::create_project_and_ticket_via_client` → compile error.
- [ ] **Implement commands.** Add to `impl DaemonClient`:
  ```rust
  fn send_json<B: serde::Serialize, T: serde::de::DeserializeOwned>(
      &self, method: reqwest::Method, path: &str, body: &B,
  ) -> Result<T> {
      let resp = self.http.request(method, format!("{}{path}", self.base))
          .json(body).send().map_err(ClientError::Unreachable)?;
      Self::parse(resp)
  }

  pub fn create_project(&self, name: &str, root_dir: &std::path::Path, default_agent: Option<Agent>) -> Result<Project> {
      self.send_json(reqwest::Method::POST, "/projects",
          &serde_json::json!({ "name": name, "root_dir": root_dir, "default_agent": default_agent }))
  }
  pub fn create_ticket(&self, project_id: i64, title: &str, description: &str, prompt: Option<&str>, agent: Agent) -> Result<Ticket> {
      self.send_json(reqwest::Method::POST, "/tickets",
          &serde_json::json!({ "project_id": project_id, "title": title, "description": description, "initial_prompt": prompt, "agent": agent }))
  }
  pub fn update_ticket(&self, id: i64, title: &str, description: Option<&str>, prompt: Option<&str>, agent: Option<Agent>) -> Result<Ticket> {
      self.send_json(reqwest::Method::PATCH, &format!("/tickets/{id}"),
          &serde_json::json!({ "title": title, "description": description, "initial_prompt": prompt, "agent": agent }))
  }
  pub fn move_ticket(&self, id: i64, target: Status) -> Result<Ticket> {
      self.send_json(reqwest::Method::POST, &format!("/tickets/{id}/move"), &serde_json::json!({ "target": target }))
  }
  pub fn start_ticket(&self, id: i64) -> Result<Ticket> {
      let resp = self.http.post(format!("{}/tickets/{id}/start", self.base)).send().map_err(ClientError::Unreachable)?;
      Self::parse(resp)
  }
  pub fn done_ticket(&self, id: i64, cleanup: bool) -> Result<Ticket> {
      self.send_json(reqwest::Method::POST, &format!("/tickets/{id}/done"), &serde_json::json!({ "cleanup": cleanup }))
  }
  pub fn delete_ticket(&self, id: i64) -> Result<()> {
      let resp = self.http.delete(format!("{}/tickets/{id}", self.base)).send().map_err(ClientError::Unreachable)?;
      let status = resp.status();
      if status.is_success() { return Ok(()); }
      match status.as_u16() { 404 => Err(ClientError::NotFound), _ => Err(ClientError::Server(String::new())) }
  }
  pub fn main_session(&self, project_id: i64) -> Result<String> {
      let resp = self.http.post(format!("{}/projects/{project_id}/main-session", self.base)).send().map_err(ClientError::Unreachable)?;
      let v: serde_json::Value = Self::parse(resp)?;
      v.get("session_name").and_then(|s| s.as_str()).map(str::to_string)
          .ok_or_else(|| ClientError::Decode("missing session_name".into()))
  }
  pub fn update_config(&self, theme: Option<&str>, default_agent: Option<&str>, worktree_base: Option<&str>) -> Result<Config> {
      self.send_json(reqwest::Method::PATCH, "/config",
          &serde_json::json!({ "theme": theme, "default_agent": default_agent, "worktree_base": worktree_base }))
  }
  ```
  > Note: `start_ticket` returns the updated `Ticket` (per `routes/tickets.rs::start` which returns `Json<Ticket>`). The spec table labels it `SessionInfo`, but the actual route returns the ticket — trust the code: it returns `Ticket` with `session_name` populated.
- [ ] **Run them (expect PASS):** `cargo test -p kamaji client::tests` → all passed.
- [ ] **Commit:** `git commit -am "feat(tui): DaemonClient command methods (create/move/start/done/delete/config/main-session)"`

### Task 2a-4: `events::from_sse` round-trip helper in core

**Files:**
- Modify: `crates/kamaji-core/src/events.rs`

- [ ] **Write a failing round-trip test.** Add to the `tests` mod in `events.rs`. First add a local helper mirroring the daemon's `payload_json` (so the test exercises the exact daemon framing):
  ```rust
  /// Mirror of kamajid's routes/events.rs::payload_json — the SSE `data:` payload.
  fn payload_json(event: &Event) -> String {
      let full = serde_json::to_value(event).unwrap();
      let data = full.get("data").cloned().unwrap_or(serde_json::Value::Null);
      serde_json::to_string(&data).unwrap()
  }

  fn sample_events() -> Vec<Event> {
      let t = crate::models::Ticket {
          id: 1, project_id: 1, title: "t".into(), description: String::new(),
          initial_prompt: None, agent: crate::models::Agent::Claude, status: Status::Todo,
          position: 0, session_name: None, worktree_path: None, branch: None,
          auto_reviewed: false, instrumented: false,
          created_at: String::new(), updated_at: String::new(),
      };
      vec![
          Event::TicketCreated(t.clone()),
          Event::TicketUpdated(t),
          Event::TicketMoved { id: 5, from: Status::InProgress, to: Status::Review, at: "2026-05-30T10:23:45Z".into() },
          Event::TicketDeleted { id: 7 },
          Event::SessionStarted { ticket_id: 3, session_name: "kamaji-3-x".into() },
          Event::SessionIdle { ticket_id: 3 },
          Event::SessionExited { ticket_id: 3, session_name: "kamaji-3-x".into() },
      ]
  }

  #[test]
  fn from_sse_round_trips_daemon_framing_for_every_variant() {
      for ev in sample_events() {
          let name = ev.sse_name();
          let data = payload_json(&ev);
          let back = Event::from_sse(name, &data).expect("from_sse should decode the daemon frame");
          assert_eq!(back.sse_name(), name, "variant changed for {name}");
          // The payload must also round-trip identically.
          assert_eq!(payload_json(&back), data, "payload differs for {name}");
      }
  }

  #[test]
  fn from_sse_rejects_unknown_event_name() {
      assert!(Event::from_sse("nope.unknown", "{}").is_none());
  }
  ```
- [ ] **Run it (expect FAIL):** `cargo test -p kamaji-core events::tests::from_sse_round_trips` → compile error (`from_sse` undefined).
- [ ] **Implement `from_sse`.** Add to `impl Event` in `events.rs`:
  ```rust
  /// Reconstruct an `Event` from the daemon's SSE framing: the dotted `event:`
  /// name plus the bare `data:` payload (the inner `data` of the tagged enum,
  /// with no `type` envelope). The inverse of [`Self::sse_name`] + the daemon's
  /// `payload_json`. Returns `None` for an unknown name or a payload that does
  /// not match the named variant.
  pub fn from_sse(name: &str, data: &str) -> Option<Event> {
      let inner: serde_json::Value = serde_json::from_str(data).ok()?;
      // Rebuild the tagged `{ "type": <snake>, "data": <inner> }` shape and
      // deserialize through the canonical enum so framing stays defined once.
      let tag = match name {
          "ticket.created" => "ticket_created",
          "ticket.updated" => "ticket_updated",
          "ticket.moved" => "ticket_moved",
          "ticket.deleted" => "ticket_deleted",
          "session.started" => "session_started",
          "session.idle" => "session_idle",
          "session.exited" => "session_exited",
          _ => return None,
      };
      let tagged = serde_json::json!({ "type": tag, "data": inner });
      serde_json::from_value(tagged).ok()
  }
  ```
  > Implementer note: `TicketCreated(Ticket)` / `TicketUpdated(Ticket)` are newtype variants — under `#[serde(tag="type", content="data")]` their `data` is the flat ticket object, which is exactly what `payload_json` emits, so the rebuilt `{type,data}` deserializes correctly. The round-trip test proves this for all seven variants.
- [ ] **Run it (expect PASS):** `cargo test -p kamaji-core events::tests` → all passed.
- [ ] **Commit:** `git commit -am "feat(core): add events::from_sse (inverse of sse_name) with round-trip test"`

### Task 2a-5: `sse.rs` listener thread

**Files:**
- Create: `crates/kamaji/src/sse.rs`
- Modify: `crates/kamaji/src/main.rs` (add `mod sse;`)

- [ ] **Write failing tests.** Create `crates/kamaji/src/sse.rs`. First a pure-parser test that needs no network:
  ```rust
  //! Background SSE listener: streams `GET /events`, decodes each record into a
  //! `kamaji_core::events::Event` via `Event::from_sse`, and forwards `SseMsg`s
  //! over an mpsc channel the sync UI loop drains. Reconnects with backoff.

  use std::sync::mpsc::Sender;
  use std::thread::JoinHandle;

  use kamaji_core::events::Event;

  pub enum SseMsg {
      Event(Event),
      Connected,
      Disconnected,
  }

  /// Pull complete SSE records out of an accumulating buffer. Returns the decoded
  /// events and leaves any partial trailing record in `buf`. A record is the text
  /// between blank-line separators; we read its `event:` and `data:` lines.
  pub(crate) fn drain_records(buf: &mut String) -> Vec<Event> {
      let mut out = Vec::new();
      while let Some(idx) = buf.find("\n\n") {
          let record: String = buf.drain(..idx + 2).collect();
          let mut name = None;
          let mut data = None;
          for line in record.lines() {
              if let Some(v) = line.strip_prefix("event:") { name = Some(v.trim().to_string()); }
              else if let Some(v) = line.strip_prefix("data:") { data = Some(v.trim().to_string()); }
          }
          if let (Some(name), Some(data)) = (name, data) {
              if let Some(ev) = Event::from_sse(&name, &data) { out.push(ev); }
          }
      }
      out
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn drain_parses_complete_records_and_keeps_partial() {
          let mut buf = String::from(
              "event: ticket.deleted\ndata: {\"id\":7}\n\nevent: session.idle\ndata: {\"ticket_id\":3}\n\nevent: ticket.del",
          );
          let events = drain_records(&mut buf);
          assert_eq!(events.len(), 2);
          assert_eq!(events[0].sse_name(), "ticket.deleted");
          assert_eq!(events[1].sse_name(), "session.idle");
          // The incomplete trailing record stays buffered for the next chunk.
          assert!(buf.starts_with("event: ticket.del"));
      }

      #[test]
      fn drain_ignores_keepalive_comments() {
          // axum keep-alive sends `:` comment lines; they carry no event:/data:.
          let mut buf = String::from(": keep-alive\n\n");
          assert!(drain_records(&mut buf).is_empty());
      }
  }
  ```
- [ ] **Run them (expect FAIL then PASS):** `cargo test -p kamaji sse::tests` → compile then pass.
- [ ] **Implement `spawn`.** Add to `sse.rs`:
  ```rust
  use std::io::Read;
  use std::time::Duration;

  /// Spawn the SSE listener thread. It connects to `<base>/events`, emits
  /// `Connected` (→ UI re-fetch), streams `Event`s, and on stream end/error emits
  /// `Disconnected` and retries with capped backoff (250ms → 2s). Ends when the
  /// receiver is dropped (send fails).
  pub fn spawn(base: String, tx: Sender<SseMsg>) -> JoinHandle<()> {
      std::thread::spawn(move || {
          let http = reqwest::blocking::Client::builder()
              .timeout(None) // a streaming response must not time out mid-stream
              .build()
              .expect("build sse client");
          let mut backoff = Duration::from_millis(250);
          loop {
              match http.get(format!("{base}/events")).send() {
                  Ok(mut resp) if resp.status().is_success() => {
                      backoff = Duration::from_millis(250);
                      if tx.send(SseMsg::Connected).is_err() { return; }
                      let mut buf = String::new();
                      let mut chunk = [0u8; 4096];
                      loop {
                          match resp.read(&mut chunk) {
                              Ok(0) => break,            // stream ended
                              Ok(n) => {
                                  buf.push_str(&String::from_utf8_lossy(&chunk[..n]));
                                  for ev in drain_records(&mut buf) {
                                      if tx.send(SseMsg::Event(ev)).is_err() { return; }
                                  }
                              }
                              Err(_) => break,
                          }
                      }
                      if tx.send(SseMsg::Disconnected).is_err() { return; }
                  }
                  _ => {
                      if tx.send(SseMsg::Disconnected).is_err() { return; }
                  }
              }
              std::thread::sleep(backoff);
              backoff = (backoff * 2).min(Duration::from_secs(2));
          }
      })
  }
  ```
  Add `mod sse;` to `main.rs`.
- [ ] **Write a gated live listener test.** Add to `sse.rs` `tests` (reuse the `spawn_daemon` pattern; gate with `#[ignore]` since it boots a daemon + relies on timing):
  ```rust
  #[test]
  #[ignore = "boots a daemon and exercises live streaming; run with --ignored"]
  fn live_listener_reports_connected_then_event() {
      // Mirror client.rs::tests::spawn_daemon to get a base URL, then:
      // 1. spawn(base, tx); 2. expect SseMsg::Connected; 3. POST a ticket via
      //    reqwest::blocking; 4. expect SseMsg::Event(ticket.created) within 2s.
      // (Implementer: copy the spawn_daemon helper or move it to a shared test mod.)
  }
  ```
  > The unit `drain_records` tests are the cheap seam; the live test is `#[ignore]`d per Phase 1 convention.
- [ ] **Run it:** `cargo test -p kamaji sse::tests` → unit tests pass; `cargo test -p kamaji sse -- --ignored` exercises the live path.
- [ ] **Commit:** `git commit -am "feat(tui): SSE listener thread with record parser + reconnect backoff"`

### Task 2a-6: `daemon.rs` — pure pidfile/liveness logic

**Files:**
- Create: `crates/kamaji/src/daemon.rs`
- Modify: `crates/kamaji/src/main.rs` (add `mod daemon;`)

- [ ] **Write failing tests for the pure pieces.** Create `crates/kamaji/src/daemon.rs`:
  ```rust
  //! Daemon auto-spawn: ensure a healthy kamajid (pidfile lock + health probe),
  //! spawning one detached if absent; race-safe via atomic pidfile create.

  use std::path::{Path, PathBuf};

  use crate::client::DaemonClient;
  use kamaji_core::config::Config;
  use kamaji_core::paths;

  /// Paths to the pidfile + addrfile under the runtime dir.
  pub fn runtime_files() -> Option<(PathBuf, PathBuf)> {
      let dir = paths::runtime_dir()?;
      Some((dir.join("kamajid.pid"), dir.join("kamajid.addr")))
  }

  /// True if `pid` names a live process. Unix: `kill(pid, 0)` semantics via
  /// checking `/proc` is avoided; we use a 0-signal. Windows: best-effort true
  /// (we rely on the health probe to catch a dead daemon).
  #[cfg(unix)]
  pub fn pid_alive(pid: i32) -> bool {
      // signal 0 only checks existence/permission, never delivers a signal.
      unsafe { libc::kill(pid, 0) == 0 }
  }
  #[cfg(not(unix))]
  pub fn pid_alive(_pid: i32) -> bool { true }

  /// Parse the PID written in the pidfile, if any.
  pub fn read_pid(pidfile: &Path) -> Option<i32> {
      std::fs::read_to_string(pidfile).ok()?.trim().parse().ok()
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn read_pid_parses_written_value() {
          let dir = tempfile::tempdir().unwrap();
          let f = dir.path().join("kamajid.pid");
          std::fs::write(&f, "4321\n").unwrap();
          assert_eq!(read_pid(&f), Some(4321));
      }

      #[test]
      fn read_pid_none_when_absent_or_garbage() {
          let dir = tempfile::tempdir().unwrap();
          let f = dir.path().join("kamajid.pid");
          assert_eq!(read_pid(&f), None);
          std::fs::write(&f, "not-a-pid").unwrap();
          assert_eq!(read_pid(&f), None);
      }

      #[cfg(unix)]
      #[test]
      fn pid_alive_true_for_self_false_for_unused() {
          assert!(pid_alive(std::process::id() as i32));
          // PID 0x7fffffff is astronomically unlikely to be live.
          assert!(!pid_alive(0x7fff_ffff));
      }
  }
  ```
  Add `libc = "0.2"` to `crates/kamaji/Cargo.toml` under `[target.'cfg(unix)'.dependencies]`. Add `mod daemon;` to `main.rs`.
- [ ] **Run them (expect PASS):** `cargo test -p kamaji daemon::tests` → all passed.
- [ ] **Commit:** `git commit -am "feat(tui): daemon pidfile read + liveness primitives"`

### Task 2a-7: `daemon.rs` — `ensure_daemon` (lock, spawn, health-wait, race)

**Files:**
- Modify: `crates/kamaji/src/daemon.rs`

- [ ] **Write failing tests.** Add (these use a custom runtime dir via env override of `XDG_RUNTIME_DIR` to a tempdir, and a "no-spawn" mode so we don't actually fork in unit tests):
  ```rust
  #[test]
  fn stale_pidfile_is_reclaimed() {
      // A pidfile naming a dead PID + no live daemon => probe_existing returns
      // None and the stale files are removed.
      let dir = tempfile::tempdir().unwrap();
      let pidfile = dir.path().join("kamajid.pid");
      let addrfile = dir.path().join("kamajid.addr");
      std::fs::write(&pidfile, "2147483647").unwrap(); // dead PID
      std::fs::write(&addrfile, "127.0.0.1:8755").unwrap();
      let got = probe_existing(&pidfile, &addrfile);
      assert!(got.is_none(), "a stale pidfile must not yield a client");
      assert!(!pidfile.exists(), "stale pidfile is removed");
      assert!(!addrfile.exists(), "stale addrfile is removed");
  }

  #[test]
  fn acquire_lock_is_exclusive() {
      let dir = tempfile::tempdir().unwrap();
      let pidfile = dir.path().join("kamajid.pid");
      assert!(acquire_lock(&pidfile).is_ok(), "first writer wins the lock");
      assert!(acquire_lock(&pidfile).is_err(), "second writer loses (AlreadyExists)");
  }

  #[test]
  fn health_wait_times_out_on_dead_port() {
      // Nothing listens on this port; bounded wait returns an error, not a hang.
      let started = std::time::Instant::now();
      let res = wait_for_health("http://127.0.0.1:1", std::time::Duration::from_millis(300));
      assert!(res.is_err());
      assert!(started.elapsed() < std::time::Duration::from_secs(2));
  }
  ```
- [ ] **Run them (expect FAIL):** `cargo test -p kamaji daemon::tests::acquire_lock_is_exclusive` → compile error.
- [ ] **Implement the pieces + `ensure_daemon`.** Add:
  ```rust
  use std::fs::OpenOptions;
  use std::io::Write;
  use std::time::{Duration, Instant};

  /// If a live daemon is described by the pidfile+addrfile, connect and return it.
  /// "Live" = the named PID exists AND `/healthz` answers. On any failure the
  /// stale files are removed and `None` is returned so the caller lock-acquires.
  pub fn probe_existing(pidfile: &Path, addrfile: &Path) -> Option<DaemonClient> {
      let pid = read_pid(pidfile)?;
      let addr = std::fs::read_to_string(addrfile).ok()?.trim().to_string();
      if pid_alive(pid) {
          if let Ok(client) = DaemonClient::connect(format!("http://{addr}")) {
              return Some(client);
          }
      }
      let _ = std::fs::remove_file(pidfile);
      let _ = std::fs::remove_file(addrfile);
      None
  }

  /// Atomically create the pidfile as a lock (O_CREAT|O_EXCL). Exactly one racer
  /// wins; losers get an `AlreadyExists` error.
  pub fn acquire_lock(pidfile: &Path) -> std::io::Result<()> {
      if let Some(parent) = pidfile.parent() { let _ = std::fs::create_dir_all(parent); }
      let mut f = OpenOptions::new().write(true).create_new(true).open(pidfile)?;
      // Placeholder; the daemon overwrites this with its real PID on bind.
      write!(f, "{}", std::process::id())
  }

  /// Poll `<base>/healthz` every ~50ms until 200 or the deadline. Bounded.
  pub fn wait_for_health(base: &str, timeout: Duration) -> std::result::Result<DaemonClient, String> {
      let deadline = Instant::now() + timeout;
      let http = reqwest::blocking::Client::builder()
          .timeout(Duration::from_millis(200)).build().map_err(|e| e.to_string())?;
      while Instant::now() < deadline {
          if http.get(format!("{base}/healthz")).send().map(|r| r.status().is_success()).unwrap_or(false) {
              return DaemonClient::connect(base.to_string()).map_err(|e| format!("{e:?}"));
          }
          std::thread::sleep(Duration::from_millis(50));
      }
      Err(format!("daemon did not become healthy at {base} within {timeout:?}"))
  }

  /// Locate the kamajid binary: a sibling next to the running kamaji, else PATH.
  fn kamajid_path() -> std::result::Result<PathBuf, String> {
      if let Ok(exe) = std::env::current_exe() {
          if let Some(dir) = exe.parent() {
              let sibling = dir.join(if cfg!(windows) { "kamajid.exe" } else { "kamajid" });
              if sibling.exists() { return Ok(sibling); }
          }
      }
      Ok(PathBuf::from("kamajid")) // fall back to PATH resolution
  }

  /// Spawn `kamajid serve --bind <addr>` detached so it outlives the TUI.
  #[cfg(unix)]
  fn spawn_detached(bin: &Path, addr: &str) -> std::io::Result<()> {
      use std::os::unix::process::CommandExt;
      use std::process::{Command, Stdio};
      let mut cmd = Command::new(bin);
      cmd.args(["serve", "--bind", addr])
          .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
      // New session so it isn't killed when the terminal closes.
      unsafe { cmd.pre_exec(|| { libc::setsid(); Ok(()) }); }
      cmd.spawn()?;
      Ok(())
  }
  #[cfg(not(unix))]
  fn spawn_detached(bin: &Path, addr: &str) -> std::io::Result<()> {
      use std::os::windows::process::CommandExt;
      use std::process::{Command, Stdio};
      const DETACHED_PROCESS: u32 = 0x0000_0008;
      const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
      Command::new(bin)
          .args(["serve", "--bind", addr])
          .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
          .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
          .spawn()?;
      Ok(())
  }

  /// Ensure a healthy daemon and return a connected client. Tries an existing
  /// daemon; else lock-acquires (winner spawns + health-waits + writes addr;
  /// loser health-waits on the expected addr). Bounded retry on a lost race whose
  /// winner crashed. `forced_addr` (from `--daemon`) skips spawning entirely.
  pub fn ensure_daemon(config: &Config, forced_addr: Option<&str>, allow_spawn: bool) -> std::result::Result<DaemonClient, String> {
      if let Some(addr) = forced_addr {
          let base = if addr.starts_with("http") { addr.to_string() } else { format!("http://{addr}") };
          return DaemonClient::connect(base).map_err(|e| format!("--daemon {addr}: {e:?}"));
      }
      let (pidfile, addrfile) = runtime_files().ok_or("cannot determine runtime dir")?;
      let bind = config.daemon.bind.clone();
      let base = format!("http://{bind}");
      for _attempt in 0..2 {
          if let Some(client) = probe_existing(&pidfile, &addrfile) { return Ok(client); }
          match acquire_lock(&pidfile) {
              Ok(()) => {
                  if !allow_spawn {
                      let _ = std::fs::remove_file(&pidfile);
                      return Err("no daemon running and --no-spawn was given".into());
                  }
                  let bin = kamajid_path()?;
                  spawn_detached(&bin, &bind).map_err(|e| format!("spawning kamajid ({}): {e}", bin.display()))?;
                  // The daemon writes its own pid/addr on bind; we just wait for health.
                  return wait_for_health(&base, Duration::from_secs(5));
              }
              Err(_already_exists) => {
                  // Someone else is starting it: wait for the winner's health.
                  if let Ok(client) = wait_for_health(&base, Duration::from_secs(5)) {
                      return Ok(client);
                  }
                  // Winner may have crashed between lock and bind: clear + retry once.
                  let _ = std::fs::remove_file(&pidfile);
                  let _ = std::fs::remove_file(&addrfile);
              }
          }
      }
      Err(format!("could not reach or start a daemon at {bind}"))
  }
  ```
  > Implementer note: the lock pidfile written by `acquire_lock` is the TUI's own PID as a placeholder; the spawned daemon **overwrites** both pid+addr on bind (Task 2-pre-B). `probe_existing` therefore reads the daemon's real PID/addr once it's healthy. The `--bind 127.0.0.1:8755` uses the configured fixed port (not `:0`) so losers know where to health-wait — matching spec §7.3.
- [ ] **Run them (expect PASS):** `cargo test -p kamaji daemon::tests` → all passed.
- [ ] **Add a gated end-to-end spawn test.** Add `#[ignore]`:
  ```rust
  #[cfg(unix)]
  #[test]
  #[ignore = "actually spawns the built kamajid detached; run with --ignored"]
  fn ensure_daemon_spawns_and_connects() {
      // Override XDG_RUNTIME_DIR + XDG_DATA_HOME + XDG_CONFIG_HOME to a tempdir,
      // build Config::default(), call ensure_daemon(&cfg, None, true), assert it
      // returns Ok and /healthz is green; then kill via the pidfile's PID.
  }
  ```
- [ ] **Run gated:** `cargo test -p kamaji daemon -- --ignored` (manual).
- [ ] **Commit:** `git commit -am "feat(tui): ensure_daemon — pidfile lock, detached spawn, health-wait, race-safe"`

### Task 2a-8: wire `ensure_daemon` + SSE thread into `main.rs` (board still core-driven)

**Files:**
- Modify: `crates/kamaji/src/main.rs`
- Modify: `crates/kamaji/src/cli.rs` (parse `--daemon`/`--no-spawn`)

- [ ] **Parse the two escape-hatch flags.** In `crates/kamaji/src/cli.rs`, extend `Command::Tui` to carry daemon options. Add a struct and thread it through `parse`:
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq, Default)]
  pub struct DaemonOpts {
      pub forced_addr: Option<String>, // --daemon <ADDR>: use it, never spawn
      pub no_spawn: bool,              // --no-spawn: fail if none up
  }
  ```
  Change `Command::Tui` to `Tui(DaemonOpts)`. In `parse`, before the `ticket` match, scan leading args for `--daemon <addr>` and `--no-spawn`; if present (and no `ticket` subcommand follows) return `Command::Tui(opts)`. Keep `parse([])` → `Command::Tui(DaemonOpts::default())`. Update the existing `no_args_runs_tui` test to `Command::Tui(DaemonOpts::default())` and add:
  ```rust
  #[test]
  fn parses_daemon_and_no_spawn_flags() {
      assert_eq!(parse(["--no-spawn"]).unwrap(), Command::Tui(DaemonOpts { forced_addr: None, no_spawn: true }));
      assert_eq!(parse(["--daemon", "127.0.0.1:9000"]).unwrap(),
          Command::Tui(DaemonOpts { forced_addr: Some("127.0.0.1:9000".into()), no_spawn: false }));
  }
  ```
  Run: `cargo test -p kamaji cli::tests::parses_daemon_and_no_spawn_flags` (FAIL → implement → PASS).
- [ ] **Wire into `run_tui`.** In `main.rs`, change the `cli::Command::Tui` arm to `cli::Command::Tui(opts) => run_tui(opts)`. In `run_tui`, before `ratatui::init()`:
  ```rust
  fn run_tui(opts: cli::DaemonOpts) -> Result<()> {
      let config = config::load_or_init()?;
      let client = daemon::ensure_daemon(&config, opts.forced_addr.as_deref(), !opts.no_spawn)
          .map_err(|e| anyhow::anyhow!("could not start kamaji: {e}"))?;

      // SSE listener thread (2a: events are logged only; applied in 2b).
      let (sse_tx, sse_rx) = std::sync::mpsc::channel::<sse::SseMsg>();
      let _sse_handle = sse::spawn(client.base().to_string(), sse_tx);

      // Existing update-check thread, unchanged.
      let update_status: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
      /* ...unchanged update::check spawn... */

      // 2a: the board still drives Engine-on-core; keep db + config.
      let db = Db::open(&db_path()?)?;
      let mut terminal = ratatui::init();
      let result = run(&mut terminal, db, config, update_status, sse_rx);
      ratatui::restore();
      result
  }
  ```
  Thread `sse_rx` into `run`/`run_board` and, for 2a only, drain it logging each msg (no UI change). Add to the top of `run_board`'s loop:
  ```rust
  while let Ok(msg) = sse_rx.try_recv() {
      match msg {
          sse::SseMsg::Connected => tracing::debug!("sse connected"),
          sse::SseMsg::Disconnected => tracing::debug!("sse disconnected"),
          sse::SseMsg::Event(ev) => tracing::debug!(event = ev.sse_name(), "sse event"),
      }
  }
  ```
  > Note: `kamaji` does not currently init `tracing`. For 2a, instead of `tracing::debug!`, drain-and-discard (the goal is "events arrive without breaking the loop"); verification is via the daemon being healthy and the board working. Keep the drain so the channel never fills.
- [ ] **Verify (manual smoke — no automated assertion).** `cargo run -p kamaji` with no daemon running spawns one (`<runtime>/kamaji/kamajid.pid` appears, `curl 127.0.0.1:8755/healthz` is green); a second `kamaji` reuses it (no second pidfile contention); the board still creates/moves/attaches exactly as before (core-driven). `cargo test --workspace` stays green.
- [ ] **Commit:** `git commit -am "feat(tui): ensure daemon + start SSE thread on startup (board still core-driven)"`

---

## Step 2b — Reads come from the daemon; SSE applied live

Picker + board seeding switch to the client; SSE deltas mutate the in-memory list. `Engine` still writes via core temporarily (same DB file, so daemon reads reflect core writes).

### Task 2b-1: picker reads/creates via `DaemonClient`

**Files:**
- Modify: `crates/kamaji/src/picker.rs`
- Modify: `crates/kamaji/src/main.rs` (call site)

- [ ] **Change `picker::run` to take `&DaemonClient` instead of `&Db`.** Replace `db.list_projects()?` with `client.list_projects().map_err(...)?` and the two `db.create_project(...)` calls with `client.create_project(name, &root, None).map_err(...)?`. Map `ClientError` to `anyhow::Error` via a small helper `fn client_err(e: ClientError) -> anyhow::Error`. Update the signature:
  ```rust
  pub fn run(terminal: &mut DefaultTerminal, client: &DaemonClient, theme: Theme) -> Result<Option<Project>>
  ```
  Remove `use kamaji_core::db::Db;`. The render tests (`picker_renders_as_centered_modal`, form-unit tests) are unaffected (they don't touch `db`).
- [ ] **Run existing picker tests (expect PASS):** `cargo test -p kamaji picker::tests` → all passed (no DB-touching tests there).
- [ ] **Update the call site in `main.rs`.** In `run`, replace `picker::run(terminal, &db, theme)?` with `picker::run(terminal, &client, theme)?` and thread `client` into `run`. (Engine still owns `db` for writes this step.)
- [ ] **Commit:** `git commit -am "feat(tui): picker lists/creates projects via DaemonClient"`

### Task 2b-2: board seeds tickets + config from the daemon

**Files:**
- Modify: `crates/kamaji/src/main.rs`, `crates/kamaji/src/engine.rs`

- [ ] **Seed the board from the client.** In `main.rs::run`, replace `let tickets = db.list_tickets(project.id)?;` with `let tickets = client.list_tickets(project.id).map_err(client_err)?;`. Replace the `theme` source: fetch config from the daemon once at startup (`let config = client.get_config().map_err(client_err)?;`) so theme/agent reflect the daemon's loaded config. Keep `Engine::new(db, config, app)` for now (Engine still needs `db` for writes).
- [ ] **Add `Engine::refresh_from_client` reading the client.** In `engine.rs`, add a method that replaces `reload`'s data source for the *list* (writes still go through core in 2b):
  ```rust
  /// Re-fetch the current project's tickets from the daemon and re-clamp the UI.
  /// Used after SSE deltas and after attach. The daemon is the read source of
  /// truth; local DB reads are being retired.
  pub fn refresh_from_client(&mut self) -> anyhow::Result<()> {
      match self.client.list_tickets(self.app.project.id) {
          Ok(tickets) => { self.app.tickets = tickets; }
          Err(e) => self.app.set_error(format!("could not refresh board: {e:?}")),
      }
      self.poll.rehydrate(&self.app.tickets);
      self.app.reclamp();
      self.app.prune_selection();
      Ok(())
  }
  ```
  This requires `Engine` to hold a `client`. Add `pub client: DaemonClient` to the struct and a constructor param. **This is the step where `Engine::new` gains the client.** Update `Engine::new(db, config, app)` → `Engine::new(client, db, config, app)` and every test constructor (`engine_with_project` builds a throwaway client — see note). Since unit tests can't trivially boot a daemon per `Engine`, add a test-only constructor that takes a pre-spawned base, OR gate the engine tests' migration to Task 2c where handlers change. **Decision for 2b:** keep `Engine` core-driven for writes and only *add* the `client` field + `refresh_from_client`; in `engine_with_project`, spawn a shared test daemon once (a `OnceLock<String>` holding a base URL) and `DaemonClient::connect` it. Provide:
  ```rust
  #[cfg(test)]
  fn test_client() -> DaemonClient {
      use std::sync::OnceLock;
      static BASE: OnceLock<String> = OnceLock::new();
      let base = BASE.get_or_init(spawn_test_daemon).clone();
      DaemonClient::connect(base).unwrap()
  }
  ```
  where `spawn_test_daemon` mirrors `client.rs::tests::spawn_daemon`. Move that helper into a shared `#[cfg(test)] mod test_support;` so both `client.rs` and `engine.rs` use it.
- [ ] **Run tests (expect PASS):** `cargo test -p kamaji engine::tests` → all passed (writes still core-driven; the client field is unused by existing assertions). This confirms the refactor compiles with the new field.
- [ ] **Commit:** `git commit -am "feat(tui): seed board tickets+config from daemon; Engine gains DaemonClient"`

### Task 2b-3: apply SSE deltas to the board

**Files:**
- Modify: `crates/kamaji/src/engine.rs`, `crates/kamaji/src/main.rs`

- [ ] **Write failing tests for the applier.** In `engine.rs` `tests`, add (these drive `apply_sse_event` directly — pure UI mutation, no daemon round-trip needed for created/moved/deleted because the payload carries everything; `session.*` and id-only events re-fetch, so test those against the test daemon or assert the toast/structure for the ones that don't re-fetch):
  ```rust
  use kamaji_core::events::Event as CoreEvent;

  #[test]
  fn sse_ticket_created_for_current_project_inserts() {
      let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
      let pid = e.app.project.id;
      let t = sample_ticket(pid, 1, "New", Status::Todo);
      e.apply_sse_event(CoreEvent::TicketCreated(t));
      assert_eq!(e.app.tickets.len(), 1);
      assert_eq!(e.app.tickets[0].title, "New");
  }

  #[test]
  fn sse_ticket_created_for_other_project_is_ignored() {
      let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
      let other = sample_ticket(e.app.project.id + 999, 1, "Elsewhere", Status::Todo);
      e.apply_sse_event(CoreEvent::TicketCreated(other));
      assert!(e.app.tickets.is_empty(), "events for other projects are ignored");
  }

  #[test]
  fn sse_ticket_moved_to_review_updates_status_and_toasts() {
      let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
      let pid = e.app.project.id;
      e.app.tickets = vec![sample_ticket(pid, 1, "t", Status::InProgress)];
      e.apply_sse_event(CoreEvent::TicketMoved {
          id: 1, from: Status::InProgress, to: Status::Review, at: String::new(),
      });
      assert_eq!(e.app.tickets[0].status, Status::Review);
      let msg = e.app.status_message.as_ref().unwrap();
      assert!(msg.text.contains("Needs attention"));
      assert_eq!(msg.kind, crate::app::StatusKind::Info);
  }

  #[test]
  fn sse_ticket_deleted_removes_and_prunes() {
      let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
      let pid = e.app.project.id;
      e.app.tickets = vec![sample_ticket(pid, 1, "t", Status::Todo)];
      e.app.selected_ids.insert(1);
      e.apply_sse_event(CoreEvent::TicketDeleted { id: 1 });
      assert!(e.app.tickets.is_empty());
      assert!(!e.app.selected_ids.contains(&1));
  }
  ```
  Add a `sample_ticket` helper in the test mod (full `Ticket` literal like the one already in `models.rs` tests).
- [ ] **Run them (expect FAIL):** `cargo test -p kamaji engine::tests::sse_ticket_created_for_current_project_inserts` → compile error.
- [ ] **Implement `apply_sse_event`.** In `engine.rs`:
  ```rust
  /// Apply one SSE delta to the in-memory board for the CURRENT project. Events
  /// for other projects are ignored. Id-only events that need the full row
  /// (`session.*`) re-fetch via the client. Mirrors today's handle_poll_events
  /// toast for an auto-review move.
  pub fn apply_sse_event(&mut self, ev: kamaji_core::events::Event) {
      use kamaji_core::events::Event;
      let pid = self.app.project.id;
      match ev {
          Event::TicketCreated(t) | Event::TicketUpdated(t) => {
              if t.project_id != pid { return; }
              match self.app.tickets.iter_mut().find(|x| x.id == t.id) {
                  Some(slot) => *slot = t,
                  None => self.app.tickets.push(t),
              }
          }
          Event::TicketMoved { id, to, .. } => {
              if let Some(slot) = self.app.tickets.iter_mut().find(|x| x.id == id) {
                  slot.status = to;
                  match to {
                      Status::Review => self.app.set_info(format!("#{id} → Needs attention (agent idle)")),
                      Status::InProgress => self.app.set_info(format!("#{id} → In Progress (agent active)")),
                      _ => {}
                  }
              }
          }
          Event::TicketDeleted { id } => { self.app.tickets.retain(|x| x.id != id); }
          Event::SessionStarted { ticket_id, .. } | Event::SessionExited { ticket_id, .. } => {
              self.refetch_ticket(ticket_id);
          }
          Event::SessionIdle { .. } => { /* informational; the ticket.moved carries the column */ }
      }
      self.app.reclamp();
      self.app.prune_selection();
  }

  /// Splice a freshly-fetched ticket row (after a session.* event). Best-effort:
  /// a failed fetch leaves the stale row until the next refresh.
  fn refetch_ticket(&mut self, id: i64) {
      if let Ok(t) = self.client.get_ticket(id) {
          if t.project_id != self.app.project.id { return; }
          match self.app.tickets.iter_mut().find(|x| x.id == id) {
              Some(slot) => *slot = t,
              None => self.app.tickets.push(t),
          }
      }
  }
  ```
- [ ] **Run them (expect PASS):** `cargo test -p kamaji engine::tests` → all passed.
- [ ] **Drain SSE into the applier in `main.rs`.** In `run_board`, replace the 2a discard-drain with:
  ```rust
  while let Ok(msg) = sse_rx.try_recv() {
      match msg {
          sse::SseMsg::Connected => { let _ = engine.refresh_from_client(); }
          sse::SseMsg::Disconnected => engine.app.set_info("daemon stream lost — reconnecting…"),
          sse::SseMsg::Event(ev) => engine.apply_sse_event(ev),
      }
  }
  ```
  Render uses `engine.poll.levels()` today; keep that call (the poll loop still runs in 2b until 2c removes it).
- [ ] **Verify (manual):** two `kamaji` against one daemon — a ticket created/moved in one appears live in the other via SSE. `cargo test --workspace` green.
- [ ] **Commit:** `git commit -am "feat(tui): apply SSE deltas to the board; re-fetch on reconnect"`

---

## Step 2c — Writes go through the daemon; delete orchestration from `Engine`

The big step. Each mutation handler calls the client; core-driven implementations are deleted; `Effect` collapses; `detect_tick`/`reconcile`/`PollLoop`/`db`/`state_dir` are removed.

### Task 2c-1: collapse the `Effect` enum + update `main.rs` match

**Files:**
- Modify: `crates/kamaji/src/engine.rs`, `crates/kamaji/src/main.rs`

- [ ] **Write/adjust a test for the new Effect set.** Update `enter_attaches_to_existing_session` and `s_targets_the_main_session` expectations to the collapsed set, and add:
  ```rust
  #[test]
  fn effect_enum_has_only_the_collapsed_variants() {
      // Compile-time guard: constructing each remaining variant must type-check.
      let _ = [
          Effect::None,
          Effect::SwitchProject,
          Effect::SelfUpdate { version: "x".into() },
          Effect::Attach { name: "s".into() },
      ];
  }
  ```
- [ ] **Replace the enum.** In `engine.rs`:
  ```rust
  #[derive(Debug, PartialEq)]
  pub enum Effect {
      None,
      SwitchProject,
      SelfUpdate { version: String },
      Attach { name: String },
  }
  ```
  Delete `RunSession`, `RunSessionBackground`, `ResumeSession`.
- [ ] **Update `main.rs`'s match.** Replace the `Effect::RunSession`/`RunSessionBackground`/`ResumeSession` arms with just:
  ```rust
  Effect::Attach { name } => {
      run_zellij(terminal, engine, |_| zellij::attach_session(&name))?;
  }
  ```
  Keep `Effect::None`, `Effect::SwitchProject`, `Effect::SelfUpdate` arms.
- [ ] **Run it (expect compile FAILs in handlers — that's the next task).** This task ends red on `engine.rs` handler callsites that still return removed variants; fix them in 2c-2. To keep the commit green, do 2c-1 and 2c-2 in one commit if the compiler won't allow a green intermediate. **Sequencing:** combine 2c-1 + 2c-2 into a single working commit (the enum change forces all handlers to change together).

### Task 2c-2: rewire every mutation handler to the client; delete core orchestration

**Files:**
- Modify: `crates/kamaji/src/engine.rs`, `crates/kamaji/src/main.rs`

- [ ] **Rewrite `submit_form`** to call the client:
  ```rust
  fn submit_form(&mut self, form: &TicketForm) -> Result<Effect> {
      match form.editing_id {
          Some(id) => {
              match self.client.update_ticket(id, &form.title, Some(&form.description), form.prompt_opt().as_deref(), Some(form.agent)) {
                  Ok(_) => self.refresh_from_client()?,
                  Err(e) => self.app.set_error(format!("could not save ticket: {e:?}")),
              }
              Ok(Effect::None)
          }
          None => {
              let created = match self.client.create_ticket(self.app.project.id, &form.title, &form.description, form.prompt_opt().as_deref(), form.agent) {
                  Ok(t) => t,
                  Err(e) => { self.app.set_error(format!("could not create ticket: {e:?}")); return Ok(Effect::None); }
              };
              self.refresh_from_client()?;
              if !form.start_in_background { return Ok(Effect::None); }
              match self.client.start_ticket(created.id) {
                  Ok(t) => { self.refresh_from_client()?; self.app.set_info(format!("Started '{}' in the background", t.session_name.as_deref().unwrap_or(""))); }
                  Err(ClientError::BadRequest(m)) => self.app.set_error(m),
                  Err(e) => self.app.set_error(format!("could not start session: {e:?}")),
              }
              Ok(Effect::None)
          }
      }
  }
  ```
  > Note: the daemon's `/start` handles the worktree-location precondition (returns 400 "no worktree location configured…"). The TUI no longer opens the worktree picker on start; it surfaces the daemon's `BadRequest` reason as a toast. The `w` worktree-location modal stays (now persisting via `PATCH /config` — Task 2c-4), so the user can still set the location and retry. (This is a deliberate behavior simplification consistent with spec §4.6/§8: orchestration preconditions are the daemon's.)
- [ ] **Rewrite `apply_move`/`move_ticket`/`move_selected`** to compose client calls (§4.6):
  ```rust
  fn apply_move(&mut self, ticket: Ticket, target: Status) -> Result<Effect> {
      if target == Status::InProgress {
          return match ticket.session_name.clone() {
              Some(name) => {
                  if let Err(e) = self.client.move_ticket(ticket.id, Status::InProgress) {
                      self.app.set_error(format!("could not move: {e:?}")); return Ok(Effect::None);
                  }
                  self.refresh_from_client()?;
                  Ok(Effect::Attach { name })
              }
              None => match self.client.start_ticket(ticket.id) {
                  Ok(t) => {
                      self.refresh_from_client()?;
                      match t.session_name {
                          Some(name) => Ok(Effect::Attach { name }),
                          None => Ok(Effect::None),
                      }
                  }
                  Err(ClientError::BadRequest(m)) => { self.app.set_error(m); Ok(Effect::None) }
                  Err(e) => { self.app.set_error(format!("could not start: {e:?}")); Ok(Effect::None) }
              },
          };
      }
      match self.client.move_ticket(ticket.id, target) {
          Ok(_) => self.refresh_from_client()?,
          Err(e) => self.app.set_error(format!("could not move: {e:?}")),
      }
      Ok(Effect::None)
  }
  ```
  Keep `move_ticket(id, target)` and `move_selected(target)` as thin lookups that call `apply_move` (unchanged signatures).
- [ ] **Rewrite `Enter` handling in `on_board_key`** (no `enter_session`/`start_session`/`ensure_worktree_location`): if the focused ticket has a `session_name`, return `Effect::Attach { name }` (plain attach — resume is a tracked follow-up, spec §6.2); else `start_ticket` then `Attach` to the returned name, surfacing `BadRequest` as a toast. Reuse `apply_move(ticket, Status::InProgress)` semantics by calling it directly:
  ```rust
  KeyCode::Enter => {
      if let Some(t) = self.app.selected_ticket().cloned() {
          return self.apply_move(t, Status::InProgress);
      }
  }
  ```
  > This unifies Enter and move-to-In-Progress, which already matched semantically in the old code.
- [ ] **Rewrite `ConfirmDone` / `ConfirmDelete`** to call the client:
  ```rust
  Modal::ConfirmDone { ticket_ids } => match key.code {
      KeyCode::Char('y') => {
          for id in &ticket_ids { let _ = self.client.done_ticket(*id, true); }
          self.app.clear_selection(); self.refresh_from_client()?; Ok(Effect::None)
      }
      KeyCode::Char('n') => {
          for id in &ticket_ids { let _ = self.client.done_ticket(*id, false); }
          self.app.clear_selection(); self.refresh_from_client()?; Ok(Effect::None)
      }
      _ => Ok(Effect::None),
  },
  Modal::ConfirmDelete { ticket_id } => match key.code {
      KeyCode::Char('y') => {
          let _ = self.client.delete_ticket(ticket_id);
          self.refresh_from_client()?; Ok(Effect::None)
      }
      _ => Ok(Effect::None),
  },
  ```
- [ ] **Rewrite `s` (main session)** to call the client:
  ```rust
  KeyCode::Char('s') => {
      return match self.client.main_session(self.app.project.id) {
          Ok(name) => Ok(Effect::Attach { name }),
          Err(e) => { self.app.set_error(format!("could not open main session: {e:?}")); Ok(Effect::None) }
      };
  }
  ```
- [ ] **Delete dead methods + fields.** Remove from `Engine`: `db`, `state_dir`, `poll`, `prepare_session`, `main_session_effect`, `ensure_worktree_location`, `start_session`, `enter_session`, `cleanup_ticket`, `reconcile`, `forget_ticket_state`, `detect_tick`, `handle_poll_events`, `detect_tick_with`, `reload`. New struct:
  ```rust
  pub struct Engine {
      pub client: DaemonClient,
      pub config: Config,
      pub app: App,
      pub config_path: std::path::PathBuf,
  }
  ```
  `Engine::new(client, config, app)`. Remove `use kamaji_core::db::Db;`, `detect`, `session`, `git`, `slug`, `zellij` (except where still needed — `slug` no longer used; `zellij` no longer used in engine).
- [ ] **Update `main.rs`'s `run`/`run_board`** to drop `engine.db`/`engine.poll`/`detect_tick`/`reconcile`/`last_tick`:
  - `Engine::new(client, config, app)` (no `db`).
  - Remove the `detect_tick` block and `last_tick`.
  - `terminal.draw(|frame| ui::render(frame, &engine.app, &Default::default()))` — `ui::render`'s third arg was `engine.poll.levels()` (a `&HashMap<i64, SignalLevel>`). The poll loop is gone client-side; pass an empty map. (The daemon owns idle detection; the green "working" bullet, which read these levels, is a follow-up — see Self-Review.)
  - `run_zellij` no longer calls `engine.reconcile()`; replace with `engine.refresh_from_client()?`.
  - After the picker returns a project, seed via `client.list_tickets` (already done in 2b) and drop `engine.reconcile()` at startup.
  - Reclaim only `config = engine.config` and `client = engine.client` for the next project loop (no `db`).
  > Implementer note on the `ui::render` signature: check `crates/kamaji/src/ui/mod.rs::render`'s third param type. If it is `&std::collections::HashMap<i64, SignalLevel>`, pass `&std::collections::HashMap::new()`. If changing the signature is cleaner, drop the param — but that touches `ui/`, which the spec lists UNCHANGED; prefer passing an empty map.
- [ ] **Migrate engine tests.** Rewrite the orchestration tests to assert via the client/test daemon instead of `e.db`:
  - Tests that did `e.db.create_ticket(...)` + `e.reload()` → use `e.client.create_ticket(...)` + `e.refresh_from_client()`.
  - `idle_after_active_*`, `detect_tick_*`, `non_instrumented_*`, `move_back_survives_*`, `manual_drag_back_*`, `never_drags_*`, `cleanup_*`, `start_session_creates_worktree`, `enter_session_resumes_only_when_exited`, `create_with_background_*` — these asserted **daemon-owned** behavior (poll, worktree creation, resume). **Delete them from `engine.rs`** (the daemon's `crates/kamajid/tests/api.rs` and `kamaji-core` poll tests already cover the moved behavior). Leave a comment block listing what moved where.
  - Keep keymap→modal tests (`c`/`e`/`m`/`d`/`D`/`t`/`a`/`w`/search/`p`/`u`/space/multi-select), retargeted to the collapsed `Effect` and the client. For move/start/attach assertions, use the shared test daemon: e.g. `enter_attaches_to_existing_session` becomes "given a ticket the daemon reports with a session_name, Enter returns `Effect::Attach { name }`" by seeding via `e.client.create_ticket` + `e.client.start_ticket` (gated `#[ignore]` if it needs a real git repo + zellij), plus a cheaper unit test asserting `apply_move` on a ticket that already carries `session_name` returns `Attach` without a network round-trip (move call hits the daemon but a no-op move is cheap).
  > Be pragmatic: where a handler test now requires a real worktree/zellij (start/attach), mark it `#[ignore]` (Phase 1 convention) and keep a fast unit test for the pure branch decision (`apply_move` with `session_name` present → `Attach`).
- [ ] **Run (expect PASS):** `cargo test -p kamaji` → green (ignored tests skipped). `cargo test --workspace`.
- [ ] **Verify (manual smoke):** create/edit/move/start/attach/done/delete all flow through the daemon; auto-review still moves cards (daemon poll → SSE); two TUIs stay in sync; `kill <kamajid pid>` mid-run → board shows "reconnecting…" and the next command respawns the daemon (next task hardens this).
- [ ] **Commit:** `git commit -am "feat(tui): route all writes through the daemon; delete Engine orchestration; collapse Effect"`

### Task 2c-3: reconnect/respawn on `Unreachable`

**Files:**
- Modify: `crates/kamaji/src/main.rs`, `crates/kamaji/src/engine.rs`

- [ ] **Write a unit test for the reconnect predicate.** In `engine.rs`, add a helper `Engine::is_unreachable(err)` (or inline) and test that a `ClientError::Unreachable` triggers a reconnect flag while `BadRequest` does not. Since `reqwest::Error` is hard to construct, test the simpler classifier on a `ClientError` you build:
  ```rust
  #[test]
  fn bad_request_does_not_request_reconnect() {
      assert!(!crate::client::is_connection_lost(&ClientError::BadRequest("x".into())));
      assert!(!crate::client::is_connection_lost(&ClientError::NotFound));
  }
  ```
  And in `client.rs` add:
  ```rust
  /// True when the error means the daemon is unreachable (vs. a domain error),
  /// signaling the UI to re-probe/respawn the daemon.
  pub fn is_connection_lost(e: &ClientError) -> bool {
      matches!(e, ClientError::Unreachable(_))
  }
  ```
- [ ] **Run it (FAIL → implement → PASS):** `cargo test -p kamaji client::tests::is_connection_lost || cargo test -p kamaji bad_request_does_not_request_reconnect`.
- [ ] **Add reconnect handling in `main.rs`.** After draining SSE and on any command that returns `Unreachable`, attempt a bounded `daemon::ensure_daemon` re-probe, swap `engine.client`, restart the SSE thread (new channel), and `refresh_from_client`. Implement as a helper:
  ```rust
  fn try_reconnect(engine: &mut Engine, config: &config::Config, sse_tx: &mut /* re-spawn */ ...) { ... }
  ```
  Track a sticky status "daemon unreachable — reconnecting…". On repeated SSE `Disconnected` the listener already retries; the command path triggers `ensure_daemon`. Keep it bounded (a couple of attempts with backoff) so the loop never blocks indefinitely; on give-up, leave the toast and a usable read-stale board.
  > Implementer note: restarting the SSE thread means dropping the old `sse_rx`/handle and calling `sse::spawn(new_base, new_tx)`. Store the SSE handle + channel in a small struct owned by `run_board` so reconnect can replace them.
- [ ] **Verify (manual):** with the TUI running, `kill` the daemon; perform an action → "reconnecting…" toast, daemon respawns, action retried or surfaced; board recovers; SSE resumes (a change in a second TUI shows up again).
- [ ] **Commit:** `git commit -am "feat(tui): reconnect/respawn the daemon on Unreachable; restart SSE"`

### Task 2c-4: config edits (theme / default-agent / worktree-location) via `PATCH /config`

**Files:**
- Modify: `crates/kamaji/src/engine.rs`

- [ ] **Write failing tests** (against the shared test daemon, with `XDG_CONFIG_HOME` pointed at a tempdir so the real config isn't touched):
  ```rust
  #[test]
  fn theme_picker_enter_persists_via_daemon() {
      // Override XDG_CONFIG_HOME to a tempdir BEFORE spawning the test daemon, so
      // PATCH /config writes there. Open ThemePicker at "nord", press Enter, assert
      // e.config.theme == "nord" and the daemon's GET /config reflects it.
  }
  ```
  (Concretely: open `Modal::ThemePicker`, press Enter, then `assert_eq!(e.client.get_config().unwrap().theme, "nord")` and `assert_eq!(e.config.theme, "nord")`.)
- [ ] **Rewrite the three persist sites** in `on_key`'s modal handlers to call `self.client.update_config(...)` instead of `kamaji_core::config::save_to(&self.config_path, ...)`:
  - `Modal::ThemePicker` Enter: `self.client.update_config(Some(&chosen.name), None, None)`; on `Ok(cfg)` set `self.config = cfg` + info toast; on `Err` revert live theme + error toast.
  - `Modal::AgentPicker` Enter: `self.client.update_config(None, Some(chosen.as_str()), None)`.
  - `save_worktree_location`: `self.client.update_config(None, None, Some(&path.to_string_lossy()))`.
  Remove `config_path` field usages for these (the daemon owns the file now). Keep the field only if other code references it; otherwise delete it and its constructor init.
- [ ] **Run (expect PASS):** `cargo test -p kamaji engine::tests` (the new config tests + retained picker-navigation tests). The old `picker_enter_persists_theme_to_config_file` style tests that asserted the local file are replaced by the daemon-backed versions above; delete the file-asserting ones.
- [ ] **Commit:** `git commit -am "feat(tui): persist theme/agent/worktree config via PATCH /config"`

---

## Step 2d — CLI subcommand through the daemon + cleanup

### Task 2d-1: `ticket create` dispatches via the daemon

**Files:**
- Modify: `crates/kamaji/src/cli.rs`, `crates/kamaji/src/main.rs`

- [ ] **Write failing tests** for a new client-based create path. Rework `run_create_ticket` to take a `&DaemonClient` instead of `&Db` + `state_dir`, returning a simpler `CreateOutcome` (the daemon now does the background start). Add a test against the shared test daemon:
  ```rust
  #[test]
  fn cli_create_ticket_via_daemon_infers_single_project() {
      // Boot test daemon; create one project via client; run_create_ticket with
      // project=None and a prompt; assert one ticket exists with the prompt.
  }

  #[test]
  fn cli_create_ticket_background_starts_via_daemon() {
      // Gated #[ignore] (needs git repo + zellij): project=None, background=true;
      // assert the returned ticket has a session_name and is In Progress.
  }
  ```
  Project inference (`select_project`/`infer_project`) now uses `client.list_projects()` + `client.list_tickets()` instead of `db.*` — keep the same matching logic, just swap the data source.
- [ ] **Run them (FAIL → implement → PASS).** Rewrite `run_create_ticket`:
  ```rust
  pub fn run_create_ticket(client: &DaemonClient, config: &Config, args: &CreateTicketArgs, cwd: &Path) -> Result<CreateOutcome> {
      let project = match args.project.as_deref() {
          Some(sel) => select_project(client, sel)?,
          None => infer_project(client, cwd)?,
      };
      let agent = args.agent.or(project.default_agent).unwrap_or_else(|| config.default_agent());
      let title = args.title_or_prompt()?;
      let prompt = args.prompt.as_deref().map(str::trim).filter(|s| !s.is_empty());
      let ticket = client.create_ticket(project.id, &title, args.description.trim(), prompt, agent)
          .map_err(|e| anyhow!("{e:?}"))?;
      let mut message = format!("Created ticket #{} in project {}: {}", ticket.id, project.name, ticket.title);
      if args.background {
          match client.start_ticket(ticket.id) {
              Ok(t) => message.push_str(&format!("\nStarted '{}' in the background", t.session_name.as_deref().unwrap_or(""))),
              Err(ClientError::BadRequest(m)) => return Ok(CreateOutcome { message, warning: Some(m), launch: None, background_failed: true }),
              Err(e) => return Ok(CreateOutcome { message, warning: Some(format!("{e:?}")), launch: None, background_failed: true }),
          }
      }
      Ok(CreateOutcome { message, warning: None, launch: None, background_failed: args.background == false && false })
  }
  ```
  Drop `LaunchSpec`/`launch` (the daemon spawns the session now); simplify `CreateOutcome` to `{ message, warning, background_failed }`. Update `select_project`/`infer_project` to take `&DaemonClient`.
- [ ] **Update `main.rs`'s `CreateTicket` arm** to `ensure_daemon` (with default `DaemonOpts`) then call the new `run_create_ticket`; print `outcome.message`; on `background_failed` print the warning to stderr and `exit(1)`. Delete the old `zellij::create_session_background` + teardown block (the daemon owns it).
- [ ] **Run (expect PASS):** `cargo test -p kamaji cli::tests` → green (ignored tests skipped).
- [ ] **Commit:** `git commit -am "feat(tui): ticket create dispatches through the daemon"`

### Task 2d-2: delete dead binary-crate imports + final sweep

**Files:**
- Modify: `crates/kamaji/src/main.rs`, `crates/kamaji/src/cli.rs`, `crates/kamaji/src/engine.rs`

- [ ] **Remove now-dead imports/helpers.** In `main.rs`: drop `use kamaji_core::db::Db;` (no longer opened by the binary), `detect`, `models`, `db_path`, and any `session`/`git`/`PollLoop` references. Keep `zellij` only for `attach_session`. In `cli.rs`: drop `use kamaji_core::db::Db;` and `session`. Confirm with a grep that no `crates/kamaji` path opens the DB or shells zellij except `zellij::attach_session`:
  ```
  grep -rn "Db::open\|db.list_\|db.create_\|create_session\|terminate_session\|list_sessions\|PollLoop\|detect_tick\|reconcile" crates/kamaji/src
  ```
  Expect zero hits outside test helpers and `attach_session`.
- [ ] **Run clippy + fmt + full test:** `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace`. All green.
- [ ] **Verify (final manual smoke):** `kamaji ticket create "do X" --background` creates + starts via the daemon; `kamaji` TUI does the full lifecycle through the daemon; two TUIs stay in sync; killing the daemon mid-run respawns; native attach execs `zellij attach <name>` and returns cleanly.
- [ ] **Commit:** `git commit -am "chore(tui): remove dead DB/zellij/poll imports; final Phase 2 cleanup"`

---

## Self-Review

**Spec coverage (every section → a task):**
- §3.2 file structure → File Structure section + all tasks. §4 client (4.1 connect, 4.2 reads, 4.3 commands, 4.4 ClientError, 4.5 threading, 4.6 compose/gaps) → Tasks 2a-1/2/3 (+ move-to-In-Progress composition in 2c-2; `main-session` in 2-pre-D/2a-3). §5 SSE (5.1 reconnect, 5.2 apply) → Tasks 2a-4 (`from_sse`), 2a-5 (listener), 2b-3 (apply). §6 native attach + collapsed `Effect` (6.1) + resume simplification (6.2) → Tasks 2c-1/2c-2 (plain `Attach`; resume is a tracked follow-up, noted inline). §7 auto-spawn (7.1 files, 7.2 liveness, 7.3 race, 7.4 detached spawn, 7.5 health-wait) → Tasks 2-pre-A/B (paths + daemon writes), 2a-6/2a-7. §8 migration steps 2a/2b/2c/2d → the four step sections, each ending green and TUI-working. §9 edges (9.1 startup fatal, 9.2 daemon dies, 9.3 SSE lag, 9.4 skew, 9.5 stale port) → 2a-7 (startup fatal before `ratatui::init()`), 2c-3 (reconnect), 2a-5/2b-3 (`Connected`→re-fetch heals lag), 2a-1 (version captured for skew warning — surfacing the warning toast is a small follow-up noted below). §10 testing strategy → per-task tests (client happy/negative, daemon race/stale/health-timeout pure + `#[ignore]` live, `from_sse` round-trip, SSE listener unit + gated live, retargeted engine tests). §11 three daemon additions → Tasks 2-pre-B/C/D, sequenced first. §12 non-goals respected (no browser, no auth, `AttachInfo.web_url/token` ignored).

**No placeholders:** every code step contains real Rust matching this codebase (actual `Event`/`Status`/`Ticket`/`Config` types, the real `payload_json` framing, the real `routes/tickets.rs::start` returning `Ticket`, the real `session::prepare_main_session`/`zellij::create_session_background` signatures, the real `AppState`/`router`/`serve`, the `OnceLock` test-daemon pattern mirroring `api.rs::spawn`). Every test step gives the exact `cargo test …` command and expected PASS/FAIL.

**Type consistency across tasks:** `DaemonClient` is introduced in 2a-1 and its method signatures (used by `picker.rs`, `engine.rs`, `cli.rs`) are fixed in 2a-2/2a-3 before any caller in 2b/2c/2d uses them. `Engine` gains `client` in 2b-2 and loses `db`/`poll`/`state_dir` in 2c-2 in one compiling commit. `Effect` collapses in a combined 2c-1+2c-2 commit so no intermediate references a deleted variant. `events::from_sse` (2a-4) is the single decode path used by `sse.rs` (2a-5) and round-trips the daemon's `sse_name`+`payload_json`.

**Known follow-ups deliberately deferred (file as issues during execution, per CLAUDE.md §2):** (1) daemon-side conversation-*resume* endpoint (spec §6.2) — TUI does plain attach; (2) the "working" green bullet in `ui::render` previously fed by `engine.poll.levels()` now gets an empty map — re-deriving per-session activity from `session.idle`/`session.started` SSE state is a small client-side follow-up; (3) surfacing the version-skew warning toast from the captured `/healthz` version (the value is already captured in `DaemonClient::version`).
