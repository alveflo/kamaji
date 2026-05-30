//! Project resource routes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
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
