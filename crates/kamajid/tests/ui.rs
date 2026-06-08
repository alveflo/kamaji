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
async fn clearing_done_patches_a_remove_per_done_card() {
    use kamaji_core::models::{Agent, Status};
    let (base, state) = spawn().await;
    let (pid, done_a, done_b) = state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            let a = db.create_ticket(p.id, "done-a", "", None, Agent::Claude)?;
            let b = db.create_ticket(p.id, "done-b", "", None, Agent::Claude)?;
            db.set_ticket_status(a.id, Status::Done)?;
            db.set_ticket_status(b.id, Status::Done)?;
            Ok((p.id, a.id, b.id))
        })
        .await
        .unwrap();

    let mut stream = connect_ui_events(&base).await;
    for _ in 0..4 {
        let _ = read_patch(&mut stream).await;
    } // drain the snapshot

    reqwest::Client::new()
        .delete(format!("{base}/projects/{pid}/done-tickets"))
        .send()
        .await
        .unwrap();

    let p1 = read_patch(&mut stream).await;
    assert!(p1.contains("mode remove"), "first is a remove:\n{p1}");
    assert!(
        p1.contains(&format!("#card-{done_a}")),
        "targets first done card:\n{p1}"
    );
    let p2 = read_patch(&mut stream).await;
    assert!(p2.contains("mode remove"), "second is a remove:\n{p2}");
    assert!(
        p2.contains(&format!("#card-{done_b}")),
        "targets second done card:\n{p2}"
    );
}

#[tokio::test]
async fn clear_done_confirm_modal_hits_bulk_delete() {
    let (base, state) = spawn().await;
    let pid = state
        .with_db(|db| {
            Ok(db
                .create_project("p", std::path::Path::new("/tmp/p"), None)?
                .id)
        })
        .await
        .unwrap();
    let body = reqwest::get(format!("{base}/ui/projects/{pid}/confirm-delete-done"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        body.starts_with(r#"<div id="modal">"#),
        "fragment rooted at #modal for morph-by-id:\n{body}"
    );
    assert!(
        body.contains(&format!(
            "fetch('/projects/{pid}/done-tickets',{{method:'DELETE'}})"
        )),
        "confirm hits the bulk delete endpoint:\n{body}"
    );
    assert!(
        body.contains("Delete all done tickets?"),
        "names the bulk action:\n{body}"
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

/// `GET /ui/tickets/:id/confirm?action=done` serves the Done confirmation modal:
/// rooted at `#modal` (so `@get` morphs the mount), naming the teardown and
/// carrying the `/done` command (with the cleanup body) on its Confirm button.
#[tokio::test]
async fn confirm_done_modal_renders() {
    let (base, _state) = spawn().await;
    let body = reqwest::get(format!("{base}/ui/tickets/42/confirm?action=done"))
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
        body.contains("Mark #42 done and tear down its session?"),
        "names the teardown:\n{body}"
    );
    assert!(
        body.contains("fetch('/tickets/42/done',{method:'POST'")
            && body.contains("body:JSON.stringify({cleanup:true})"),
        "Confirm fires the teardown POST with the cleanup body:\n{body}"
    );
}

/// `GET /ui/tickets/:id/confirm?action=delete` serves the Delete confirmation
/// modal, warning it is irreversible and carrying the DELETE on Confirm.
#[tokio::test]
async fn confirm_delete_modal_renders() {
    let (base, _state) = spawn().await;
    let body = reqwest::get(format!("{base}/ui/tickets/9/confirm?action=delete"))
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
        body.contains("Delete #9? This cannot be undone."),
        "warns it is irreversible:\n{body}"
    );
    assert!(
        body.contains("fetch('/tickets/9',{method:'DELETE'})"),
        "Confirm fires the DELETE:\n{body}"
    );
}

/// An unknown `?action=` is rejected (the query enum only accepts done|delete).
#[tokio::test]
async fn confirm_modal_rejects_unknown_action() {
    let (base, _state) = spawn().await;
    let resp = reqwest::get(format!("{base}/ui/tickets/1/confirm?action=bogus"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "unknown action is a 400");
}

/// `GET /ui/projects/new` serves the create-project modal fragment: rooted at
/// `#modal` (so `@get` morphs the mount), composing the shared chrome, and
/// submitting via `POST /projects`. On success it navigates to the new project
/// so the rail shows its tile (projects broadcast no SSE event).
#[tokio::test]
async fn new_project_modal_renders_form() {
    let (base, _state) = spawn().await;
    let body = reqwest::get(format!("{base}/ui/projects/new"))
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
        body.contains("fetch('/projects',{method:'POST'"),
        "create action posts to /projects:\n{body}"
    );
    assert!(body.contains(r#"name="name""#), "name field:\n{body}");
    assert!(
        body.contains(r#"name="root_dir" class="mono""#),
        "root dir is a mono path input:\n{body}"
    );
    assert!(
        body.contains(r#"class="seg""#),
        "agent is a segmented control:\n{body}"
    );
    assert!(
        body.contains("window.location='/?project='+p.id"),
        "navigates to the new project on success:\n{body}"
    );
}

/// End-to-end: the fragment's `POST /projects` body deserializes and creates a
/// project, and its returned `id` is what the fragment navigates to — so the
/// rail (which renders from the project list) shows the new tile after the nav.
#[tokio::test]
async fn create_project_via_fragments_endpoint_succeeds() {
    let (base, state) = spawn().await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/projects"))
        .json(&serde_json::json!({
            "name": "Acme",
            "root_dir": "/tmp/acme",
            "default_agent": "codex",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let created: serde_json::Value = resp.json().await.unwrap();
    let id = created["id"].as_i64().unwrap();
    let names = state.with_db(|db| db.list_projects()).await.unwrap();
    assert!(
        names.iter().any(|p| p.id == id && p.name == "Acme"),
        "created project is listed (rail renders from this list): {names:?}"
    );
}

/// Boot a daemon whose session driver reports a fixed `list-sessions` output,
/// and seed one ticket whose session is in that list.
async fn spawn_with_sessions(list: &str) -> (String, AppState) {
    let mut state = AppState::new(Db::open_in_memory().unwrap(), Config::default());
    state.set_session_driver(std::sync::Arc::new(
        kamajid::session_driver::FakeSessionDriver::new(true).with_sessions(list),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = kamajid::router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), state)
}

#[tokio::test]
async fn manage_sessions_modal_lists_classified_sessions() {
    let list = "kamaji-1-fix-auth [Created 2h ago]\nkamaji-9-orphan [Created 1h ago]\n";
    let (base, state) = spawn_with_sessions(list).await;
    state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            let t = db.create_ticket(
                p.id,
                "Fix auth",
                "",
                None,
                kamaji_core::models::Agent::Claude,
            )?;
            db.set_ticket_session(t.id, "kamaji-1-fix-auth", "/wt", "kamaji-1-fix-auth")?;
            Ok(())
        })
        .await
        .unwrap();

    let html = reqwest::get(format!("{base}/ui/sessions/manage"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.starts_with(r#"<div id="modal">"#), "{html}");
    assert!(
        html.contains(r#"value="kamaji-1-fix-auth""#),
        "ticket session row:\n{html}"
    );
    assert!(html.contains("Fix auth"), "ticket title:\n{html}");
    assert!(
        html.contains(r#"value="kamaji-9-orphan""#),
        "orphan row:\n{html}"
    );
    assert!(html.contains("orphan"), "orphan label:\n{html}");
}
