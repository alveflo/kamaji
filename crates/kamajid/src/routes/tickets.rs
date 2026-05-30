//! Ticket resource routes.

use axum::extract::{Path, State};
use axum::Json;
use kamaji_core::models::Ticket;

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /projects/:id/tickets` → the project's tickets, ordered.
pub async fn list_for_project(
    State(state): State<AppState>,
    Path(project_id): Path<i64>,
) -> Result<Json<Vec<Ticket>>, ApiError> {
    let tickets = state.with_db(move |db| db.list_tickets(project_id)).await?;
    Ok(Json(tickets))
}

/// `GET /tickets/:id` → one ticket, or 404.
pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Ticket>, ApiError> {
    let ticket = state
        .with_db(move |db| db.get_ticket(id))
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(ticket))
}
