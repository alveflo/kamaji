//! Project resource routes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use kamaji_core::events::Event;
use kamaji_core::models::{Agent, Project};
use kamaji_core::{session, slug, zellij};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /projects` → all projects.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<Project>>, ApiError> {
    let projects = state.with_db(|db| db.list_projects()).await?;
    Ok(Json(projects))
}

/// `GET /projects/:id` → one project, or 404.
pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Project>, ApiError> {
    let project = state
        .with_db(move |db| db.get_project(id))
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(project))
}

#[derive(Deserialize)]
pub struct CreateProject {
    pub name: String,
    pub root_dir: std::path::PathBuf,
    #[serde(default)]
    pub default_agent: Option<Agent>,
}

/// `POST /projects` → create a project. (No project event type exists in the
/// taxonomy, so nothing is broadcast.)
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateProject>,
) -> Result<(StatusCode, Json<Project>), ApiError> {
    if body.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name must not be empty".into()));
    }
    let project = state
        .with_db(move |db| db.create_project(&body.name, &body.root_dir, body.default_agent))
        .await?;
    Ok((StatusCode::CREATED, Json(project)))
}

#[derive(Deserialize)]
pub struct UpdateProject {
    pub name: String,
    pub root_dir: std::path::PathBuf,
    #[serde(default)]
    pub default_agent: Option<Agent>,
}

/// `PATCH /projects/:id` → edit a project's name, root dir, and default agent
/// (full replace of all three). `name` must be non-empty. 404 if the project is
/// missing. Like create, no project event exists, so nothing is broadcast — the
/// browser navigates to reflect the rename and the TUI updates in place.
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateProject>,
) -> Result<Json<Project>, ApiError> {
    if body.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name must not be empty".into()));
    }
    let project = state
        .with_db(move |db| db.update_project(id, &body.name, &body.root_dir, body.default_agent))
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(project))
}

/// `DELETE /projects/:id` → delete the project and every ticket it owns. Before
/// removing the rows, each ticket's worktree + zellij session + branch is torn
/// down (best-effort: one ticket's failed teardown is logged and never aborts
/// the delete). Emits one `ticket.deleted` per removed card so any board still
/// viewing the project drops them. 404 if the project does not exist; otherwise
/// 204. Unlike the bulk Done-clear, deleting a whole project DOES tear down its
/// sessions/worktrees — nothing is left to own them once the project is gone.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let state_dir = state.state_dir().to_path_buf();
    let deleted = state
        .with_db(move |db| {
            let Some(project) = db.get_project(id)? else {
                return Ok(None);
            };
            // Tear down each ticket's session/worktree before the rows vanish.
            // Best-effort: a single ticket's failed teardown is logged, never
            // fatal, so the project still gets deleted.
            for ticket in db.list_tickets(id)? {
                if let Err(e) =
                    session::cleanup_ticket(db, &project.root_dir, &state_dir, ticket.id)
                {
                    tracing::warn!(
                        project_id = id,
                        ticket_id = ticket.id,
                        error = %e,
                        "cleanup during project delete failed; deleting the row anyway"
                    );
                }
            }
            Ok(Some(db.delete_project(id)?))
        })
        .await?;
    let ids = deleted.ok_or(ApiError::NotFound)?;
    for ticket_id in ids {
        state.emit(Event::TicketDeleted { id: ticket_id });
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
pub struct MainSession {
    pub session_name: String,
}

/// `POST /projects/:id/main-session` → start (or reuse) the project's main
/// workspace session — not tied to any ticket — and return its name. Idempotent:
/// if zellij already lists the session, no new one is spawned. 404 if the
/// project is missing; 500 if layout prep or the zellij spawn fails.
pub async fn main_session(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<MainSession>, ApiError> {
    let project = state
        .with_db(move |db| db.get_project(id))
        .await?
        .ok_or(ApiError::NotFound)?;
    let config = state.config_async().await;
    let name = slug::main_session_name(project.id);
    let already_live = tokio::task::spawn_blocking({
        let name = name.clone();
        move || {
            zellij::list_sessions()
                .map(|l| zellij::session_in_list(&l, &name))
                .unwrap_or(false)
        }
    })
    .await
    .map_err(|e| ApiError::Internal(anyhow::anyhow!("list-sessions task panicked: {e}")))?;
    if already_live {
        return Ok(Json(MainSession { session_name: name }));
    }
    let prepared =
        tokio::task::spawn_blocking(move || session::prepare_main_session(&project, &config))
            .await
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("prepare task panicked: {e}")))?
            .map_err(|e| ApiError::Internal(anyhow::anyhow!("prepare main session failed: {e}")))?;
    let cwd = std::env::temp_dir();
    let layout = prepared.layout_path.clone();
    let name2 = prepared.name.clone();
    tokio::task::spawn_blocking(move || zellij::create_session_background(&name2, &layout, &cwd))
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("spawn task panicked: {e}")))?
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("starting main session failed: {e}")))?;
    Ok(Json(MainSession {
        session_name: prepared.name,
    }))
}
