//! Integration tests for the Phase 3 browser routes. Boots the daemon on an
//! ephemeral port (same harness as tests/api.rs) and drives it over HTTP.

mod support;

use futures::StreamExt;
use kamaji_core::config::Config;
use kamaji_core::db::Db;
use kamajid::state::AppState;

/// Box-pinned (so it is `Unpin` for `StreamExt::next`) live SSE byte stream.
type ByteStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>;

/// Open `/ui/events` (the Datastar fragment SSE stream) and return its live byte
/// stream. When this returns, the server-side broadcast subscription is active,
/// so any command issued afterwards is delivered on this stream.
async fn connect_ui_events(base: &str) -> ByteStream {
    let resp = reqwest::Client::new()
        .get(format!("{base}/ui/events"))
        .send()
        .await
        .unwrap();
    Box::pin(resp.bytes_stream())
}

/// Read SSE records until one whose `event:` is `datastar-patch-elements`,
/// returning its joined `data:` payload. Times out after ~2s to avoid hanging CI.
async fn read_patch<S>(stream: &mut S) -> String
where
    S: futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin,
{
    let mut buf = String::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let chunk = tokio::time::timeout_at(deadline, stream.next())
            .await
            .expect("timed out waiting for SSE patch")
            .expect("SSE stream ended")
            .expect("SSE chunk error");
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(idx) = buf.find("\n\n") {
            let record: String = buf.drain(..idx + 2).collect();
            let mut name = None;
            let mut data = String::new();
            for line in record.lines() {
                if let Some(v) = line.strip_prefix("event:") {
                    name = Some(v.trim().to_string());
                } else if let Some(v) = line.strip_prefix("data:") {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(v.trim());
                }
            }
            if name.as_deref() == Some("datastar-patch-elements") {
                return data;
            }
        }
    }
}

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

    let css = reqwest::get(format!("{base}/assets/tokens.css"))
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

#[tokio::test]
async fn ui_events_emits_full_board_snapshot_on_connect() {
    let (base, state) = spawn().await;
    state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            db.create_ticket(p.id, "Seed", "", None, kamaji_core::models::Agent::Claude)?;
            Ok(())
        })
        .await
        .unwrap();

    let mut stream = connect_ui_events(&base).await;
    // Collect the four snapshot column patches; the seeded card must appear in todo.
    let mut seen = String::new();
    for _ in 0..4 {
        seen.push_str(&read_patch(&mut stream).await);
    }
    assert!(
        seen.contains("col-todo"),
        "snapshot includes todo column:\n{seen}"
    );
    assert!(
        seen.contains("Seed"),
        "snapshot includes seeded card:\n{seen}"
    );
}

#[tokio::test]
async fn moving_a_ticket_patches_affected_columns() {
    let (base, state) = spawn().await;
    let tid = state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
            Ok(t.id)
        })
        .await
        .unwrap();

    let mut stream = connect_ui_events(&base).await;
    for _ in 0..4 {
        let _ = read_patch(&mut stream).await;
    } // drain snapshot

    reqwest::Client::new()
        .post(format!("{base}/tickets/{tid}/move"))
        .json(&serde_json::json!({ "target": "in_progress" }))
        .send()
        .await
        .unwrap();

    // Two column patches arrive (from=todo, to=in_progress); read both.
    let a = read_patch(&mut stream).await;
    let b = read_patch(&mut stream).await;
    let both = format!("{a}\n{b}");
    assert!(
        both.contains("col-todo"),
        "from column re-rendered:\n{both}"
    );
    assert!(
        both.contains("col-in_progress"),
        "to column re-rendered:\n{both}"
    );
    assert!(
        both.contains(&format!("card-{tid}")),
        "card present in a patch:\n{both}"
    );
}

#[tokio::test]
async fn deleting_a_ticket_patches_a_remove() {
    let (base, state) = spawn().await;
    let tid = state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            let t = db.create_ticket(p.id, "t", "", None, kamaji_core::models::Agent::Claude)?;
            Ok(t.id)
        })
        .await
        .unwrap();

    let mut stream = connect_ui_events(&base).await;
    for _ in 0..4 {
        let _ = read_patch(&mut stream).await;
    }

    reqwest::Client::new()
        .delete(format!("{base}/tickets/{tid}"))
        .send()
        .await
        .unwrap();

    let data = read_patch(&mut stream).await;
    assert!(data.contains("mode remove"), "remove mode:\n{data}");
    assert!(
        data.contains(&format!("#card-{tid}")),
        "targets the card:\n{data}"
    );
}

#[tokio::test]
async fn new_ticket_modal_renders_form() {
    let (base, state) = spawn().await;
    let pid = state
        .with_db(|db| {
            Ok(db
                .create_project("p", std::path::Path::new("/tmp/p"), None)?
                .id)
        })
        .await
        .unwrap();
    let body = reqwest::get(format!("{base}/ui/tickets/new?project={pid}"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        body.contains("fetch('/tickets',{method:'POST'"),
        "create action:\n{body}"
    );
    assert!(body.contains(r#"name="title""#), "title field:\n{body}");
    assert!(
        body.starts_with(r#"<div id="modal">"#),
        "fragment rooted at #modal for morph-by-id:\n{body}"
    );
}

#[tokio::test]
async fn edit_ticket_modal_prefills() {
    let (base, state) = spawn().await;
    let tid = state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            let t = db.create_ticket(
                p.id,
                "Add login",
                "",
                None,
                kamaji_core::models::Agent::Claude,
            )?;
            Ok(t.id)
        })
        .await
        .unwrap();
    let body = reqwest::get(format!("{base}/ui/tickets/{tid}/edit"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        body.contains(&format!("fetch('/tickets/{tid}',{{method:'PATCH'")),
        "patch action:\n{body}"
    );
    assert!(body.contains("Add login"), "prefilled title:\n{body}");
}

/// Creating a ticket re-renders its whole column (idempotent whole-column
/// re-render landed in 84027f3): the next patch carries `id="col-todo"` and the
/// new card, and is NOT an append.
#[tokio::test]
async fn creating_a_ticket_rerenders_its_column() {
    let (base, state) = spawn().await;
    let pid = state
        .with_db(|db| {
            Ok(db
                .create_project("p", std::path::Path::new("/tmp/p"), None)?
                .id)
        })
        .await
        .unwrap();

    let mut stream = connect_ui_events(&base).await;
    for _ in 0..4 {
        let _ = read_patch(&mut stream).await;
    } // drain snapshot

    reqwest::Client::new()
        .post(format!("{base}/tickets"))
        .json(&serde_json::json!({ "project_id": pid, "title": "Fresh", "agent": "claude" }))
        .send()
        .await
        .unwrap();

    let data = read_patch(&mut stream).await;
    assert!(
        data.contains(r#"id="col-todo""#),
        "re-renders the todo column:\n{data}"
    );
    assert!(data.contains("Fresh"), "new card content:\n{data}");
    assert!(
        !data.contains("mode append"),
        "whole-column re-render, not an append:\n{data}"
    );
}

/// `GET /ui/tickets/cancel` returns the empty `#modal` mount, which `@get`
/// morphs over `#modal` to clear the dialog. (The Cancel button itself clears
/// `#modal` inline, but the route remains a valid server-side close path.)
#[tokio::test]
async fn cancel_route_returns_empty_modal() {
    let (base, _state) = spawn().await;
    let resp = reqwest::get(format!("{base}/ui/tickets/cancel"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains(r#"id="modal""#),
        "carries #modal mount:\n{body}"
    );
    assert!(
        !body.contains("<dialog"),
        "clears the dialog (empty mount):\n{body}"
    );
}

/// The new-ticket fragment morphs the `#modal` mount (so it actually appears)
/// and carries the client-side close: a successful `@post` resolves and clears
/// `#modal`. The full DOM clear happens in the browser; here we assert the
/// served fragment carries the mechanism (the server-observable half).
#[tokio::test]
async fn new_ticket_fragment_mounts_and_self_closes() {
    let (base, state) = spawn().await;
    let pid = state
        .with_db(|db| {
            Ok(db
                .create_project("p", std::path::Path::new("/tmp/p"), None)?
                .id)
        })
        .await
        .unwrap();
    let body = reqwest::get(format!("{base}/ui/tickets/new?project={pid}"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        body.starts_with(r#"<div id="modal">"#),
        "morphs the #modal mount:\n{body}"
    );
    assert!(
        body.contains("document.getElementById('modal').replaceChildren()"),
        "submit closes the modal on a 2xx:\n{body}"
    );
    assert!(
        body.contains("f.elements['start_now'].checked"),
        "create-submit branches on the background-start checkbox:\n{body}"
    );
}
