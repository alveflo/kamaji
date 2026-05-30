# Phase 1d — `kamajid` Daemon: `zellij web` Management + Browser Attach Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the daemon hand a client everything it needs to open a ticket's agent session in a browser: a `POST /tickets/:id/attach` route that ensures `zellij web` is running, manages its auth token, and returns an `AttachInfo { session_name, web_url, token }`.

**Architecture:** A new `kamajid::zellij_web::ZellijWeb` owns the `zellij web` server lifecycle (lazy-spawn on first attach), a cached auth token, and the URL/token assembly. It's a daemon concern — `kamaji-core` knows nothing about it. `ZellijWeb` is a concrete type with a real constructor (`new`) and a test constructor (`fake`) that returns canned `AttachInfo` without spawning anything, so the `/attach` route is fully integration-testable in CI; the genuine `zellij web` spawn is exercised by one `#[ignore]`d test plus a manual smoke. `AppState` holds an `Arc<ZellijWeb>`, defaulting to the real one, with a setter so tests inject the fake.

**Tech Stack:** Rust 2021, `axum` 0.7, `tokio` (incl. `process` for spawning `zellij web`), the existing `kamajid` daemon + `kamaji-core::slug`. No new crates.

**Parent spec:** `docs/superpowers/specs/2026-05-30-phase-1-kamajid-design.md` §6 (`zellij web` management) and §4 (the `/tickets/:id/attach` route). This plan implements that one subsystem.

**Precondition:** Plan 1c merged. On `main`, `cargo test --all-targets --all-features` reports 152 (kamaji) + 97 (kamaji-core) + 16 (kamajid) = 265 passing.

**Relevant existing API (from Plan 1b/1c):**
- `AppState { db (private Arc<Mutex<Db>>), pub config: Arc<Config>, pub tx, state_dir: Arc<PathBuf> }` with `new(db, config)`, `with_db`, `emit`, `state_dir()`, `set_state_dir`, `db_handle`.
- `ApiError { NotFound, BadRequest(String), Internal(anyhow::Error) }` (derives Debug), `IntoResponse` → `{error, kind}` + status.
- `crates/kamajid/tests/api.rs` has the `spawn() -> (String, AppState)` harness.
- `kamaji_core::slug::ticket_name(id, title)` builds a ticket's session name (e.g. `kamaji-7-add-login`) — but the **recorded** `session_name` on a started ticket is the authoritative value; attach uses the DB column, not a re-derivation.

**Repo conventions (from `CLAUDE.md`):** all work on a branch in a worktree (the executing skill sets this up), never on `main`. Commit style mirrors history (`feat(kamajid): …`). Ship with `gh pr create --fill --base main` → `gh pr merge --squash --auto --delete-branch` (the `--delete-branch` errors from inside a worktree but the merge still lands — verify + clean up manually).

---

## Verification commands (run at every checkpoint)

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E '(test result:|Running )'
```

The live `zellij web` test is marked `#[ignore]`, so it does NOT run in the default `cargo test` (and thus not in CI, which has no `zellij`). Run it manually with `cargo test -p kamajid -- --ignored` on a machine that has `zellij` ≥ 0.43.

---

## File Structure (after this plan)

```
crates/kamajid/src/zellij_web.rs       NEW — AttachInfo, web_url(), ZellijWeb (real + fake)
crates/kamajid/src/state.rs            MODIFIED — hold Arc<ZellijWeb> + set_zellij_web()
crates/kamajid/src/routes/tickets.rs   MODIFIED — add `attach` handler
crates/kamajid/src/lib.rs              MODIFIED — pub mod zellij_web; mount /attach
crates/kamajid/tests/api.rs            MODIFIED — attach tests (fake provider) + #[ignore] live test
```

---

## Task 1: `AttachInfo` + the `web_url` builder (pure, fully testable)

The serializable response shape and the pure URL-assembly function. No subprocess yet — this task is all unit-testable.

**Files:**
- Create: `crates/kamajid/src/zellij_web.rs`
- Modify: `crates/kamajid/src/lib.rs` (declare `pub mod zellij_web;`)

- [ ] **Step 1: Write the failing unit test**

Create `crates/kamajid/src/zellij_web.rs` with the doc comment + the test module first:

```rust
//! `zellij web` lifecycle + browser-attach info. Owns the optional `zellij web`
//! subprocess (lazy-spawned on first attach), a cached auth token, and the
//! assembly of the per-session attach URL. A daemon concern — `kamaji-core`
//! knows nothing about `zellij web`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_url_joins_base_and_session() {
        assert_eq!(
            web_url("http://127.0.0.1:8082", "kamaji-7-add-login"),
            "http://127.0.0.1:8082/kamaji-7-add-login"
        );
        // A trailing slash on the base must not double up.
        assert_eq!(
            web_url("http://127.0.0.1:8082/", "s"),
            "http://127.0.0.1:8082/s"
        );
    }

    #[test]
    fn fake_attach_info_returns_canned_token_and_url() {
        let zw = ZellijWeb::fake("test-token");
        let info = zw.attach_info("kamaji-1-x").unwrap();
        assert_eq!(info.session_name, "kamaji-1-x");
        assert_eq!(info.web_url, "http://127.0.0.1:8082/kamaji-1-x");
        assert_eq!(info.token, "test-token");
    }
}
```

- [ ] **Step 2: Declare the module so the test compiles**

In `crates/kamajid/src/lib.rs`, add `pub mod zellij_web;` near the other `pub mod` lines (alphabetical: after `pub mod state;` or wherever fits the existing order).

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p kamajid zellij_web::tests::web_url_joins_base_and_session`
Expected: FAIL — `web_url`/`ZellijWeb` undefined (compile error).

- [ ] **Step 4: Write `AttachInfo`, `web_url`, and the `ZellijWeb` skeleton (fake path only)**

Prepend to `crates/kamajid/src/zellij_web.rs` (above the test module):

```rust
use std::sync::Mutex;

use serde::Serialize;

/// The default base URL `zellij web` serves on (spec §6).
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8082";

/// What a client needs to attach to a ticket's session in the browser.
#[derive(Debug, Clone, Serialize)]
pub struct AttachInfo {
    pub session_name: String,
    /// `<base>/<session>` — the browser opens/iframes this; `zellij web` creates,
    /// attaches, or resurrects the named session.
    pub web_url: String,
    /// The `zellij web` login token (consumed by the login page).
    pub token: String,
}

/// Build the per-session attach URL, tolerating a trailing slash on the base.
pub fn web_url(base: &str, session_name: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), session_name)
}

/// Manages the `zellij web` server + its auth token. `new()` is the real,
/// lazy-spawning manager; `fake()` returns canned attach info without touching
/// any subprocess (for tests and CI, which have no `zellij`).
pub struct ZellijWeb {
    base_url: String,
    /// Cached login token (created lazily via `zellij web --create-token`).
    token: Mutex<Option<String>>,
    /// In `fake` mode this token is returned directly and no subprocess runs.
    fake_token: Option<String>,
}

impl ZellijWeb {
    /// The real manager: lazily spawns `zellij web` and creates a token on the
    /// first `attach_info` call.
    pub fn new() -> Self {
        ZellijWeb {
            base_url: DEFAULT_BASE_URL.to_string(),
            token: Mutex::new(None),
            fake_token: None,
        }
    }

    /// A test double: every `attach_info` returns `token` and the assembled URL,
    /// with no `zellij` subprocess. Used by integration tests and CI.
    pub fn fake(token: &str) -> Self {
        ZellijWeb {
            base_url: DEFAULT_BASE_URL.to_string(),
            token: Mutex::new(Some(token.to_string())),
            fake_token: Some(token.to_string()),
        }
    }

    /// Ensure `zellij web` is running with a valid token and return the attach
    /// info for `session_name`. In `fake` mode this is pure; in real mode it may
    /// spawn the server and create a token (see [`Self::ensure_running`]).
    pub fn attach_info(&self, session_name: &str) -> anyhow::Result<AttachInfo> {
        let token = if let Some(t) = &self.fake_token {
            t.clone()
        } else {
            self.ensure_running()?
        };
        Ok(AttachInfo {
            session_name: session_name.to_string(),
            web_url: web_url(&self.base_url, session_name),
            token,
        })
    }
}

impl Default for ZellijWeb {
    fn default() -> Self {
        Self::new()
    }
}
```

This references `ensure_running`, which Task 2 implements. To keep this task compiling and green on its own, add a temporary minimal `ensure_running` that errors (it's only reachable in real mode, which no test in this task exercises):

```rust
impl ZellijWeb {
    fn ensure_running(&self) -> anyhow::Result<String> {
        anyhow::bail!("zellij web management not yet implemented")
    }
}
```

(Task 2 replaces this body with the real subprocess logic. The fake path — the only one tested here and in the route tests — never calls it.)

- [ ] **Step 5: Run the tests**

Run: `cargo test -p kamajid zellij_web::tests`
Expected: PASS — both unit tests (`web_url_joins_base_and_session`, `fake_attach_info_returns_canned_token_and_url`).

- [ ] **Step 6: Full verification**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E 'test result:'
```

Expected: all green; kamajid gains 2 unit tests (now 18 across the crate, counting the 16 integration tests in `tests/api.rs` plus these 2 in-crate unit tests — they're reported as separate test binaries).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(kamajid): AttachInfo + web_url + ZellijWeb skeleton

The serializable attach response, the pure per-session URL builder,
and a ZellijWeb with a real (lazy) constructor and a fake() test
double that returns canned attach info with no subprocess. The real
ensure_running is stubbed; Task 2 implements it. Phase 1d step 1."
```

---

## Task 2: Real `ensure_running` — spawn `zellij web` + manage the token

Replace the stub with the genuine lifecycle: probe the server, spawn it if absent, create/cache the token. This path needs a real `zellij` binary, so it's covered by one `#[ignore]`d test plus the manual smoke — never run in CI.

**Files:**
- Modify: `crates/kamajid/src/zellij_web.rs`

- [ ] **Step 1: Replace the stub `ensure_running` with the real implementation**

In `crates/kamajid/src/zellij_web.rs`, replace the temporary `ensure_running` body with:

```rust
impl ZellijWeb {
    /// Ensure the `zellij web` server is reachable and we hold a login token.
    /// Returns the token. Steps (spec §6): (1) create + cache a token if we have
    /// none, (2) probe the server's port; if unreachable, spawn `zellij web` and
    /// poll until it answers (≤3s). Best-effort: a spawn we don't own (server
    /// already running) is fine — we just reuse it.
    fn ensure_running(&self) -> anyhow::Result<String> {
        // (1) Token: create once, then cache. Tokens persist in zellij's own
        // store across server restarts, so a cached one keeps working.
        let token = {
            let mut guard = self.token.lock().expect("token mutex poisoned");
            if guard.is_none() {
                *guard = Some(create_token()?);
            }
            guard.clone().expect("token just set")
        };

        // (2) Ensure the server is up.
        if !port_reachable(&self.base_url) {
            spawn_zellij_web()?;
            wait_until_reachable(&self.base_url, std::time::Duration::from_secs(3))?;
        }
        Ok(token)
    }
}

/// Run `zellij web --create-token` once and parse the printed token.
///
/// NOTE: the exact stdout format must be verified against the installed
/// `zellij` during implementation (the spec flags this as a spike). The current
/// parse takes the last non-empty whitespace-delimited token of stdout, which
/// matches `zellij`'s "Token: <value>" / bare-token styles. The `#[ignore]`d
/// live test and the manual smoke are how this is validated.
fn create_token() -> anyhow::Result<String> {
    let out = std::process::Command::new("zellij")
        .args(["web", "--create-token"])
        .output()?;
    if !out.status.success() {
        anyhow::bail!(
            "zellij web --create-token failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let token = stdout
        .split_whitespace()
        .last()
        .map(str::to_string)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| anyhow::anyhow!("could not parse a token from: {stdout:?}"))?;
    Ok(token)
}

/// Spawn a detached `zellij web` server. We do not hold the child (the spec
/// accepts that the server outlives the daemon and is reused on next start).
fn spawn_zellij_web() -> anyhow::Result<()> {
    std::process::Command::new("zellij")
        .arg("web")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

/// True if the host:port of `base_url` accepts a TCP connection right now.
fn port_reachable(base_url: &str) -> bool {
    if let Some(addr) = base_url_to_socket_addr(base_url) {
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok()
    } else {
        false
    }
}

/// Poll the port until reachable or the deadline elapses.
fn wait_until_reachable(base_url: &str, timeout: std::time::Duration) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if port_reachable(base_url) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    anyhow::bail!("zellij web did not become reachable within {timeout:?}")
}

/// Parse the `host:port` out of a `http://host:port` base URL into a SocketAddr.
fn base_url_to_socket_addr(base_url: &str) -> Option<std::net::SocketAddr> {
    let hostport = base_url.strip_prefix("http://").or_else(|| base_url.strip_prefix("https://"))?;
    let hostport = hostport.trim_end_matches('/');
    use std::net::ToSocketAddrs;
    hostport.to_socket_addrs().ok()?.next()
}
```

- [ ] **Step 2: Add unit tests for the pure helpers, and an `#[ignore]`d live test**

Append to the `#[cfg(test)] mod tests` in `crates/kamajid/src/zellij_web.rs`:

```rust
    #[test]
    fn base_url_parses_to_socket_addr() {
        let addr = base_url_to_socket_addr("http://127.0.0.1:8082").unwrap();
        assert_eq!(addr.port(), 8082);
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        // Trailing slash tolerated.
        assert!(base_url_to_socket_addr("http://127.0.0.1:8082/").is_some());
        // Garbage → None.
        assert!(base_url_to_socket_addr("not a url").is_none());
    }

    #[test]
    fn unreachable_port_is_false() {
        // Port 1 on localhost is not listening in any sane test environment.
        assert!(!port_reachable("http://127.0.0.1:1"));
    }

    /// Live test: requires a real `zellij` ≥ 0.43 on PATH. Not run in CI.
    /// `cargo test -p kamajid -- --ignored zellij_web_real_attach_info`
    #[test]
    #[ignore = "requires a real zellij binary; run manually with --ignored"]
    fn zellij_web_real_attach_info() {
        let zw = ZellijWeb::new();
        let info = zw.attach_info("kamaji-smoke-test").unwrap();
        assert_eq!(info.session_name, "kamaji-smoke-test");
        assert_eq!(info.web_url, "http://127.0.0.1:8082/kamaji-smoke-test");
        assert!(!info.token.is_empty(), "real attach must yield a non-empty token");
    }
```

- [ ] **Step 3: Run the (non-ignored) tests**

```bash
cargo test -p kamajid zellij_web::tests 2>&1 | grep -E 'test result:'
```
Expected: PASS — the 4 non-ignored unit tests (`web_url…`, `fake_attach_info…`, `base_url_parses…`, `unreachable_port…`); the live test is reported as `ignored`.

- [ ] **Step 4: Full verification**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E 'test result:'
```

Expected: all green; the zellij_web unit-test binary now reports `4 passed; … 1 ignored`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(kamajid): real zellij web ensure_running

Token create+cache (zellij web --create-token), TCP port probe, and
lazy spawn of `zellij web` with a readiness poll (<=3s). The live path
is covered by an #[ignore]d test (needs a real zellij) plus the pure
helpers (URL/addr parsing, port probe) which are unit-tested in CI.
Phase 1d step 2."
```

---

## Task 3: `AppState` holds `ZellijWeb`; add `POST /tickets/:id/attach`

Wire the manager into the daemon and add the route. The route looks up the ticket's recorded session, asks `ZellijWeb` for attach info, and returns it. Integration-tested with the `fake()` manager so no real `zellij` is needed.

**Files:**
- Modify: `crates/kamajid/src/state.rs`, `crates/kamajid/src/routes/tickets.rs`, `crates/kamajid/src/lib.rs`, `crates/kamajid/tests/api.rs`

- [ ] **Step 1: Hold `Arc<ZellijWeb>` in `AppState`**

In `crates/kamajid/src/state.rs`, add the field + setter (mirroring `state_dir`). Add `use crate::zellij_web::ZellijWeb;` to the imports. Update the struct, `new`, and add `set_zellij_web`/`zellij_web`:

```rust
#[derive(Clone)]
pub struct AppState {
    db: Arc<Mutex<Db>>,
    pub config: Arc<Config>,
    pub tx: broadcast::Sender<Event>,
    state_dir: Arc<PathBuf>,
    zellij_web: Arc<ZellijWeb>,
}
```

In `new`, add `zellij_web: Arc::new(ZellijWeb::new()),` to the constructed `AppState`. Add the accessors (near `set_state_dir`):

```rust
    /// Override the `zellij web` manager (tests inject `ZellijWeb::fake(...)`).
    pub fn set_zellij_web(&mut self, zw: ZellijWeb) {
        self.zellij_web = Arc::new(zw);
    }

    /// The `zellij web` manager (lazy server + token).
    pub fn zellij_web(&self) -> &ZellijWeb {
        &self.zellij_web
    }
```

- [ ] **Step 2: Add the `attach` handler to `tickets.rs`**

Append to `crates/kamajid/src/routes/tickets.rs`. Add `use kamaji_core::zellij_web` is NOT needed — the type lives in the daemon; the handler uses `state.zellij_web()` and the `AttachInfo` return type. Add `use crate::zellij_web::AttachInfo;` at the top if not present:

```rust
/// `POST /tickets/:id/attach` → the info a client needs to open the ticket's
/// session in a browser. 404 if the ticket is missing; 409 if it has no session
/// (start it first via `/start`). Ensures `zellij web` is running (real mode)
/// and returns `{ session_name, web_url, token }`. The blocking ensure-running
/// work runs on the blocking pool.
pub async fn attach(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<AttachInfo>, ApiError> {
    // Resolve the ticket's recorded session name (the authoritative value).
    let session_name = state
        .with_db(move |db| Ok(db.get_ticket(id)?.map(|t| t.session_name)))
        .await?
        .ok_or(ApiError::NotFound)?
        .ok_or_else(|| ApiError::BadRequest("ticket has no session; start it first".into()))?;

    // Ensure zellij web + token. This can spawn a subprocess and probe a socket,
    // so run it on the blocking pool (mirrors the daemon's other blocking work).
    let state2 = state.clone();
    let info = tokio::task::spawn_blocking(move || state2.zellij_web().attach_info(&session_name))
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("attach task panicked: {e}")))?
        .map_err(ApiError::Internal)?;
    Ok(Json(info))
}
```

Note: `db.get_ticket(id)?.map(|t| t.session_name)` yields `Option<Option<String>>` — the outer `Option` is "ticket exists?", the inner is "has a session?". The two `.ok_or(...)` calls peel them: outer → 404, inner → 409-equivalent (here a 400 `bad_request`, consistent with the daemon's existing error vocabulary — there is no 409 variant on `ApiError`; `BadRequest` with a clear message is the established pattern).

- [ ] **Step 3: Mount the route in `lib.rs`**

In `crates/kamajid/src/lib.rs`, add to `router` (after `/tickets/:id/done`):

```rust
        .route(
            "/tickets/:id/attach",
            axum::routing::post(routes::tickets::attach),
        )
```

- [ ] **Step 4: Write the `/attach` integration tests**

Append to `crates/kamajid/tests/api.rs`. These build an `AppState` with the FAKE manager so no real `zellij` is needed:

```rust
/// Boot a daemon whose ZellijWeb is the fake (canned token, no subprocess).
async fn spawn_with_fake_attach(token: &str) -> (String, kamajid::state::AppState) {
    let mut state =
        kamajid::state::AppState::new(Db::open_in_memory().unwrap(), Config::default());
    state.set_zellij_web(kamajid::zellij_web::ZellijWeb::fake(token));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = kamajid::router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), state)
}

#[tokio::test]
async fn attach_returns_info_for_a_ticket_with_a_session() {
    let (base, state) = spawn_with_fake_attach("tok-123").await;
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

    let info: serde_json::Value = reqwest::Client::new()
        .post(format!("{base}/tickets/{tid}/attach"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["session_name"], "kamaji-1-t");
    assert_eq!(info["web_url"], "http://127.0.0.1:8082/kamaji-1-t");
    assert_eq!(info["token"], "tok-123");
}

#[tokio::test]
async fn attach_on_ticket_without_a_session_is_400() {
    let (base, state) = spawn_with_fake_attach("tok").await;
    let tid = state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            // No session set → cannot attach.
            let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
            Ok(t.id)
        })
        .await
        .unwrap();

    let resp = reqwest::Client::new()
        .post(format!("{base}/tickets/{tid}/attach"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["kind"], "bad_request");
}

#[tokio::test]
async fn attach_missing_ticket_is_404() {
    let (base, _state) = spawn_with_fake_attach("tok").await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/tickets/999/attach"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}
```

- [ ] **Step 5: Build, test**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test -p kamajid 2>&1 | grep -E 'test result:'
cargo test --all-targets --all-features 2>&1 | grep -E 'test result:'
```

Expected: the `tests/api.rs` binary now has 19 tests (16 + 3 new); the `zellij_web` unit-test binary has 4 passed + 1 ignored; whole suite green.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(kamajid): POST /tickets/:id/attach

Resolve the ticket's recorded session, ensure zellij web is running
(blocking-pool), and return AttachInfo { session_name, web_url, token }.
404 when the ticket is missing, 400 when it has no session. Integration
tests use the fake ZellijWeb (no real zellij). AppState holds an
Arc<ZellijWeb>, default real, with a setter for the fake. Phase 1d
step 3."
```

---

## Task 4: Ship

- [ ] **Step 1: Final full verification**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E '(test result:|Running )'
```

Expected: all green — kamaji 152, kamaji-core 97, kamajid integration 19, kamajid `zellij_web` unit 4 (+1 ignored).

- [ ] **Step 2: Manual smoke — only if `zellij` ≥ 0.43 is installed**

If a real `zellij` is available, run the live path:

```bash
cargo test -p kamajid -- --ignored zellij_web_real_attach_info
```

Expected: PASS — `zellij web --create-token` yields a non-empty token and the server becomes reachable. **If the token parse fails**, inspect the real `zellij web --create-token` stdout and adjust `create_token`'s parser accordingly (the spec flagged this as a spike). If `zellij` is NOT installed, skip this step — CI does not run it.

End-to-end (optional, with zellij): start the daemon, start a ticket's session, then attach:

```bash
cargo build --release
DATA=$(mktemp -d); CFG=$(mktemp -d)
XDG_DATA_HOME=$DATA XDG_CONFIG_HOME=$CFG ./target/release/kamajid serve --bind 127.0.0.1:8804 &
# … create a project + ticket in a real git repo, POST /start, then:
# curl -s -X POST http://127.0.0.1:8804/tickets/1/attach
# → {"session_name":"kamaji-1-…","web_url":"http://127.0.0.1:8082/kamaji-1-…","token":"…"}
# Open web_url in a browser to confirm the session attaches.
```

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

- **Spec coverage (§6):** `ZellijWeb` owns the server + token (Task 1–2); lazy ensure-running triggered only by `/attach` (Task 3 — no startup/healthz hook); `AttachInfo { session_name, web_url, token }` (Task 1); the `/tickets/:id/attach` route (Task 3). **Deferred (documented below):** kill-on-SIGTERM shutdown of the held child (the spec accepts the server outliving the daemon + reuse on next start, so Phase 1 holds no child); token-cache invalidation on a manual `--delete-token` (a 401-probe refinement — flagged as a spike; the cached-token-keeps-working path is the common case).
- **Type consistency:** `AttachInfo` fields (`session_name`, `web_url`, `token`) and `web_url(base, session)` / `ZellijWeb::{new, fake, attach_info}` used identically across tasks. `AppState::{set_zellij_web, zellij_web}` consistent. The route returns `Json<AttachInfo>`.
- **No placeholders:** every code step is complete. The one acknowledged uncertainty — the exact `zellij web --create-token` stdout format — has concrete parser code plus an explicit verification step (the manual smoke), matching the spec's own "spike" note.
- **CI safety:** the only code requiring a real `zellij` is `ensure_running`, reached solely by `ZellijWeb::new()`. Every CI test uses `ZellijWeb::fake(...)` or pure helpers; the live test is `#[ignore]`d.

## What this plan deliberately does NOT do (→ Plan 1e: daemon hardening & completeness)

- **Autonomous `session.exited` reconciliation** of vanished sessions (a `kamaji-core` `reconcile` helper + poll-task wiring). The `/done` cleanup already emits `session.exited`; the *autonomous* detection of an externally-killed session is 1e.
- `kill`-on-shutdown of the `zellij web` child + token-cache 401-invalidation refinement.
- The carried 1b/1c follow-ups: `PATCH /config`; `/start` double-start guard (409 if already running); SQLite FK enforcement (`PRAGMA foreign_keys`) to prevent orphaned tickets; `PATCH /tickets/:id` field expansion (`initial_prompt`/`agent`); write-side 404 + `ticket.updated`/`deleted` SSE tests.
- Daemon auto-spawn and the TUI-as-client flip (Phase 2).
