# Phase 1c — `kamajid` Daemon: Session Lifecycle + Auto-Review Poll Task Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `kamajid` daemon functionally complete for sessions: add `POST /tickets/:id/start` (create a worktree + agent session), `POST /tickets/:id/done` (with optional cleanup), and a background **auto-review poll task** that moves idle agents' tickets to Review and broadcasts `session.idle`/`ticket.moved` — the daemon's headline autonomous behavior.

**Architecture:** Builds on Plan 1b's daemon (HTTP API + SSE + `AppState` + broadcast). Adds a new additive `kamaji-core::session::cleanup_ticket` helper, gives `AppState` a `state_dir` and a public `Arc<Mutex<Db>>` handle, two orchestration routes, and a poll task. The poll task reuses the `kamaji_core::poll::PollLoop` from Plan 1a — it gathers in-progress/review tickets, runs one tick, and emits the returned events. The detection of an idle **Claude** session works in CI without zellij because it reads a marker **file** (`detect::marker_level`), so the headline path is integration-testable; the worktree side uses real temporary git repos (git is available in CI); only the actual zellij session spawn is un-CI-testable and is exercised by an error/rollback path plus a manual smoke.

**Tech Stack:** Rust 2021, `axum` 0.7, `tokio` (incl. `time`), the existing `kamaji-core` (`session`, `poll`, `git`, `zellij`, `detect`, `db`). No new crates or external deps.

**Parent spec:** `docs/superpowers/specs/2026-05-30-phase-1-kamajid-design.md` (§4 the `/start`/`/done` routes, §3 `session.*` events, §9 async/DB). This plan implements those plus the poll task from §8's rollout step 4. It **defers** `zellij web` management + `POST /tickets/:id/attach` (§6) and `session.exited` reconciliation to **Plan 1d**.

**Precondition:** Plan 1b merged. On `main`, `cargo test --all-targets --all-features` reports 152 (kamaji) + 96 (kamaji-core) + 8 (kamajid) = 256 passing.

**Relevant existing API (verified):**
- `kamaji_core::session::prepare_session(project: &Project, config: &Config, state_dir: &Path, ticket: &Ticket) -> Result<Prepared>` where `Prepared { name: String, layout_path: PathBuf, worktree: PathBuf, instrumented: bool }`.
- `kamaji_core::session::commit_session(db: &Db, ticket_id: i64, p: &Prepared) -> Result<()>` (writes session columns + instrumented flag + status InProgress).
- `kamaji_core::zellij::create_session_background(name: &str, layout_path: &Path, cwd: &Path) -> Result<()>` (spawns zellij — fails if zellij is absent); `zellij::terminate_session(name: &str)` (ignores errors).
- `kamaji_core::git::{remove_worktree(root, wt), delete_branch(root, branch), is_git_repo(root)}`.
- `kamaji_core::detect::{marker_path(state_dir, session), default_state_dir()}`.
- `kamaji_core::poll::PollLoop` — `new()`, `rehydrate(&[Ticket])`, `tick(&[Ticket], &Db, &Config, &Path) -> Result<Vec<Event>>`.
- `db.get_project(id)`, `db.list_projects()`, `db.list_tickets(project_id)`, `db.get_ticket(id)`, `db.set_ticket_status(id, status)`, `db.clear_ticket_session(id)`.
- From Plan 1b: `AppState { db (private Arc<Mutex<Db>>), pub config: Arc<Config>, pub tx: broadcast::Sender<Event> }` with `new(db, config)`, `with_db`, `emit`. `ApiError { NotFound, BadRequest(String), Internal(anyhow::Error) }` (derives Debug).

**Repo conventions (from `CLAUDE.md`):** all work on a branch in a worktree (the executing skill sets this up), never on `main`. Commit style mirrors history (`feat(kamajid): …`, `feat(core): …`). Ship at the end with `gh pr create --fill --base main` → `gh pr merge --squash --auto --delete-branch` (the `--delete-branch` errors from inside a worktree but the merge still lands — verify + clean up manually).

---

## Verification commands (run at every checkpoint)

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E '(test result:|Running )'
```

---

## File Structure (after this plan)

```
crates/kamaji-core/src/session.rs       MODIFIED — add pub fn cleanup_ticket
crates/kamajid/src/state.rs             MODIFIED — add state_dir + db_handle()
crates/kamajid/src/routes/tickets.rs    MODIFIED — add start, done handlers
crates/kamajid/src/poll_task.rs         NEW — poll_round + spawn_poll_task
crates/kamajid/src/lib.rs               MODIFIED — mount routes, pub mod poll_task
crates/kamajid/src/main.rs              MODIFIED — spawn the poll task
crates/kamajid/tests/api.rs             MODIFIED — orchestration + poll tests
crates/kamajid/tests/support.rs         NEW — temp-git-repo helper (shared by tests)
```

---

## Task 1: Add `session::cleanup_ticket` to `kamaji-core`

Extract the session-teardown sequence (terminate zellij session, remove worktree, delete branch, clear DB columns, remove the idle marker) into a reusable core function the daemon's `/done` route will call. This is **additive** — the TUI keeps its own `Engine::cleanup_ticket` for now (it is retired in Phase 2 when the TUI becomes a daemon client); the mild duplication is intentional and low-risk.

**Files:**
- Modify: `crates/kamaji-core/src/session.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/kamaji-core/src/session.rs`:

```rust
    /// cleanup_ticket removes the worktree + branch and clears the ticket's
    /// session columns. Uses a real temp git repo (git is available in tests).
    #[test]
    fn cleanup_ticket_removes_worktree_and_clears_session() {
        // A committed git repo so `worktree add` has a base.
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        let run = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.email", "t@t.t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("README.md"), "hi").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "init"]);

        let worktree = repo.path().join("..").join("kamaji-wt-cleanup");
        let _ = crate::git::remove_worktree(root, &worktree); // ignore if absent
        crate::git::add_worktree(root, &worktree, "kamaji-1-x", "main").unwrap();
        assert!(worktree.exists());

        let state_dir = tempfile::tempdir().unwrap();
        let marker = crate::detect::marker_path(state_dir.path(), "kamaji-1-x");
        std::fs::write(&marker, "").unwrap();

        let db = Db::open_in_memory().unwrap();
        let p = db.create_project("p", root, None).unwrap();
        let t = db.create_ticket(p.id, "x", "", None, Agent::Claude).unwrap();
        db.set_ticket_session(
            t.id,
            "kamaji-1-x",
            &worktree.to_string_lossy(),
            "kamaji-1-x",
        )
        .unwrap();

        cleanup_ticket(&db, root, state_dir.path(), t.id).unwrap();

        assert!(!worktree.exists(), "worktree should be removed");
        assert!(!marker.exists(), "idle marker should be removed");
        let got = db.get_ticket(t.id).unwrap().unwrap();
        assert_eq!(got.session_name, None);
        assert_eq!(got.worktree_path, None);
        assert_eq!(got.branch, None);
    }
```

Run: `cargo test -p kamaji-core session::tests::cleanup_ticket_removes_worktree_and_clears_session`
Expected: FAIL — `cleanup_ticket` is undefined (compile error).

- [ ] **Step 2: Implement `cleanup_ticket`**

Add this `pub fn` to `crates/kamaji-core/src/session.rs` (after `commit_session`, before the test module). Note the existing imports at the top of the file already bring in `git`, `detect`, `zellij_config`, etc. via `use crate::{agent, detect, git, layout, slug, zellij_config};` and `use crate::zellij;` — confirm `git`, `detect`, and `zellij` are in scope; if `zellij` is not imported at the top, add `use crate::zellij;`.

```rust
/// Tear down a ticket's session: kill the zellij session, remove its worktree
/// and branch, delete the idle marker, and clear the ticket's session columns.
/// Best-effort on the external steps (a session/worktree may already be gone);
/// only the DB clear is required to succeed. Mirrors the TUI's
/// `Engine::cleanup_ticket` so the daemon and TUI tear down identically.
pub fn cleanup_ticket(
    db: &Db,
    root_dir: &Path,
    state_dir: &Path,
    ticket_id: i64,
) -> Result<()> {
    if let Some(t) = db.get_ticket(ticket_id)? {
        if let Some(name) = &t.session_name {
            zellij::terminate_session(name);
            let _ = std::fs::remove_file(detect::marker_path(state_dir, name));
        }
        if let Some(wt) = &t.worktree_path {
            let _ = git::remove_worktree(root_dir, wt);
        }
        if let Some(b) = &t.branch {
            let _ = git::delete_branch(root_dir, b);
        }
        db.clear_ticket_session(ticket_id)?;
    }
    Ok(())
}
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p kamaji-core session::tests::cleanup_ticket_removes_worktree_and_clears_session`
Expected: PASS.

- [ ] **Step 4: Full verification**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E 'test result:'
```

Expected: all green; kamaji-core now 97 (96 + 1).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): add session::cleanup_ticket

Reusable session teardown (kill zellij session, remove worktree +
branch, delete idle marker, clear DB columns) that the daemon's
/done route will call. Mirrors the TUI's Engine::cleanup_ticket;
additive (the TUI keeps its copy until Phase 2). Phase 1c step 1."
```

---

## Task 2: `AppState` gains a `state_dir` + DB handle; add `POST /tickets/:id/start`

The `/start` route creates a worktree + agent session for a ticket. It needs the daemon's `state_dir` (for marker placement during `prepare_session`) and the project. On a zellij spawn failure it rolls back (terminate the half-created session, clear the DB columns) so the ticket is left recoverable.

**Files:**
- Modify: `crates/kamajid/src/state.rs`
- Create: `crates/kamajid/tests/support.rs`
- Modify: `crates/kamajid/src/routes/tickets.rs`, `crates/kamajid/src/lib.rs`, `crates/kamajid/tests/api.rs`

- [ ] **Step 1: Add `state_dir` + `db_handle` to `AppState`**

In `crates/kamajid/src/state.rs`, add a `state_dir` field and accessors. Update the struct, `new`, and add `set_state_dir`/`state_dir`/`db_handle`:

```rust
use std::path::PathBuf;
// ... existing imports ...

#[derive(Clone)]
pub struct AppState {
    db: Arc<Mutex<Db>>,
    pub config: Arc<Config>,
    pub tx: broadcast::Sender<Event>,
    state_dir: Arc<PathBuf>,
}

impl AppState {
    pub fn new(db: Db, config: Config) -> Self {
        let (tx, _rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        AppState {
            db: Arc::new(Mutex::new(db)),
            config: Arc::new(config),
            tx,
            state_dir: Arc::new(kamaji_core::detect::default_state_dir()),
        }
    }

    /// Override the per-session idle-marker directory. Call before sharing the
    /// state (tests use a temp dir; production uses the default).
    pub fn set_state_dir(&mut self, dir: PathBuf) {
        self.state_dir = Arc::new(dir);
    }

    /// The per-session idle-marker directory.
    pub fn state_dir(&self) -> &std::path::Path {
        &self.state_dir
    }

    /// A clone of the shared DB handle, for code that locks it directly (the
    /// poll task) rather than going through the async `with_db` helper.
    pub fn db_handle(&self) -> Arc<Mutex<Db>> {
        self.db.clone()
    }

    // ... existing with_db, emit unchanged ...
}
```

- [ ] **Step 2: Write `crates/kamajid/tests/support.rs` (shared temp-git-repo helper)**

```rust
//! Shared test support: build a committed temp git repo so the daemon's
//! worktree-creating routes have a real base branch to branch from.

#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

/// Create a committed git repo at a fresh temp dir and return it. The repo has
/// one commit on `main` so `git worktree add -b … main` works.
pub fn committed_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let run = |args: &[&str]| {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap()
                .status
                .success(),
            "git {args:?} failed"
        );
    };
    run(&["init", "-b", "main"]);
    run(&["config", "user.email", "t@t.t"]);
    run(&["config", "user.name", "t"]);
    std::fs::write(root.join("README.md"), "hi").unwrap();
    run(&["add", "."]);
    run(&["commit", "-m", "init"]);
    dir
}

/// True if a `zellij` binary is on PATH (the live session-spawn path needs it).
pub fn zellij_available() -> bool {
    let _ = Path::new("/"); // silence unused import on some platforms
    Command::new("zellij")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
```

- [ ] **Step 3: Add the `start` handler to `tickets.rs`**

Append to `crates/kamajid/src/routes/tickets.rs`. The handler separates the three error classes cleanly without smuggling sentinels through `anyhow`: missing ticket/project are looked up first (→ 404); the prepare step returns an **inner** `Result` so a precondition failure (no `worktree_base`, non-git root) maps to 400 while a genuine DB error from `with_db` maps to 500; a zellij spawn failure rolls back and maps to 500.

```rust
use kamaji_core::session;

/// `POST /tickets/:id/start` → create the ticket's worktree + agent session in
/// the background, record it, and move the ticket to In Progress. Emits
/// `session.started`. Missing ticket/project → 404. A preparation failure (no
/// `worktree_base` configured, or a non-git project root) → 400. A zellij spawn
/// failure rolls back the half-created session and returns 500, leaving the
/// ticket recoverable (no session recorded).
pub async fn start(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Ticket>, ApiError> {
    let state_dir = state.state_dir().to_path_buf();
    let config = (*state.config).clone();

    // Fetch ticket + its project up front so a missing row is a clean 404.
    let (ticket, project) = state
        .with_db(move |db| {
            let ticket = db.get_ticket(id)?;
            let project = match &ticket {
                Some(t) => db.get_project(t.project_id)?,
                None => None,
            };
            Ok((ticket, project))
        })
        .await?;
    let ticket = ticket.ok_or(ApiError::NotFound)?;
    let project = project.ok_or(ApiError::NotFound)?;

    // Prepare (worktree + layout) + commit, on the blocking pool. The closure's
    // OUTER error (via `?`) is a real DB failure → 500; the INNER `Err(String)`
    // is a preparation precondition failure → 400.
    let prepared = state
        .with_db(move |db| {
            match session::prepare_session(&project, &config, &state_dir, &ticket) {
                Ok(p) => {
                    session::commit_session(db, id, &p)?;
                    Ok(Ok((p.name, p.layout_path, p.worktree)))
                }
                Err(e) => Ok(Err(e.to_string())),
            }
        })
        .await?;
    let (name, layout_path, worktree) = match prepared {
        Ok(triple) => triple,
        Err(msg) => return Err(ApiError::BadRequest(msg)),
    };

    // Phase 2: spawn the zellij session (the only step needing the zellij binary).
    if let Err(e) = kamaji_core::zellij::create_session_background(&name, &layout_path, &worktree) {
        // Roll back: kill any partially-created session and clear the columns so
        // the ticket is recoverable (no session recorded).
        kamaji_core::zellij::terminate_session(&name);
        let _ = state
            .with_db(move |db| {
                db.clear_ticket_session(id)?;
                Ok(())
            })
            .await;
        return Err(ApiError::Internal(anyhow::anyhow!(
            "starting session failed: {e}"
        )));
    }

    state.emit(Event::SessionStarted {
        ticket_id: id,
        session_name: name,
    });
    let ticket = state
        .with_db(move |db| db.get_ticket(id))
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(ticket))
}
```

Note the closure return type is `anyhow::Result<Result<(String, PathBuf, PathBuf), String>>` — `with_db`'s `T` is the inner `Result<…, String>`. `PathBuf` is already in scope via the handlers' imports; if not, add `use std::path::PathBuf;` at the top of `tickets.rs`.

- [ ] **Step 4: Mount the route in `lib.rs`**

In `crates/kamajid/src/lib.rs`, add to `router` (after the `/tickets/:id/move` route):

```rust
        .route(
            "/tickets/:id/start",
            axum::routing::post(routes::tickets::start),
        )
```

- [ ] **Step 5: Write the `/start` tests**

Append to `crates/kamajid/tests/api.rs`. First add `mod support;` near the top (after the existing `use` lines):

```rust
mod support;
```

Then the tests:

```rust
#[tokio::test]
async fn start_without_worktree_base_is_400() {
    // Default config has worktree_base = None, so prepare fails before zellij.
    let (base, state) = spawn().await;
    let repo = support::committed_repo();
    let tid = state
        .with_db({
            let root = repo.path().to_path_buf();
            move |db| {
                let p = db.create_project("p", &root, None)?;
                let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
                Ok(t.id)
            }
        })
        .await
        .unwrap();

    let resp = reqwest::Client::new()
        .post(format!("{base}/tickets/{tid}/start"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["kind"], "bad_request");
    // The ticket has no session recorded.
    let t: serde_json::Value = reqwest::get(format!("{base}/tickets/{tid}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(t["session_name"].is_null());
}

#[tokio::test]
async fn start_on_non_git_project_is_400() {
    // worktree_base set, but the project root is not a git repo → prepare fails.
    let mut cfg = kamaji_core::config::Config::default();
    cfg.worktree_base = Some(format!("{}/wt", std::env::temp_dir().display()));
    let mut state = kamajid::state::AppState::new(Db::open_in_memory().unwrap(), cfg);
    // Bind the temp dir for the test's lifetime so it isn't cleaned early.
    let sd = tempfile::tempdir().unwrap();
    state.set_state_dir(sd.path().to_path_buf());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = kamajid::router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{addr}");

    let not_a_repo = tempfile::tempdir().unwrap();
    let tid = state
        .with_db({
            let root = not_a_repo.path().to_path_buf();
            move |db| {
                let p = db.create_project("p", &root, None)?;
                let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
                Ok(t.id)
            }
        })
        .await
        .unwrap();

    let resp = reqwest::Client::new()
        .post(format!("{base}/tickets/{tid}/start"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn start_missing_ticket_is_404() {
    let (base, _state) = spawn().await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/tickets/999/start"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}
```

Note on `tempfile::TempDir::keep`: `set_state_dir` needs an owned `PathBuf` that outlives the test; `tempfile::tempdir().unwrap().keep()` returns the `PathBuf` and leaks the temp dir (acceptable in a test — the OS cleans `/tmp`). If your `tempfile` version exposes `into_path` instead of `keep`, use that; both return the `PathBuf`.

- [ ] **Step 6: Build, test**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test -p kamajid 2>&1 | grep -E 'test result:'
cargo test --all-targets --all-features 2>&1 | grep -E 'test result:'
```

Expected: `kamajid` now has 11 tests (8 + 3 new), all green. (None of these reach the zellij spawn — they fail at `prepare_session` or the existence check, which is exactly the CI-safe surface.)

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(kamajid): POST /tickets/:id/start

Create the ticket's worktree + agent session in the background and
move it to In Progress, emitting session.started. AppState gains a
state_dir (marker dir) and a db_handle accessor. Preparation errors
(no worktree_base, non-git root) map to 400; a zellij spawn failure
rolls back the half-created session and returns 500. Tests cover the
400/404 precondition paths (CI-safe; no zellij needed). Phase 1c
step 2."
```

---

## Task 3: `POST /tickets/:id/done` (with optional cleanup)

Move a ticket to Done; when `cleanup` is requested, tear down its worktree/session via the core `cleanup_ticket` from Task 1. Emits `ticket.moved` to Done and, when cleaned, `session.exited`.

**Files:**
- Modify: `crates/kamajid/src/routes/tickets.rs`, `crates/kamajid/src/lib.rs`, `crates/kamajid/tests/api.rs`

- [ ] **Step 1: Add the `done` handler to `tickets.rs`**

Append to `crates/kamajid/src/routes/tickets.rs`:

```rust
#[derive(Deserialize)]
pub struct DoneTicket {
    /// When true, tear down the ticket's worktree + zellij session + branch.
    #[serde(default)]
    pub cleanup: bool,
}

/// `POST /tickets/:id/done` → move the ticket to Done. With `{"cleanup": true}`,
/// also tears down its worktree/session/branch. Emits `ticket.moved` (to done)
/// and, when cleaned and a session existed, `session.exited`.
pub async fn done(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<DoneTicket>,
) -> Result<Json<Ticket>, ApiError> {
    let state_dir = state.state_dir().to_path_buf();
    let cleanup = body.cleanup;

    let outcome = state
        .with_db(move |db| {
            let Some(ticket) = db.get_ticket(id)? else {
                return Ok(None);
            };
            let from = ticket.status;
            let session_name = ticket.session_name.clone();
            if cleanup {
                // root_dir comes from the ticket's project.
                if let Some(project) = db.get_project(ticket.project_id)? {
                    session::cleanup_ticket(db, &project.root_dir, &state_dir, id)?;
                }
            }
            db.set_ticket_status(id, kamaji_core::models::Status::Done)?;
            let updated = db.get_ticket(id)?.expect("ticket exists; just updated");
            Ok(Some((from, session_name, updated)))
        })
        .await?;

    let (from, session_name, ticket) = outcome.ok_or(ApiError::NotFound)?;
    if from != kamaji_core::models::Status::Done {
        state.emit(Event::TicketMoved {
            id,
            from,
            to: kamaji_core::models::Status::Done,
            at: chrono::Utc::now().to_rfc3339(),
        });
    }
    if cleanup {
        if let Some(name) = session_name {
            state.emit(Event::SessionExited {
                ticket_id: id,
                session_name: name,
            });
        }
    }
    Ok(Json(ticket))
}
```

- [ ] **Step 2: Mount the route in `lib.rs`**

In `crates/kamajid/src/lib.rs`, add to `router` (after `/tickets/:id/start`):

```rust
        .route(
            "/tickets/:id/done",
            axum::routing::post(routes::tickets::done),
        )
```

- [ ] **Step 3: Write the `/done` tests**

Append to `crates/kamajid/tests/api.rs`:

```rust
#[tokio::test]
async fn done_without_cleanup_moves_to_done() {
    let (base, state) = spawn().await;
    let tid = state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
            db.set_ticket_status(t.id, kamaji_core::models::Status::InProgress)?;
            Ok(t.id)
        })
        .await
        .unwrap();

    let ticket: serde_json::Value = reqwest::Client::new()
        .post(format!("{base}/tickets/{tid}/done"))
        .json(&serde_json::json!({ "cleanup": false }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ticket["status"], "done");
}

#[tokio::test]
async fn done_with_cleanup_tears_down_worktree() {
    let (base, state) = spawn().await;
    let repo = support::committed_repo();
    let worktree = repo.path().join("..").join("kamaji-wt-done");
    let _ = kamaji_core::git::remove_worktree(repo.path(), &worktree);
    kamaji_core::git::add_worktree(repo.path(), &worktree, "kamaji-9-x", "main").unwrap();
    assert!(worktree.exists());

    let tid = state
        .with_db({
            let root = repo.path().to_path_buf();
            let wt = worktree.to_string_lossy().to_string();
            move |db| {
                let p = db.create_project("p", &root, None)?;
                let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
                db.set_ticket_session(t.id, "kamaji-9-x", &wt, "kamaji-9-x")?;
                db.set_ticket_status(t.id, kamaji_core::models::Status::InProgress)?;
                Ok(t.id)
            }
        })
        .await
        .unwrap();

    let resp = reqwest::Client::new()
        .post(format!("{base}/tickets/{tid}/done"))
        .json(&serde_json::json!({ "cleanup": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(!worktree.exists(), "cleanup should remove the worktree");

    let t: serde_json::Value = reqwest::get(format!("{base}/tickets/{tid}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(t["status"], "done");
    assert!(t["session_name"].is_null());
}
```

- [ ] **Step 4: Build, test**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test -p kamajid 2>&1 | grep -E 'test result:'
cargo test --all-targets --all-features 2>&1 | grep -E 'test result:'
```

Expected: `kamajid` now has 13 tests (11 + 2), all green.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(kamajid): POST /tickets/:id/done with optional cleanup

Move a ticket to Done; with {cleanup:true} tear down its worktree +
session + branch via core session::cleanup_ticket. Emits ticket.moved
(to done) and, when cleaned, session.exited. Tests cover both paths
with a real temp git repo for the worktree teardown. Phase 1c step 3."
```

---

## Task 4: The auto-review poll background task

Add the periodic task that gives the daemon its headline autonomous behavior: gather in-progress/review tickets, run one `PollLoop::tick`, and broadcast the resulting events. The detection of an idle **Claude** session is a marker-file check, so this is integration-testable in CI without zellij. The task is split into a testable `poll_round` (one tick + emit, deterministic) and a thin `spawn_poll_task` (the interval loop) wired into `main`.

**Files:**
- Create: `crates/kamajid/src/poll_task.rs`
- Modify: `crates/kamajid/src/lib.rs`, `crates/kamajid/src/main.rs`, `crates/kamajid/tests/api.rs`

- [ ] **Step 1: Write `crates/kamajid/src/poll_task.rs`**

```rust
//! The auto-review poll task: periodically detect idle agent sessions and move
//! their tickets to Review, broadcasting the resulting events. Reuses
//! `kamaji_core::poll::PollLoop` (the same detection the TUI uses).

use std::path::Path;
use std::time::Duration;

use kamaji_core::db::Db;
use kamaji_core::poll::PollLoop;

use crate::state::AppState;

/// Gather every in-progress/review ticket across all projects.
fn all_tickets(db: &Db) -> Vec<kamaji_core::models::Ticket> {
    let mut out = Vec::new();
    if let Ok(projects) = db.list_projects() {
        for p in projects {
            if let Ok(tickets) = db.list_tickets(p.id) {
                out.extend(tickets);
            }
        }
    }
    out
}

/// Run ONE poll round: lock the DB, gather tickets, tick the detector, and
/// broadcast the events that fired. Synchronous DB work is done under the lock
/// (no `.await` held across it). Public so tests can drive rounds deterministically.
pub async fn poll_round(state: &AppState, poll: &mut PollLoop, state_dir: &Path) {
    let events = {
        let db = state.db_handle();
        let db = db.lock().expect("db mutex poisoned");
        let tickets = all_tickets(&db);
        poll.tick(&tickets, &db, &state.config, state_dir)
            .unwrap_or_default()
    };
    for ev in events {
        state.emit(ev);
    }
}

/// Spawn the background poll loop. Ticks every `interval`. Rehydrates auto-review
/// provenance once from the DB at startup, then maintains it across ticks.
pub fn spawn_poll_task(state: AppState, interval: Duration) {
    let state_dir = state.state_dir().to_path_buf();
    tokio::spawn(async move {
        let mut poll = PollLoop::new();
        {
            let db = state.db_handle();
            let db = db.lock().expect("db mutex poisoned");
            poll.rehydrate(&all_tickets(&db));
        }
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            poll_round(&state, &mut poll, &state_dir).await;
        }
    });
}
```

- [ ] **Step 2: Declare the module and wire it into `main`**

In `crates/kamajid/src/lib.rs`, add near the other `pub mod` lines:

```rust
pub mod poll_task;
```

In `crates/kamajid/src/main.rs`, after building `state` and before binding/serving, spawn the task using the configured poll interval:

```rust
    let state = AppState::new(db, config);
    kamajid::poll_task::spawn_poll_task(state.clone(), state.config.poll_interval());
```

(`Config::poll_interval()` already exists in `kamaji-core` and returns a `Duration`, clamped to ≥1s.)

- [ ] **Step 3: Write the deterministic poll integration test**

Append to `crates/kamajid/tests/api.rs`:

```rust
#[tokio::test]
async fn poll_round_moves_idle_claude_ticket_to_review_and_emits() {
    use kamaji_core::poll::PollLoop;

    // A daemon whose marker dir is a temp dir we control.
    let state_dir = tempfile::tempdir().unwrap();
    let mut state =
        kamajid::state::AppState::new(Db::open_in_memory().unwrap(), kamaji_core::config::Config::default());
    state.set_state_dir(state_dir.path().to_path_buf());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = kamajid::router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{addr}");

    // An instrumented Claude ticket In Progress with a session — its idle signal
    // is a marker FILE, so detection works without zellij.
    let tid = state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
            db.set_ticket_session(t.id, "kamaji-1-t", "/wt", "kamaji-1-t")?;
            db.set_ticket_status(t.id, kamaji_core::models::Status::InProgress)?;
            db.set_ticket_instrumented(t.id, true)?;
            Ok(t.id)
        })
        .await
        .unwrap();

    // Connect SSE first so the move event is delivered.
    let mut stream = connect_events(&base).await;

    // Drive rounds deterministically (no interval timer):
    let mut poll = PollLoop::new();
    let sd = state_dir.path().to_path_buf();
    // Round 1: no marker → Active baseline, no move.
    kamajid::poll_task::poll_round(&state, &mut poll, &sd).await;
    // The agent "stops": its Stop hook creates the idle marker.
    let marker = kamaji_core::detect::marker_path(state_dir.path(), "kamaji-1-t");
    std::fs::write(&marker, "").unwrap();
    // Round 2: marker present → Idle → move to Review + emit.
    kamajid::poll_task::poll_round(&state, &mut poll, &sd).await;

    let (name, data) = read_named_event(&mut stream, "ticket.moved").await;
    assert_eq!(name, "ticket.moved");
    assert_eq!(data["id"], tid);
    assert_eq!(data["to"], "review");

    // The DB reflects the auto-move.
    let t: serde_json::Value = reqwest::get(format!("{base}/tickets/{tid}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(t["status"], "review");
}
```

- [ ] **Step 4: Build, test (run the poll test a few times for stability)**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test -p kamajid 2>&1 | grep -E 'test result:'
cargo test -p kamajid poll_round_moves_idle 2>&1 | grep -E 'test result:'
cargo test --all-targets --all-features 2>&1 | grep -E 'test result:'
```

Expected: `kamajid` now has 14 tests, all green. The poll test is deterministic (it drives `poll_round` directly rather than waiting on the interval), so it must pass every run.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(kamajid): auto-review poll background task

A periodic task gathers in-progress/review tickets, runs one
kamaji_core::poll::PollLoop tick, and broadcasts the resulting
ticket.moved/session.idle events — the daemon's headline autonomous
behaviour. Split into a deterministic poll_round (one tick + emit)
and a thin spawn_poll_task interval loop wired into main. The
integration test drives an idle Claude session via a marker file
(no zellij needed) and asserts the auto-move to Review over SSE.
Phase 1c step 4."
```

---

## Task 5: Ship

- [ ] **Step 1: Final full verification**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E '(test result:|Running )'
```

Expected: all green — kamaji 152, kamaji-core 97, kamajid 14.

- [ ] **Step 2: Manual smoke (the daemon starts a session — needs zellij; skip the spawn if absent)**

```bash
cargo build --release
DATA=$(mktemp -d); CFG=$(mktemp -d)
XDG_DATA_HOME=$DATA XDG_CONFIG_HOME=$CFG ./target/release/kamajid serve --bind 127.0.0.1:8802 >/tmp/kamajid-1c.log 2>&1 &
DPID=$!
for i in $(seq 1 25); do curl -sf http://127.0.0.1:8802/healthz >/dev/null 2>&1 && break; sleep 0.2; done
echo "healthz: $(curl -s http://127.0.0.1:8802/healthz)"
echo "--- log (poll task should be ticking quietly at info+) ---"; tail -3 /tmp/kamajid-1c.log
kill $DPID 2>/dev/null || true
```

Expected: healthz ok; the daemon log shows it listening; no panics from the poll task. (A full `/start` smoke needs a real git project + zellij and is out of scope for CI.)

- [ ] **Step 3: Push and open the PR**

```bash
git push -u origin "$(git branch --show-current)"
gh pr create --fill --base main
```

- [ ] **Step 4: Auto-merge with branch delete**

```bash
gh pr merge --squash --auto --delete-branch
```

Per the known worktree gotcha, the post-merge local cleanup may error from inside the worktree; the merge still lands. Wait for CI to go green (gate manually — CI isn't a required check), then verify `gh pr view --json state -q .state`. Once `MERGED`, clean up from `/home/victor/dev/kamaji`:

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

- **Spec coverage (Phase 1 spec §4):** `POST /tickets/:id/start` (T2), `POST /tickets/:id/done` (T3), the poll task → `ticket.moved`/`session.idle` (T4), `session.exited` on `/done` cleanup (T3). **Deferred to Plan 1d:** `POST /tickets/:id/attach` + `zellij web` management (§6), `session.exited` via background reconciliation of vanished sessions, and `PATCH /config`. Plus the carried 1b follow-ups (PATCH field coverage for prompt/agent; write-side 404 + update/delete SSE tests; FK enforcement decision).
- **Type consistency:** `AppState` API (`new`, `with_db`, `emit`, `config`, `tx`, `state_dir()`, `set_state_dir`, `db_handle`) consistent across routes/poll/tests. `session::cleanup_ticket(db, root_dir, state_dir, id)` and `poll_round(state, poll, state_dir)` / `spawn_poll_task(state, interval)` used identically where referenced.
- **No placeholders:** every code step is complete. (The `config_for` mis-step in Task 2 Step 3 is explicitly corrected in the same step to the cloned-config form.)
- **CI safety:** every new test avoids requiring a real `zellij` — `/start` tests stop at the precondition/prepare failure; `/done` cleanup uses a temp git repo; the poll test uses a Claude marker file. git IS available in CI (the existing `git.rs` tests rely on it).

## What this plan deliberately does NOT do (→ Plan 1d)

- `zellij web` lifecycle management + `POST /tickets/:id/attach` (the browser-attach path). Mostly un-CI-testable; grouped on its own.
- `session.exited` via background reconciliation of vanished sessions (a `reconcile` extraction). The `/done` cleanup already emits `session.exited`; the *autonomous* detection of an externally-killed session is 1d.
- `PATCH /config`, the `PATCH /tickets/:id` field expansion (prompt/agent), the extra write-side 404 + update/delete SSE tests, and the SQLite FK-enforcement decision — all carried 1b follow-ups.
- Daemon auto-spawn and the TUI-as-client flip (Phase 2).
