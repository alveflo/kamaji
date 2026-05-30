//! Integration tests: boot the daemon on an ephemeral port with an in-memory
//! DB, drive it over HTTP with reqwest, and assert responses + SSE events.

mod support;

use futures::StreamExt;
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

#[tokio::test]
async fn lists_and_gets_projects_and_tickets() {
    let (base, state) = spawn().await;
    // Seed directly through the DB the daemon owns.
    let (pid, tid) = state
        .with_db(|db| {
            let p = db.create_project("acme", std::path::Path::new("/tmp/acme"), None)?;
            let t = db.create_ticket(
                p.id,
                "Add login",
                "desc",
                Some("do it"),
                kamaji_core::models::Agent::Claude,
            )?;
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

#[tokio::test]
async fn create_edit_move_delete_ticket_lifecycle() {
    let (base, state) = spawn().await;
    let pid = state
        .with_db(|db| {
            Ok(db
                .create_project("p", std::path::Path::new("/tmp/p"), None)?
                .id)
        })
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
    let resp = client
        .get(format!("{base}/tickets/{tid}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn create_ticket_rejects_empty_title() {
    let (base, state) = spawn().await;
    let pid = state
        .with_db(|db| {
            Ok(db
                .create_project("p", std::path::Path::new("/tmp/p"), None)?
                .id)
        })
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

#[tokio::test]
async fn creating_a_ticket_emits_ticket_created_on_sse() {
    let (base, state) = spawn().await;
    let pid = state
        .with_db(|db| {
            Ok(db
                .create_project("p", std::path::Path::new("/tmp/p"), None)?
                .id)
        })
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
                let t =
                    db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
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
    let cfg = kamaji_core::config::Config {
        worktree_base: Some(format!("{}/wt", std::env::temp_dir().display())),
        ..Default::default()
    };
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
                let t =
                    db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
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

#[tokio::test]
async fn start_rolls_back_fully_on_session_spawn_failure() {
    // Prepare succeeds (real git repo + worktree_base set), but spawning the
    // zellij session fails when no zellij binary is present — exercising the
    // rollback. Skipped when zellij IS available (it would spawn a real session).
    if support::zellij_available() {
        return;
    }
    let repo = support::committed_repo();
    let wt_base = tempfile::tempdir().unwrap();
    let cfg = kamaji_core::config::Config {
        worktree_base: Some(wt_base.path().join("wt").to_string_lossy().to_string()),
        ..kamaji_core::config::Config::default()
    };
    let mut state = kamajid::state::AppState::new(Db::open_in_memory().unwrap(), cfg);
    let sd = tempfile::tempdir().unwrap();
    state.set_state_dir(sd.path().to_path_buf());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = kamajid::router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{addr}");

    let tid = state
        .with_db({
            let root = repo.path().to_path_buf();
            move |db| {
                let p = db.create_project("p", &root, None)?;
                let t =
                    db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
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
    assert_eq!(resp.status(), 500);

    // Rolled back fully: the ticket is back in its prior column (Todo) with no
    // session recorded — the failed start left no trace in the DB.
    let t: serde_json::Value = reqwest::get(format!("{base}/tickets/{tid}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(t["status"], "todo");
    assert!(t["session_name"].is_null());
}

#[tokio::test]
async fn done_without_cleanup_moves_to_done_and_keeps_session() {
    let (base, state) = spawn().await;
    let tid = state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
            // Give it a recorded session so we can assert cleanup did NOT run.
            db.set_ticket_session(t.id, "kamaji-1-t", "/wt", "kamaji-1-t")?;
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
    // cleanup=false must leave the session columns intact (no teardown).
    assert_eq!(ticket["session_name"], "kamaji-1-t");
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
                let t =
                    db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
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

#[tokio::test]
async fn poll_round_moves_idle_claude_ticket_to_review_and_emits() {
    use kamaji_core::poll::PollLoop;

    // A daemon whose marker dir is a temp dir we control.
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
    poll = kamajid::poll_task::poll_round(&state, poll, &sd).await;
    // The agent "stops": its Stop hook creates the idle marker.
    let marker = kamaji_core::detect::marker_path(state_dir.path(), "kamaji-1-t");
    std::fs::write(&marker, "").unwrap();
    // Round 2: marker present → Idle → move to Review + emit. (Returned PollLoop
    // isn't needed after the final round.)
    let _ = kamajid::poll_task::poll_round(&state, poll, &sd).await;

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

#[tokio::test]
async fn poll_respects_externally_cleared_auto_review_provenance() {
    // Regression: a human who manually keeps a card in Review (POST /move clears
    // its `auto_reviewed` column) must not have it dragged back to In Progress by
    // a later active signal. The poll task re-syncs provenance from the DB each
    // round, so the externally-cleared flag is honored.
    use kamaji_core::poll::PollLoop;

    let state_dir = tempfile::tempdir().unwrap();
    let mut state = kamajid::state::AppState::new(
        Db::open_in_memory().unwrap(),
        kamaji_core::config::Config::default(),
    );
    state.set_state_dir(state_dir.path().to_path_buf());

    let tid = state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
            db.set_ticket_session(t.id, "kamaji-2-t", "/wt", "kamaji-2-t")?;
            db.set_ticket_status(t.id, kamaji_core::models::Status::InProgress)?;
            db.set_ticket_instrumented(t.id, true)?;
            Ok(t.id)
        })
        .await
        .unwrap();

    let mut poll = PollLoop::new();
    let sd = state_dir.path().to_path_buf();
    let marker = kamaji_core::detect::marker_path(state_dir.path(), "kamaji-2-t");

    // Round 1: no marker → Active baseline. Round 2: marker → Idle → auto-move to Review.
    poll = kamajid::poll_task::poll_round(&state, poll, &sd).await;
    std::fs::write(&marker, "").unwrap();
    poll = kamajid::poll_task::poll_round(&state, poll, &sd).await;
    assert_eq!(
        state
            .with_db(move |db| Ok(db.get_ticket(tid)?.unwrap().status))
            .await
            .unwrap(),
        kamaji_core::models::Status::Review
    );

    // The human clears auto-review provenance in the DB (what POST /move does).
    state
        .with_db(move |db| db.set_ticket_auto_reviewed(tid, false))
        .await
        .unwrap();

    // The agent resumes (marker gone → Active). Without the per-round rehydrate,
    // the stale in-memory provenance would drag the card back to In Progress.
    std::fs::remove_file(&marker).unwrap();
    let _ = kamajid::poll_task::poll_round(&state, poll, &sd).await;

    assert_eq!(
        state
            .with_db(move |db| Ok(db.get_ticket(tid)?.unwrap().status))
            .await
            .unwrap(),
        kamaji_core::models::Status::Review,
        "a manually-kept Review card must not be auto-dragged back"
    );
}
