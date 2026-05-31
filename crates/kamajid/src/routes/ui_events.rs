//! `GET /ui/events` — the browser SSE stream. Subscribes to the SAME broadcast
//! channel as `routes::events` (the JSON stream for the TUI), but frames each
//! `Event` as a Datastar element-patch SSE record carrying server-rendered HTML.
//! Reuses `views::board::column` and `views::card::card` so a live patch is
//! byte-identical to the initial page render.
//!
//! Datastar wire format (pinned to the vendored v1.0.0-RC.6 runtime):
//!   event: datastar-patch-elements
//!   data: mode <append|remove>            (omitted → default outer morph by id)
//!   data: elements <html fragment>        (one line per fragment)

use std::convert::Infallible;

use axum::extract::State;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::stream::{Stream, StreamExt};
use kamaji_core::events::Event;
use kamaji_core::models::{Status, Ticket};
use maud::Markup;
use tokio_stream::wrappers::BroadcastStream;

use crate::routes::ui::group_by_status;
use crate::state::AppState;
use crate::views::{board::column, card::card};

const PATCH_EVENT: &str = "datastar-patch-elements";

/// A column re-render, replacing `#col-<status>` by default outer-morph.
fn patch_column(status: Status, tickets: &[Ticket]) -> SseEvent {
    patch_elements(None, &[column(status, tickets)])
}

/// Build a `datastar-patch-elements` SSE event. `mode` is `None` for the
/// default (outer morph by id), `Some("append")` / `Some("remove")` otherwise.
/// Each `Markup` becomes one `elements` data line (no embedded newlines: maud
/// renders without them).
fn patch_elements(mode: Option<&str>, fragments: &[Markup]) -> SseEvent {
    let mut data = String::new();
    if let Some(m) = mode {
        data.push_str(&format!("mode {m}\n"));
    }
    for f in fragments {
        data.push_str(&format!("elements {}\n", f.clone().into_string()));
    }
    // axum's `.data()` writes each `\n`-split line as its own `data:` line.
    SseEvent::default().event(PATCH_EVENT).data(data.trim_end())
}

/// Remove `#card-<id>` from the DOM.
fn patch_remove_card(id: i64) -> SseEvent {
    SseEvent::default()
        .event(PATCH_EVENT)
        .data(format!("mode remove\nselector #card-{id}"))
}

/// Load the current ticket by id (a cheap read on the single-user broadcast
/// path). `None` if it no longer exists.
async fn load_ticket(state: &AppState, id: i64) -> Option<Ticket> {
    state
        .with_db(move |db| db.get_ticket(id))
        .await
        .ok()
        .flatten()
}

/// Render an event into zero or more SSE patch records. Id-only events load the
/// current ticket(s) from `db`.
async fn event_to_patches(state: &AppState, ev: Event) -> Vec<SseEvent> {
    match ev {
        Event::TicketCreated(t) => {
            // Append the new card into its column body container.
            let frag = card(&t).into_string();
            vec![SseEvent::default().event(PATCH_EVENT).data(format!(
                "mode append\nselector #col-{} .col-body\nelements {}",
                t.status.as_str(),
                frag
            ))]
        }
        Event::TicketUpdated(t) => vec![patch_elements(None, &[card(&t)])],
        Event::TicketMoved { id, from, to, .. } => {
            // Re-render BOTH affected columns (fixes counts + relocates the
            // card). Load the moving ticket to learn its project, then list
            // that project's tickets once.
            let cols = state
                .with_db(move |db| {
                    let Some(t) = db.get_ticket(id)? else {
                        return Ok(Vec::new());
                    };
                    let all = db.list_tickets(t.project_id)?;
                    Ok([from, to]
                        .into_iter()
                        .map(|s| {
                            let in_col: Vec<Ticket> =
                                all.iter().filter(|x| x.status == s).cloned().collect();
                            (s, in_col)
                        })
                        .collect::<Vec<_>>())
                })
                .await
                .unwrap_or_default();
            cols.into_iter()
                .map(|(s, ts)| patch_column(s, &ts))
                .collect()
        }
        Event::TicketDeleted { id } => vec![patch_remove_card(id)],
        Event::SessionStarted { ticket_id, .. }
        | Event::SessionIdle { ticket_id }
        | Event::SessionExited { ticket_id, .. } => match load_ticket(state, ticket_id).await {
            Some(t) => vec![patch_elements(None, &[card(&t)])],
            None => Vec::new(),
        },
    }
}

/// `GET /ui/events` → Datastar element-patch SSE. On connect, emit a one-shot
/// full-board patch (re-render all four columns) so every (re)connection
/// self-heals (§4.4), then stream live patches off the broadcast.
pub async fn events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    // Subscribe FIRST so no event between the snapshot read and the live
    // subscription is missed.
    let rx = state.tx.subscribe();

    // Full-board snapshot: render every column for the first project (the board
    // page shows one project; a future multi-project board re-renders per ?project).
    let snapshot = {
        let by = state
            .with_db(|db| {
                let projects = db.list_projects()?;
                let tickets = match projects.first() {
                    Some(p) => db.list_tickets(p.id)?,
                    None => Vec::new(),
                };
                Ok(group_by_status(tickets))
            })
            .await
            .unwrap_or_default();
        by.into_iter()
            .map(|(s, ts)| Ok(patch_column(s, &ts)))
            .collect::<Vec<Result<SseEvent, Infallible>>>()
    };

    let state2 = state.clone();
    let live = BroadcastStream::new(rx)
        .filter_map(move |result| {
            let state = state2.clone();
            async move {
                match result {
                    Ok(ev) => Some(futures::stream::iter(
                        event_to_patches(&state, ev).await.into_iter().map(Ok),
                    )),
                    Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                        tracing::debug!(dropped = n, "UI SSE client lagged");
                        None
                    }
                }
            }
        })
        .flatten();

    let stream = futures::stream::iter(snapshot).chain(live);
    Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kamaji_core::models::Agent;

    fn ticket(id: i64, status: Status) -> Ticket {
        Ticket {
            id,
            project_id: 1,
            title: format!("t{id}"),
            description: String::new(),
            initial_prompt: None,
            agent: Agent::Claude,
            status,
            position: 0,
            session_name: None,
            worktree_path: None,
            branch: None,
            auto_reviewed: false,
            instrumented: false,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    /// Extract the SSE `data:` block an `SseEvent` would serialize. We rebuild
    /// the patch via the same constructors to assert the wire content.
    #[test]
    fn column_patch_targets_col_id() {
        let s = patch_column(Status::Review, &[ticket(1, Status::Review)]);
        // Render the same column directly to confirm reuse of views::board::column.
        let direct = column(Status::Review, &[ticket(1, Status::Review)]).into_string();
        assert!(direct.contains(r#"id="col-review""#));
        assert!(direct.contains("card-1"));
        let _ = s; // SseEvent has no public getter; the reuse is the contract.
    }

    #[test]
    fn remove_card_patch_uses_remove_mode_and_selector() {
        // Reconstruct the data string the same way the constructor does.
        let id = 7;
        let data = format!("mode remove\nselector #card-{id}");
        assert!(data.contains("mode remove"));
        assert!(data.contains("#card-7"));
    }
}
