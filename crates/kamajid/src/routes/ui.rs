//! HTML-serving routes: the board page (`GET /`) and the create/edit modal
//! fragments (`GET /ui/tickets/new`, `GET /ui/tickets/:id/edit`, added in 3e).
//! Read/render only — all mutations reuse the existing JSON command API.

use axum::extract::{Path, Query, State};
use maud::Markup;
use serde::Deserialize;

use kamaji_core::models::{Status, Ticket};
use kamaji_core::session;

use crate::error::ApiError;
use crate::state::AppState;
use crate::views;
use crate::views::confirm::{delete_done_confirm, ticket_confirm, ConfirmAction};
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

/// `GET /ui/tickets/cancel` → the empty `#modal` mount. Wired to the modal's
/// Cancel button; the morph clears the dialog without touching the JSON command
/// API. (A successful submit closes the modal client-side; see `views::modal`.)
pub async fn cancel_ticket() -> Markup {
    views::modal::modal_closed()
}

#[derive(Deserialize)]
pub struct ConfirmQuery {
    pub action: ConfirmAction,
}

/// `GET /ui/tickets/:id/confirm?action=done|delete` → the in-page confirmation
/// modal fragment for a destructive card action (replacing the browser's native
/// `confirm()`). Render-only: the actual command fires from the modal's Confirm
/// button. An unknown `action` fails query deserialization → 400.
pub async fn confirm_ticket(Path(id): Path<i64>, Query(q): Query<ConfirmQuery>) -> Markup {
    ticket_confirm(id, q.action)
}

/// `GET /ui/projects/:id/confirm-delete-done` → the confirmation modal for
/// clearing the project's whole Done column. Render-only: the bulk
/// `DELETE /projects/:id/done-tickets` fires from the modal's Confirm button.
pub async fn confirm_delete_done(Path(project_id): Path<i64>) -> Markup {
    delete_done_confirm(project_id)
}

/// `GET /ui/projects/new` → the create-project modal fragment. There is no
/// project yet, so the segmented agent control defaults to the config default.
/// Submit reuses `POST /projects`; on success the page navigates to the new
/// project so the rail shows its tile (projects broadcast no SSE event).
pub async fn new_project(State(state): State<AppState>) -> Markup {
    let default_agent = state.config_async().await.default_agent();
    views::project_form::project_form(default_agent, None)
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

/// `GET /ui/tickets/:id/terminal` → the inline terminal panel. Ensures the
/// ticket's session is live (resurrecting if needed) and `zellij web` is up,
/// pre-authenticates the reverse proxy, then morphs a near-fullscreen iframe of
/// the session over `#modal`. Same ensure path as `POST /tickets/:id/attach`.
pub async fn terminal(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Markup, ApiError> {
    let (ticket, info) = crate::routes::tickets::ensure_attach(&state, id).await?;
    // Seed the proxy's auth cookie so the iframe loads with no token prompt.
    // Best-effort: if it fails the iframe falls back to zellij's own prompt.
    if let Err(e) = state.zellij_proxy().ensure_authenticated(&info.token).await {
        tracing::warn!(ticket = id, error = %e, "proxy pre-auth failed; iframe may prompt");
    }
    let src = format!("{}/{}", state.proxy_base(), info.session_name);
    Ok(views::terminal::terminal_panel(
        ticket.id,
        &ticket.title,
        ticket.agent,
        &info.session_name,
        &src,
    ))
}

/// `GET /ui/sessions/manage` → the session-cleanup modal fragment. Snapshots
/// `zellij list-sessions` (through the session-driver seam, off the async
/// runtime since it shells out), classifies each `kamaji-*` session against the
/// DB, and renders the modal. When zellij can't be queried the modal shows an
/// explanatory note rather than erroring.
pub async fn manage_sessions(State(state): State<AppState>) -> Result<Markup, ApiError> {
    let st = state.clone();
    let list = tokio::task::spawn_blocking(move || st.sessions().list_sessions())
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("list-sessions task panicked: {e}")))?;
    let reachable = list.is_some();
    let entries = match list {
        Some(list) => {
            state
                .with_db(move |db| session::classify_sessions(db, &list))
                .await?
        }
        None => Vec::new(),
    };
    Ok(views::sessions::sessions_modal(&entries, reachable))
}
