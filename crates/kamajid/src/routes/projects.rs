//! Project resource routes.

use axum::extract::{Path, State};
use axum::Json;
use kamaji_core::models::Project;

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
