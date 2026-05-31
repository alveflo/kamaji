//! HTML-serving routes: the board page (`GET /`) and the create/edit modal
//! fragments (`GET /ui/tickets/new`, `GET /ui/tickets/:id/edit`, added in 3e).
//! Read/render only — all mutations reuse the existing JSON command API.

use axum::extract::{Path, Query, State};
use maud::Markup;
use serde::Deserialize;

use kamaji_core::models::{Status, Ticket};

use crate::error::ApiError;
use crate::state::AppState;
use crate::views;
use crate::views::modal::ticket_form;

#[derive(Deserialize)]
pub struct BoardQuery {
    pub project: Option<i64>,
}

/// `GET /` → the full board page. `?project=<id>` selects the project; absent,
/// the first project is used. 404 (rendered as an error) if there are none.
pub async fn board(
    State(state): State<AppState>,
    Query(q): Query<BoardQuery>,
) -> Result<Markup, ApiError> {
    let want = q.project;
    let (projects, project, by_status) = state
        .with_db(move |db| {
            let projects = db.list_projects()?;
            let Some(project) = (match want {
                Some(id) => projects.iter().find(|p| p.id == id).cloned(),
                None => projects.first().cloned(),
            }) else {
                return Ok(None);
            };
            let tickets = db.list_tickets(project.id)?;
            let by_status = group_by_status(tickets);
            Ok(Some((projects, project, by_status)))
        })
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(views::page::page(&project, &projects, &by_status))
}

/// Partition a project's tickets into `Status::all()` order — the shape both
/// `views::board::board` and the SSE serializer consume.
pub fn group_by_status(tickets: Vec<Ticket>) -> Vec<(Status, Vec<Ticket>)> {
    Status::all()
        .into_iter()
        .map(|s| {
            (
                s,
                tickets.iter().filter(|t| t.status == s).cloned().collect(),
            )
        })
        .collect()
}

// modal fragment handlers added in 3e

#[derive(Deserialize)]
pub struct NewTicketQuery {
    pub project: i64,
}

/// `GET /ui/tickets/new?project=<id>` → the create-ticket modal fragment.
pub async fn new_ticket(
    State(state): State<AppState>,
    Query(q): Query<NewTicketQuery>,
) -> Result<Markup, ApiError> {
    let pid = q.project;
    let project_default = state
        .with_db(move |db| Ok(db.get_project(pid)?.and_then(|p| p.default_agent)))
        .await?;
    let default_agent = match project_default {
        Some(a) => a,
        None => state.config_async().await.default_agent(),
    };
    Ok(ticket_form(pid, None, default_agent, None))
}

/// `GET /ui/tickets/:id/edit` → the edit-ticket modal fragment, prefilled.
pub async fn edit_ticket(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Markup, ApiError> {
    let ticket = state
        .with_db(move |db| db.get_ticket(id))
        .await?
        .ok_or(ApiError::NotFound)?;
    let default_agent = ticket.agent;
    Ok(ticket_form(
        ticket.project_id,
        Some(&ticket),
        default_agent,
        None,
    ))
}
