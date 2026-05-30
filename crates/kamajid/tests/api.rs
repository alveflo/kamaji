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
