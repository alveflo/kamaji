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
