# Phase 1e — `kamajid` Daemon: Hardening & Completeness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close out the daemon's correctness and completeness gaps surfaced by the Phase 1b/1c reviews: autonomous `session.exited` detection of vanished sessions, a double-start guard, ticket-create project validation + SQLite FK enforcement, full-field `PATCH /tickets/:id`, and the carried test backlog.

**Architecture:** Builds on the Phase 1c daemon. Adds a `kamaji-core::session::reconcile` helper (detect sessions that vanished from `zellij list-sessions`, clear their DB columns + markers) wired into the daemon's poll task to emit `session.exited`; plus small route-level guards and a `db.update_ticket_full` for the expanded `PATCH`. Each change is independently CI-testable — `reconcile` takes the session list as a parameter so tests inject a crafted list (no real zellij needed).

**Tech Stack:** Rust 2021, the existing `kamajid` daemon + `kamaji-core` (`session`, `poll`, `db`, `zellij`). No new crates.

**Parent context:** the Phase 1 spec (`docs/superpowers/specs/2026-05-30-phase-1-kamajid-design.md`) and the carried follow-ups from the Plan 1b/1c reviews. This plan does NOT touch the `zellij web`/`/attach` subsystem (that is Plan 1d).

**Precondition:** Plans 1c **and** 1d merged. On `main`, `cargo test --all-targets --all-features` reports 152 (kamaji) + 97 (kamaji-core) + the kamajid tests (19 integration + 4 zellij_web unit after 1d) passing. (If 1d is not yet merged when this executes, the only interaction is `tests/api.rs` test counts; adjust the expected numbers accordingly — this plan's changes are orthogonal to 1d's.)

**Relevant existing API:**
- `kamaji_core::zellij::{list_sessions() -> Option<String>, session_in_list(list, name) -> bool}`.
- `kamaji_core::detect::marker_path(state_dir, session)`.
- `kamaji_core::db::Db` — `Db::open`, `Db::open_in_memory`, `get_ticket`, `get_project`, `list_projects`, `list_tickets`, `clear_ticket_session`, `update_ticket_fields(id, title, desc)`, `create_ticket`.
- `kamaji_core::events::Event::SessionExited { ticket_id, session_name }`.
- Daemon (Plan 1c): `AppState` with `with_db`, `emit`, `db_handle`, `state_dir`; `kamajid::poll_task::{poll_round, spawn_poll_task}` (poll_round runs the locked detection on the blocking pool and returns the mutated `PollLoop`); `kamajid::poll_task::all_tickets(db)` is private.
- `ApiError { NotFound, BadRequest(String), Internal(anyhow::Error) }`.

**Repo conventions (from `CLAUDE.md`):** all work on a branch in a worktree; never on `main`. Plan execution is always subagent-driven. Commit style mirrors history (`feat(core): …`, `feat(kamajid): …`). Ship with `gh pr create --fill --base main` → `gh pr merge --squash --auto --delete-branch`.

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
crates/kamaji-core/src/session.rs       MODIFIED — add pub fn reconcile
crates/kamaji-core/src/db.rs            MODIFIED — FK pragma + update_ticket_full
crates/kamajid/src/poll_task.rs         MODIFIED — reconcile_emit + wire into the loop; make all_tickets pub(crate)
crates/kamajid/src/routes/tickets.rs    MODIFIED — start double-start guard; create project-validate; patch full fields
crates/kamajid/tests/api.rs             MODIFIED — new tests
```

---

## Task 1: `kamaji-core::session::reconcile`

A pure-ish helper that, given the current `zellij list-sessions` output, finds tickets whose recorded session has vanished, clears their DB columns + idle markers, and returns the `(id, session_name)` pairs. The session list is a **parameter** so the function is fully unit-testable with a crafted string (no real zellij).

**Files:**
- Modify: `crates/kamaji-core/src/session.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/kamaji-core/src/session.rs`:

```rust
    #[test]
    fn reconcile_clears_vanished_sessions_and_returns_them() {
        let db = Db::open_in_memory().unwrap();
        let p = db.create_project("p", std::path::Path::new("/tmp/p"), None).unwrap();
        // t1 has a session that is STILL in the list; t2's session has vanished.
        let t1 = db.create_ticket(p.id, "a", "", None, Agent::Claude).unwrap();
        db.set_ticket_session(t1.id, "kamaji-1-a", "/wt1", "kamaji-1-a").unwrap();
        let t2 = db.create_ticket(p.id, "b", "", None, Agent::Claude).unwrap();
        db.set_ticket_session(t2.id, "kamaji-2-b", "/wt2", "kamaji-2-b").unwrap();

        let state_dir = tempfile::tempdir().unwrap();
        let marker2 = crate::detect::marker_path(state_dir.path(), "kamaji-2-b");
        std::fs::write(&marker2, "").unwrap();

        let tickets = db.list_tickets(p.id).unwrap();
        // The session list contains only t1's session — t2's has vanished.
        let sessions = "kamaji-1-a [Created 1h ago]\n";
        let vanished = reconcile(&db, &tickets, state_dir.path(), Some(sessions)).unwrap();

        assert_eq!(vanished, vec![(t2.id, "kamaji-2-b".to_string())]);
        // t2's columns cleared + marker removed; t1 untouched.
        assert_eq!(db.get_ticket(t2.id).unwrap().unwrap().session_name, None);
        assert!(!marker2.exists());
        assert_eq!(
            db.get_ticket(t1.id).unwrap().unwrap().session_name.as_deref(),
            Some("kamaji-1-a")
        );
    }

    #[test]
    fn reconcile_is_a_noop_when_session_list_is_unavailable() {
        let db = Db::open_in_memory().unwrap();
        let p = db.create_project("p", std::path::Path::new("/tmp/p"), None).unwrap();
        let t = db.create_ticket(p.id, "a", "", None, Agent::Claude).unwrap();
        db.set_ticket_session(t.id, "kamaji-1-a", "/wt", "kamaji-1-a").unwrap();
        let tickets = db.list_tickets(p.id).unwrap();
        let state_dir = tempfile::tempdir().unwrap();

        // `None` means zellij couldn't be queried — never clear anything.
        let vanished = reconcile(&db, &tickets, state_dir.path(), None).unwrap();
        assert!(vanished.is_empty());
        assert_eq!(
            db.get_ticket(t.id).unwrap().unwrap().session_name.as_deref(),
            Some("kamaji-1-a")
        );
    }
```

Run: `cargo test -p kamaji-core session::tests::reconcile_clears_vanished_sessions_and_returns_them`
Expected: FAIL — `reconcile` undefined (compile error).

- [ ] **Step 2: Implement `reconcile`**

Add this `pub fn` to `crates/kamaji-core/src/session.rs` (after `cleanup_ticket`, before the test module). Ensure `zellij` is imported at the top (Plan 1c added `use crate::zellij;`):

```rust
/// Reconcile recorded sessions against the live `zellij list-sessions` output.
/// For every ticket whose `session_name` is NOT present in `sessions`, clear its
/// session columns and remove its idle marker, and collect `(id, name)`. When
/// `sessions` is `None` (zellij couldn't be queried) this is a no-op, so a
/// transient failure never wipes valid state. The session list is a parameter
/// (not fetched here) so callers control it and tests can inject a crafted list.
pub fn reconcile(
    db: &Db,
    tickets: &[Ticket],
    state_dir: &Path,
    sessions: Option<&str>,
) -> Result<Vec<(i64, String)>> {
    let Some(list) = sessions else {
        return Ok(Vec::new());
    };
    let vanished: Vec<(i64, String)> = tickets
        .iter()
        .filter_map(|t| {
            t.session_name
                .as_deref()
                .filter(|n| !zellij::session_in_list(list, n))
                .map(|n| (t.id, n.to_string()))
        })
        .collect();
    for (id, name) in &vanished {
        db.clear_ticket_session(*id)?;
        let _ = std::fs::remove_file(detect::marker_path(state_dir, name));
    }
    Ok(vanished)
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p kamaji-core session::tests::reconcile_clears_vanished_sessions_and_returns_them session::tests::reconcile_is_a_noop_when_session_list_is_unavailable`
Expected: PASS — both.

- [ ] **Step 4: Full verification**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E 'test result:'
```

Expected: all green; kamaji-core gains 2 tests.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(core): add session::reconcile

Given a zellij list-sessions snapshot, clears the session columns +
idle markers of tickets whose recorded session has vanished and
returns the (id, name) pairs. None (zellij unreachable) is a no-op.
The session list is a parameter so it's fully unit-testable without
zellij. The daemon's poll task will emit session.exited from this.
Phase 1e step 1."
```

---

## Task 2: Daemon emits `session.exited` — wire `reconcile` into the poll task

Add `reconcile_emit` (run reconcile + broadcast `session.exited` per vanished session) and call it each poll round with the live `zellij list-sessions`. `reconcile_emit` takes the session list as a parameter so an integration test injects a crafted list (CI has no zellij).

**Files:**
- Modify: `crates/kamajid/src/poll_task.rs`, `crates/kamajid/tests/api.rs`

- [ ] **Step 1: Make `all_tickets` reachable + add `reconcile_emit`**

In `crates/kamajid/src/poll_task.rs`, change `fn all_tickets` to `pub(crate) fn all_tickets` (so `reconcile_emit` and tests can reuse it). Then add:

```rust
use kamaji_core::session;

/// Reconcile recorded sessions against `sessions` (a `zellij list-sessions`
/// snapshot, or `None` when zellij is unreachable) and broadcast `session.exited`
/// for each ticket whose session vanished. The DB work runs on the blocking pool
/// (mirrors `poll_round`); `sessions` is a parameter so tests can inject a list.
pub async fn reconcile_emit(state: &AppState, state_dir: &Path, sessions: Option<String>) {
    let task_state = state.clone();
    let state_dir = state_dir.to_path_buf();
    let vanished = tokio::task::spawn_blocking(move || {
        let db = task_state.db_handle();
        let db = db.lock().expect("db mutex poisoned");
        let tickets = all_tickets(&db);
        session::reconcile(&db, &tickets, &state_dir, sessions.as_deref()).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    for (id, name) in vanished {
        state.emit(kamaji_core::events::Event::SessionExited {
            ticket_id: id,
            session_name: name,
        });
    }
}
```

- [ ] **Step 2: Call it from the poll loop**

In `spawn_poll_task`'s loop body (in the same file), after the `poll_round` call, add a reconcile pass using the live session list. The blocking `zellij::list_sessions()` is fetched inside a `spawn_blocking` to keep it off the async worker:

```rust
        loop {
            ticker.tick().await;
            poll = poll_round(&state, poll, &state_dir).await;
            let sessions = tokio::task::spawn_blocking(kamaji_core::zellij::list_sessions)
                .await
                .unwrap_or(None);
            reconcile_emit(&state, &state_dir, sessions).await;
        }
```

- [ ] **Step 3: Write the integration test (injected session list)**

Append to `crates/kamajid/tests/api.rs`:

```rust
#[tokio::test]
async fn reconcile_emit_clears_vanished_session_and_emits_session_exited() {
    let state_dir = tempfile::tempdir().unwrap();
    let mut state = kamajid::state::AppState::new(
        Db::open_in_memory().unwrap(),
        kamaji_core::config::Config::default(),
    );
    state.set_state_dir(state_dir.path().to_path_buf());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = kamajid::router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{addr}");

    let tid = state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
            db.set_ticket_session(t.id, "kamaji-1-t", "/wt", "kamaji-1-t")?;
            db.set_ticket_status(t.id, kamaji_core::models::Status::InProgress)?;
            Ok(t.id)
        })
        .await
        .unwrap();

    // Subscribe first, then run reconcile with a session list that does NOT
    // contain this ticket's session (it vanished).
    let mut stream = connect_events(&base).await;
    let sd = state_dir.path().to_path_buf();
    kamajid::poll_task::reconcile_emit(&state, &sd, Some("some-other-session\n".to_string())).await;

    let (name, data) = read_named_event(&mut stream, "session.exited").await;
    assert_eq!(name, "session.exited");
    assert_eq!(data["ticket_id"], tid);
    assert_eq!(data["session_name"], "kamaji-1-t");

    // The DB session columns were cleared.
    let t: serde_json::Value = reqwest::get(format!("{base}/tickets/{tid}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
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

Expected: the `tests/api.rs` binary gains 1 test; whole suite green. (Run the new test a couple of times — it uses the race-free `connect_events`-before-command pattern, so it must be deterministic.)

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(kamajid): emit session.exited via poll-task reconcile

Each poll round now reconciles recorded sessions against a live
zellij list-sessions snapshot (fetched on the blocking pool) and
broadcasts session.exited for any whose session vanished, clearing
its DB columns. reconcile_emit takes the session list as a parameter
so the integration test injects a crafted list (no zellij in CI).
Completes the daemon's autonomous session-lifecycle events. Phase 1e
step 2."
```

---

## Task 3: Correctness guards — `/start` double-start + ticket-create project validation + FK enforcement

Three small correctness fixes: refuse to start a ticket that already has a session; reject creating a ticket under a non-existent project with a clean 400; and enable SQLite foreign-key enforcement as defense-in-depth.

**Files:**
- Modify: `crates/kamaji-core/src/db.rs`, `crates/kamajid/src/routes/tickets.rs`, `crates/kamajid/tests/api.rs`

- [ ] **Step 1: Enable FK enforcement in `Db`**

In `crates/kamaji-core/src/db.rs`, in BOTH `open` and `open_in_memory`, enable foreign-key enforcement right after the connection is created. Use `execute_batch` with the raw `PRAGMA` (the keyword form `= ON` is unambiguous; binding a string value can get quoted to `= 'ON'`, which SQLite rejects). In `open` (after `let conn = Connection::open(path)?;`, alongside the WAL pragma):

```rust
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
```

In `open_in_memory` (after `let conn = Connection::open_in_memory()?;`):

```rust
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
```

Add a test to `db.rs`'s test module:

```rust
    #[test]
    fn foreign_keys_are_enforced() {
        let db = db();
        // Inserting a ticket under a non-existent project must fail (FK ON).
        let err = db.create_ticket(9999, "t", "", None, Agent::Claude);
        assert!(err.is_err(), "FK enforcement should reject a bad project_id");
    }
```

Run: `cargo test -p kamaji-core db::tests` — expected: PASS (incl. the new one). **If any pre-existing `kamaji` or `kamaji-core` test now fails**, it relied on FK-off behavior (e.g. created a ticket under a project id it never inserted) — fix that test's setup to insert a real project first; do NOT disable the pragma.

- [ ] **Step 2: Validate the project on ticket create (clean 400)**

In `crates/kamajid/src/routes/tickets.rs`, update the `create` handler's `with_db` closure to check the project exists first, returning a typed not-found that maps to 400 (so the client gets a clear message rather than a 500 from a raw FK error). Replace the closure body:

```rust
    let ticket = state
        .with_db(move |db| {
            if db.get_project(body.project_id)?.is_none() {
                // Signal "bad project" distinctly from a real DB error.
                return Ok(Err(format!("no such project: {}", body.project_id)));
            }
            let t = db.create_ticket(
                body.project_id,
                &body.title,
                &body.description,
                body.initial_prompt.as_deref(),
                body.agent,
            )?;
            Ok(Ok(t))
        })
        .await?;
    let ticket = match ticket {
        Ok(t) => t,
        Err(msg) => return Err(ApiError::BadRequest(msg)),
    };
    state.emit(Event::TicketCreated(ticket.clone()));
    Ok((StatusCode::CREATED, Json(ticket)))
```

(The closure now returns `anyhow::Result<Result<Ticket, String>>` — outer `Err` = real DB error → 500; inner `Err(String)` = bad project → 400. Same inner-Result pattern used by `/start`.)

- [ ] **Step 3: Add the `/start` double-start guard**

In the `start` handler (added in Plan 1c), after fetching the ticket and before preparing the session, refuse if a session already exists. Find the line `let original_status = ticket.status;` and insert before it:

```rust
    if ticket.session_name.is_some() {
        return Err(ApiError::BadRequest(
            "ticket already has a session; stop it first".into(),
        ));
    }
```

(This prevents overwriting a live session's DB columns and emitting a duplicate `session.started`.)

- [ ] **Step 4: Write the tests**

Append to `crates/kamajid/tests/api.rs`:

```rust
#[tokio::test]
async fn create_ticket_under_missing_project_is_400() {
    let (base, _state) = spawn().await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/tickets"))
        .json(&serde_json::json!({ "project_id": 4242, "title": "t", "agent": "claude" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["kind"], "bad_request");
}

#[tokio::test]
async fn start_on_already_started_ticket_is_400() {
    let (base, state) = spawn().await;
    let tid = state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
            // Already has a session.
            db.set_ticket_session(t.id, "kamaji-1-t", "/wt", "kamaji-1-t")?;
            db.set_ticket_status(t.id, kamaji_core::models::Status::InProgress)?;
            Ok(t.id)
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
}
```

- [ ] **Step 5: Build, test**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E 'test result:'
```

Expected: all green. kamaji-core gains 1 test (FK); kamajid `tests/api.rs` gains 2. If any pre-existing test broke from the FK pragma, fix its setup (per Step 1's note).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: ticket-create + start correctness guards + FK enforcement

Enable SQLite foreign_keys in Db::open/open_in_memory (defense in
depth). POST /tickets validates the project exists -> clean 400
(not a raw FK 500). POST /tickets/:id/start refuses a ticket that
already has a session -> 400 (no clobbering a live session / no
duplicate session.started). Phase 1e step 3."
```

---

## Task 4: Full-field `PATCH /tickets/:id` + the carried test backlog

Expand `PATCH /tickets/:id` to cover all editable fields (title, description, initial_prompt, agent — matching the spec's field list), backed by a new `db.update_ticket_full`. Then fill the carried test gaps: write-side 404s and SSE coverage for `ticket.updated`/`ticket.deleted`.

**Files:**
- Modify: `crates/kamaji-core/src/db.rs`, `crates/kamajid/src/routes/tickets.rs`, `crates/kamajid/tests/api.rs`

- [ ] **Step 1: Add `db.update_ticket_full`**

In `crates/kamaji-core/src/db.rs`, add (near `update_ticket_fields`):

```rust
    /// Edit all caller-editable ticket fields at once (full replace): title,
    /// description, initial prompt, and agent.
    pub fn update_ticket_full(
        &self,
        id: i64,
        title: &str,
        description: &str,
        initial_prompt: Option<&str>,
        agent: Agent,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE tickets SET title = ?2, description = ?3, initial_prompt = ?4, agent = ?5,
             updated_at = datetime('now') WHERE id = ?1",
            params![id, title, description, initial_prompt, agent.as_str()],
        )?;
        Ok(())
    }
```

Add a test to `db.rs`'s test module:

```rust
    #[test]
    fn update_ticket_full_replaces_all_fields() {
        let db = db();
        let p = db.create_project("p", &PathBuf::from("/tmp/p"), None).unwrap();
        let t = db.create_ticket(p.id, "t", "d", Some("p1"), Agent::Claude).unwrap();
        db.update_ticket_full(t.id, "t2", "d2", Some("p2"), Agent::Codex).unwrap();
        let got = db.get_ticket(t.id).unwrap().unwrap();
        assert_eq!(got.title, "t2");
        assert_eq!(got.description, "d2");
        assert_eq!(got.initial_prompt.as_deref(), Some("p2"));
        assert_eq!(got.agent, Agent::Codex);
    }
```

- [ ] **Step 2: Expand the `UpdateTicket` DTO + handler**

In `crates/kamajid/src/routes/tickets.rs`, replace the `UpdateTicket` struct and the `update` handler:

```rust
#[derive(Deserialize)]
pub struct UpdateTicket {
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Replaces the initial prompt when present; kept unchanged when omitted.
    #[serde(default)]
    pub initial_prompt: Option<String>,
    /// Replaces the agent when present; kept unchanged when omitted.
    #[serde(default)]
    pub agent: Option<Agent>,
}

/// `PATCH /tickets/:id` → edit title/description (full-replace) and, when
/// provided, initial_prompt/agent (omitted fields keep their current value).
/// 404 if missing. Emits `ticket.updated`.
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateTicket>,
) -> Result<Json<Ticket>, ApiError> {
    if body.title.trim().is_empty() {
        return Err(ApiError::BadRequest("title must not be empty".into()));
    }
    let ticket = state
        .with_db(move |db| {
            let Some(current) = db.get_ticket(id)? else {
                return Ok(None);
            };
            // agent / initial_prompt keep their current value when omitted.
            let agent = body.agent.unwrap_or(current.agent);
            let prompt = match &body.initial_prompt {
                Some(p) => Some(p.as_str()),
                None => current.initial_prompt.as_deref(),
            };
            db.update_ticket_full(id, &body.title, &body.description, prompt, agent)?;
            db.get_ticket(id)
        })
        .await?
        .ok_or(ApiError::NotFound)?;
    state.emit(Event::TicketUpdated(ticket.clone()));
    Ok(Json(ticket))
}
```

Because `agent`/`initial_prompt` are optional and kept-when-omitted, the existing 1b lifecycle test (which PATCHes only `{title, description}`) keeps passing **without modification** — no `agent` is required.

- [ ] **Step 3: Add the PATCH-field, write-side-404, and SSE update/delete tests**

Append to `crates/kamajid/tests/api.rs`:

```rust
#[tokio::test]
async fn patch_ticket_updates_all_fields() {
    let (base, state) = spawn().await;
    let tid = state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            let t = db.create_ticket(p.id, "t", "d", Some("p1"), kamaji_core::models::Agent::Claude)?;
            Ok(t.id)
        })
        .await
        .unwrap();

    let edited: serde_json::Value = reqwest::Client::new()
        .patch(format!("{base}/tickets/{tid}"))
        .json(&serde_json::json!({
            "title": "t2", "description": "d2", "initial_prompt": "p2", "agent": "codex"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(edited["title"], "t2");
    assert_eq!(edited["initial_prompt"], "p2");
    assert_eq!(edited["agent"], "codex");
}

#[tokio::test]
async fn patch_and_delete_missing_ticket_are_404() {
    let (base, _state) = spawn().await;
    let client = reqwest::Client::new();

    let patch = client
        .patch(format!("{base}/tickets/777"))
        .json(&serde_json::json!({ "title": "x", "agent": "claude" }))
        .send()
        .await
        .unwrap();
    assert_eq!(patch.status(), 404);

    let del = client
        .delete(format!("{base}/tickets/777"))
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 404);
}

#[tokio::test]
async fn editing_and_deleting_emit_sse_events() {
    let (base, state) = spawn().await;
    let tid = state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
            Ok(t.id)
        })
        .await
        .unwrap();

    // ticket.updated
    let mut stream = connect_events(&base).await;
    reqwest::Client::new()
        .patch(format!("{base}/tickets/{tid}"))
        .json(&serde_json::json!({ "title": "edited", "agent": "claude" }))
        .send()
        .await
        .unwrap();
    let (n1, d1) = read_named_event(&mut stream, "ticket.updated").await;
    assert_eq!(n1, "ticket.updated");
    assert_eq!(d1["title"], "edited");

    // ticket.deleted
    let mut stream2 = connect_events(&base).await;
    reqwest::Client::new()
        .delete(format!("{base}/tickets/{tid}"))
        .send()
        .await
        .unwrap();
    let (n2, d2) = read_named_event(&mut stream2, "ticket.deleted").await;
    assert_eq!(n2, "ticket.deleted");
    assert_eq!(d2["id"], tid);
}
```

- [ ] **Step 4: Build, test**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E 'test result:'
```

Expected: all green. kamaji-core gains 1 test (`update_ticket_full`); kamajid `tests/api.rs` gains 3. (Run the SSE test a couple of times for determinism.) The existing `create_edit_move_delete_ticket_lifecycle` test (1b) PATCHes only `{title, description}`; because `agent`/`initial_prompt` are optional and kept-when-omitted, it keeps passing unchanged — no edit needed.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: full-field PATCH /tickets/:id + carried test backlog

PATCH now full-replaces title/description/initial_prompt/agent
(matching the spec field list) via a new db.update_ticket_full.
Adds the carried test coverage: write-side 404s (PATCH/DELETE on a
missing ticket) and SSE end-to-end for ticket.updated / ticket.deleted.
Phase 1e step 4."
```

---

## Task 5: Ship

- [ ] **Step 1: Final full verification**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E '(test result:|Running )'
```

Expected: all green across the workspace.

- [ ] **Step 2: Smoke**

```bash
cargo build --release
DATA=$(mktemp -d); CFG=$(mktemp -d)
XDG_DATA_HOME=$DATA XDG_CONFIG_HOME=$CFG ./target/release/kamajid serve --bind 127.0.0.1:8805 >/tmp/kamajid-1e.log 2>&1 &
DPID=$!
for i in $(seq 1 25); do curl -sf http://127.0.0.1:8805/healthz >/dev/null 2>&1 && break; sleep 0.2; done
echo "healthz: $(curl -s http://127.0.0.1:8805/healthz)"
echo "bad-project create (expect 400): $(curl -s -o /dev/null -w '%{http_code}' -X POST http://127.0.0.1:8805/tickets -H 'content-type: application/json' -d '{"project_id":99,"title":"t","agent":"claude"}')"
kill $DPID 2>/dev/null || true
```

Expected: healthz ok; the bad-project create prints `400`.

- [ ] **Step 3: Push and open the PR**

```bash
git push -u origin "$(git branch --show-current)"
gh pr create --fill --base main
```

- [ ] **Step 4: Auto-merge with branch delete**

```bash
gh pr merge --squash --auto --delete-branch
```

Per the known worktree gotcha, the post-merge local cleanup may error from inside the worktree; the merge still lands. Wait for CI green (gate manually), verify `gh pr view --json state -q .state`, then clean up from `/home/victor/dev/kamaji`:

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

- **Coverage of the carried follow-ups:** autonomous `session.exited` reconciliation (T1 core + T2 daemon); SQLite FK enforcement (T3); ticket-create project validation → 400 (T3); `/start` double-start guard → 400 (T3); `PATCH /tickets/:id` field expansion to prompt/agent (T4); write-side 404 + `ticket.updated`/`deleted` SSE tests (T4). **Deferred (see below):** `PATCH /config`.
- **Type consistency:** `session::reconcile(db, tickets, state_dir, sessions)` and `poll_task::reconcile_emit(state, state_dir, sessions)` and `db.update_ticket_full(id, title, desc, prompt, agent)` used identically across tasks. The expanded `UpdateTicket` DTO (required `title`+`agent`, defaulted `description`+`initial_prompt`) is consistent between the handler and the tests.
- **No placeholders:** every code step is complete.
- **CI safety:** `reconcile` and `reconcile_emit` take the session list as a parameter, so the daemon emission is tested with a crafted list — no real zellij. The FK pragma + all route changes are pure DB/HTTP, fully CI-testable. **Watch item:** the FK pragma (T3) and the now-required `agent` on PATCH (T4) can break pre-existing tests whose setup assumed otherwise — both tasks call this out explicitly and instruct the fix (insert a real project; add `"agent"` to the lifecycle test's PATCH body).

## What this plan deliberately does NOT do (→ later / backlog)

- **`PATCH /config`.** Mutating the daemon's config at runtime needs a mutable shared `Config` (e.g. `Arc<RwLock<Config>>`), which ripples through every reader (`/start`, the config route, the poll task). No Phase-1 surface edits config, so this is deferred until a config-editing UI needs it — at which point the `Arc<Config>` → `Arc<RwLock<Config>>` change is its own focused task.
- **`zellij web` kill-on-shutdown** + token-cache 401-invalidation (Plan 1d's deferred niceties).
- **Daemon auto-spawn and the TUI-as-client flip** (Phase 2).
