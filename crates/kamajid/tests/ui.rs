//! Integration tests for the Phase 3 browser routes. Boots the daemon on an
//! ephemeral port (same harness as tests/api.rs) and drives it over HTTP.

mod support;

use kamaji_core::config::Config;
use kamaji_core::db::Db;
use kamajid::state::AppState;

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
async fn serves_embedded_datastar_and_css() {
    let (base, _state) = spawn().await;
    let js = reqwest::get(format!("{base}/assets/datastar.js"))
        .await
        .unwrap();
    assert_eq!(js.status(), 200);
    let ct = js
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.contains("javascript"), "datastar served as JS, got {ct}");

    let css = reqwest::get(format!("{base}/assets/app.css"))
        .await
        .unwrap();
    assert_eq!(css.status(), 200);
    let ct = css
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.contains("css"), "css content-type, got {ct}");

    let missing = reqwest::get(format!("{base}/assets/nope.txt"))
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}

#[tokio::test]
async fn board_page_renders_columns_and_seeded_card() {
    let (base, state) = spawn().await;
    state
        .with_db(|db| {
            let p = db.create_project("acme", std::path::Path::new("/tmp/acme"), None)?;
            db.create_ticket(
                p.id,
                "Add login",
                "",
                None,
                kamaji_core::models::Agent::Claude,
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let resp = reqwest::get(format!("{base}/")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.contains("text/html"), "html content-type, got {ct}");
    let body = resp.text().await.unwrap();
    for id in ["col-todo", "col-in_progress", "col-review", "col-done"] {
        assert!(body.contains(id), "missing {id}:\n{body}");
    }
    assert!(body.contains("Add login"), "seeded card title present");
    assert!(
        body.contains("Needs attention"),
        "review header label present"
    );
}

#[tokio::test]
async fn move_command_relocates_card_on_next_render() {
    let (base, state) = spawn().await;
    let tid = state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
            Ok(t.id)
        })
        .await
        .unwrap();

    reqwest::Client::new()
        .post(format!("{base}/tickets/{tid}/move"))
        .json(&serde_json::json!({ "target": "in_progress" }))
        .send()
        .await
        .unwrap();

    let body = reqwest::get(format!("{base}/"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    // After the move, the in_progress column contains the card.
    let in_prog = body.split(r#"id="col-in_progress""#).nth(1).unwrap();
    let next_col = in_prog.split(r#"id="col-review""#).next().unwrap();
    assert!(
        next_col.contains(&format!("card-{tid}")),
        "card now in in_progress:\n{body}"
    );
}
