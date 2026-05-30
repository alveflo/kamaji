//! Project resource routes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use kamaji_core::models::{Agent, Project};
use serde::Deserialize;

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
