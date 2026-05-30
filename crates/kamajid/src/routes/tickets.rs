//! Ticket resource routes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use kamaji_core::events::Event;
use kamaji_core::models::{Agent, Status, Ticket};
use serde::Deserialize;

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

#[derive(Deserialize)]
pub struct CreateTicket {
    pub project_id: i64,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub initial_prompt: Option<String>,
    pub agent: Agent,
}

/// `POST /tickets` → create a ticket in Todo. Emits `ticket.created`.
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateTicket>,
) -> Result<(StatusCode, Json<Ticket>), ApiError> {
    if body.title.trim().is_empty() {
        return Err(ApiError::BadRequest("title must not be empty".into()));
    }
    let ticket = state
        .with_db(move |db| {
            db.create_ticket(
                body.project_id,
                &body.title,
                &body.description,
                body.initial_prompt.as_deref(),
                body.agent,
            )
        })
        .await?;
    state.emit(Event::TicketCreated(ticket.clone()));
    Ok((StatusCode::CREATED, Json(ticket)))
}

#[derive(Deserialize)]
pub struct UpdateTicket {
    pub title: String,
    pub description: String,
}

/// `PATCH /tickets/:id` → edit title/description. Emits `ticket.updated`.
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateTicket>,
) -> Result<Json<Ticket>, ApiError> {
    let ticket = state
        .with_db(move |db| {
            if db.get_ticket(id)?.is_none() {
                return Ok(None);
            }
            db.update_ticket_fields(id, &body.title, &body.description)?;
            db.get_ticket(id)
        })
        .await?
        .ok_or(ApiError::NotFound)?;
    state.emit(Event::TicketUpdated(ticket.clone()));
    Ok(Json(ticket))
}

#[derive(Deserialize)]
pub struct MoveTicket {
    pub target: Status,
}

/// `POST /tickets/:id/move` → set the ticket's column. A manual move clears
/// auto-review provenance (so a human-placed card is not auto-dragged back).
/// Emits `ticket.moved` only when the column actually changes. This does NOT
/// start or stop any session — that is the `/start` route (Plan 1c).
pub async fn move_ticket(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<MoveTicket>,
) -> Result<Json<Ticket>, ApiError> {
    let target = body.target;
    let moved = state
        .with_db(move |db| {
            let Some(current) = db.get_ticket(id)? else {
                return Ok(None);
            };
            let from = current.status;
            db.set_ticket_auto_reviewed(id, false)?;
            db.set_ticket_status(id, target)?;
            let updated = db.get_ticket(id)?.expect("ticket exists; just updated");
            Ok(Some((from, updated)))
        })
        .await?;
    let (from, ticket) = moved.ok_or(ApiError::NotFound)?;
    if from != target {
        state.emit(Event::TicketMoved {
            id,
            from,
            to: target,
            at: chrono::Utc::now().to_rfc3339(),
        });
    }
    Ok(Json(ticket))
}

/// `DELETE /tickets/:id` → remove the ticket from the board. Emits
/// `ticket.deleted`. NOTE: this does not tear down any worktree/zellij session
/// the ticket may have — session cleanup is a Plan 1c concern (`/done`).
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let existed = state
        .with_db(move |db| {
            if db.get_ticket(id)?.is_none() {
                return Ok(false);
            }
            db.delete_ticket(id)?;
            Ok(true)
        })
        .await?;
    if !existed {
        return Err(ApiError::NotFound);
    }
    state.emit(Event::TicketDeleted { id });
    Ok(StatusCode::NO_CONTENT)
}
