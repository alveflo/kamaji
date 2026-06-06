//! Session-management command routes. `POST /sessions/delete` tears down the
//! selected zellij sessions with a smart per-session rule: a ticket-linked
//! session gets the full `session::cleanup_ticket` (kill + remove worktree +
//! delete branch + clear DB columns) and a `session.exited` broadcast; an orphan
//! or main/project session is just killed via the session-driver seam. The
//! request's session kinds are re-derived server-side from the live DB + zellij
//! list — the client's checkbox values are only a selection set, never trusted.

use axum::extract::State;
use axum::Json;
use kamaji_core::events::Event;
use kamaji_core::session::{self, SessionEntry, SessionKind};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct DeleteSessions {
    pub names: Vec<String>,
}

/// `POST /sessions/delete` → tear down the named sessions. Body
/// `{ "names": ["kamaji-…", …] }`. Returns `{ "deleted": N, "failed": [...] }`.
/// Per-session failures are collected, not fatal: one bad name never aborts the
/// batch.
pub async fn delete(
    State(state): State<AppState>,
    Json(body): Json<DeleteSessions>,
) -> Result<Json<Value>, ApiError> {
    if body.names.is_empty() {
        return Err(ApiError::BadRequest("no sessions selected".into()));
    }

    // Snapshot + classify the live sessions once (off the runtime: list shells out).
    let st = state.clone();
    let list = tokio::task::spawn_blocking(move || st.sessions().list_sessions())
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("list-sessions task panicked: {e}")))?;
    let entries: Vec<SessionEntry> = match list {
        Some(list) => {
            state
                .with_db(move |db| session::classify_sessions(db, &list))
                .await?
        }
        None => Vec::new(),
    };

    let mut deleted = 0u64;
    let mut failed: Vec<Value> = Vec::new();

    for name in &body.names {
        if !name.starts_with("kamaji-") {
            failed.push(json!({ "name": name, "reason": "not a kamaji session" }));
            continue;
        }
        match entries.iter().find(|e| &e.name == name).map(|e| &e.kind) {
            // Ticket-linked: full teardown + SessionExited.
            Some(SessionKind::Ticket { id, .. }) => {
                let id = *id;
                let state_dir = state.state_dir().to_path_buf();
                let res = state
                    .with_db(move |db| {
                        let Some(t) = db.get_ticket(id)? else {
                            return Ok(false);
                        };
                        let Some(p) = db.get_project(t.project_id)? else {
                            return Ok(false);
                        };
                        session::cleanup_ticket(db, &p.root_dir, &state_dir, id)?;
                        Ok(true)
                    })
                    .await;
                match res {
                    Ok(true) => {
                        deleted += 1;
                        state.emit(Event::SessionExited {
                            ticket_id: id,
                            session_name: name.clone(),
                        });
                    }
                    // Ticket vanished between classify and teardown → already gone.
                    Ok(false) => deleted += 1,
                    Err(e) => {
                        failed.push(json!({ "name": name, "reason": format_api_error(&e) }));
                    }
                }
            }
            // Orphan / main: kill via the driver (best-effort, off the runtime).
            Some(SessionKind::Main) | Some(SessionKind::Orphan) => {
                let st = state.clone();
                let n = name.clone();
                let _ = tokio::task::spawn_blocking(move || st.sessions().terminate(&n)).await;
                deleted += 1;
            }
            // A kamaji-* name not in the live list: already gone → idempotent success.
            None => deleted += 1,
        }
    }

    Ok(Json(json!({ "deleted": deleted, "failed": failed })))
}

/// Human-readable reason for a per-session teardown failure. `with_db` only ever
/// yields `ApiError::Internal`, but matching the whole enum keeps this total.
fn format_api_error(e: &ApiError) -> String {
    match e {
        ApiError::Internal(err) => err.to_string(),
        ApiError::BadRequest(m) => m.clone(),
        ApiError::NotFound => "not found".to_string(),
    }
}
