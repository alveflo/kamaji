//! `GET /ui/events` — the browser SSE stream. Subscribes to the SAME broadcast
//! channel as `routes::events` (the JSON stream for the TUI), but frames each
//! `Event` as a Datastar element-patch SSE record carrying server-rendered HTML.
//! Reuses `views::board::column` and `views::card::card` so a live patch is
//! byte-identical to the initial page render.
//!
//! Datastar wire format (pinned to the vendored v1.0.0-RC.6 runtime):
//!   event: datastar-patch-elements
//!   data: mode remove                     (omitted → default outer morph by id)
//!   data: selector <css>                  (mode remove only)
//!   data: elements <html fragment>        (one line per fragment)

use std::convert::Infallible;

use axum::extract::{Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::stream::{Stream, StreamExt};
use kamaji_core::events::Event;
use kamaji_core::models::{Project, Status, Ticket};
use maud::Markup;
use serde::Deserialize;
use tokio_stream::wrappers::BroadcastStream;

use crate::routes::ui::group_by_status;
use crate::state::AppState;
use crate::views::{board::column, card::card};

const PATCH_EVENT: &str = "datastar-patch-elements";

/// A column re-render, replacing `#col-<status>` by default outer-morph.
fn patch_column(status: Status, tickets: &[Ticket]) -> SseEvent {
    patch_elements(None, &[column(status, tickets)])
}

/// Build the `data:` payload for a `datastar-patch-elements` SSE event. PURE:
/// no I/O, just string assembly, so it can be unit-tested on its wire output.
///
/// `mode` is `None` for the default (outer morph by id), `Some("remove")` etc.
/// `selector` adds a `selector` line (used by `mode remove`). Each `Markup`
/// becomes one `elements` data line (no embedded newlines: maud renders without
/// them). The trailing newline is trimmed; axum's `.data()` writes each
/// `\n`-split line as its own `data:` line.
fn patch_data(mode: Option<&str>, selector: Option<&str>, elements: &[Markup]) -> String {
    let mut data = String::new();
    if let Some(m) = mode {
        data.push_str(&format!("mode {m}\n"));
    }
    if let Some(s) = selector {
        data.push_str(&format!("selector {s}\n"));
    }
    for f in elements {
        data.push_str(&format!("elements {}\n", f.clone().into_string()));
    }
    data.trim_end().to_string()
}

/// Build a `datastar-patch-elements` SSE event. `mode` is `None` for the
/// default (outer morph by id), `Some("remove")` otherwise.
fn patch_elements(mode: Option<&str>, fragments: &[Markup]) -> SseEvent {
    SseEvent::default()
        .event(PATCH_EVENT)
        .data(patch_data(mode, None, fragments))
}

/// Remove `#card-<id>` from the DOM.
fn patch_remove_card(id: i64) -> SseEvent {
    SseEvent::default().event(PATCH_EVENT).data(patch_data(
        Some("remove"),
        Some(&format!("#card-{id}")),
        &[],
    ))
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

/// Render an event into zero or more SSE patch records, scoped to the project
/// the stream is viewing (`viewed`). Events for any other project produce no
/// patches so one browser never sees another project's tickets. Id-only events
/// load the current ticket(s) from `db` (and learn their project that way).
async fn event_to_patches(state: &AppState, ev: Event, viewed: Option<i64>) -> Vec<SseEvent> {
    match ev {
        Event::TicketCreated(t) => {
            if Some(t.project_id) != viewed {
                return Vec::new();
            }
            // Re-render the WHOLE destination column (default outer-morph by
            // `#col-<status>`) instead of appending a single card. This keeps
            // the column-count header correct and makes the patch idempotent
            // (no duplicate `#card-N` under the subscribe/snapshot race),
            // matching every other arm's convergence guarantee.
            let status = t.status;
            let cols = state
                .with_db(move |db| {
                    let all = db.list_tickets(t.project_id)?;
                    let in_col: Vec<Ticket> =
                        all.into_iter().filter(|x| x.status == status).collect();
                    Ok(in_col)
                })
                .await
                .unwrap_or_default();
            vec![patch_column(status, &cols)]
        }
        Event::TicketUpdated(t) => {
            if Some(t.project_id) != viewed {
                return Vec::new();
            }
            vec![patch_elements(None, &[card(&t)])]
        }
        Event::TicketMoved { id, from, to, .. } => {
            // Re-render BOTH affected columns (fixes counts + relocates the
            // card). Load the moving ticket to learn its project, then list
            // that project's tickets once. A move in another project is ignored.
            let cols = state
                .with_db(move |db| {
                    let Some(t) = db.get_ticket(id)? else {
                        return Ok(Vec::new());
                    };
                    if Some(t.project_id) != viewed {
                        return Ok(Vec::new());
                    }
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
        // A deleted ticket carries no project, but its `#card-<id>` only exists
        // on the board of the project it belonged to; the remove selector is a
        // no-op on any other project's board, so emit it unconditionally.
        Event::TicketDeleted { id } => vec![patch_remove_card(id)],
        Event::SessionStarted { ticket_id, .. }
        | Event::SessionIdle { ticket_id }
        | Event::SessionExited { ticket_id, .. } => match load_ticket(state, ticket_id).await {
            Some(t) if Some(t.project_id) == viewed => vec![patch_elements(None, &[card(&t)])],
            _ => Vec::new(),
        },
        // The TUI's per-session activity bullet rides this event; the browser's
        // card chip is derived from the ticket's column, so there is nothing to
        // re-render here.
        Event::SessionSignal { .. } => Vec::new(),
    }
}

/// The project a board SSE stream is scoped to: `want` (`?project=<id>`) if it
/// exists, else the first project — mirroring `routes::ui::board`'s selection so
/// the live stream matches the page render. `None` when the requested id is
/// unknown or there are no projects (an empty board).
fn resolve_project_id(want: Option<i64>, projects: &[Project]) -> Option<i64> {
    match want {
        Some(id) => projects.iter().find(|p| p.id == id).map(|p| p.id),
        None => projects.first().map(|p| p.id),
    }
}

/// Query for `GET /ui/events`: `?project=<id>` scopes the stream to that board.
#[derive(Deserialize)]
pub struct EventsQuery {
    pub project: Option<i64>,
}

/// `GET /ui/events` → Datastar element-patch SSE, scoped to `?project=<id>` (the
/// board the page is showing). On connect, emit a one-shot full-board patch
/// (re-render all four columns) so every (re)connection self-heals (§4.4), then
/// stream live patches off the broadcast — both filtered to the viewed project.
pub async fn events(
    State(state): State<AppState>,
    Query(q): Query<EventsQuery>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    // Subscribe FIRST so no event between the snapshot read and the live
    // subscription is missed.
    let rx = state.tx.subscribe();

    // Resolve which project this stream serves once, up front; the snapshot and
    // every live patch are filtered to it so a browser never sees another
    // project's tickets (the snapshot used to hardcode the first project).
    let want = q.project;
    let viewed = state
        .with_db(move |db| Ok(resolve_project_id(want, &db.list_projects()?)))
        .await
        .unwrap_or(None);

    // Full-board snapshot: render every column for the viewed project.
    let snapshot = {
        let by = state
            .with_db(move |db| {
                let tickets = match viewed {
                    Some(pid) => db.list_tickets(pid)?,
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
                        event_to_patches(&state, ev, viewed)
                            .await
                            .into_iter()
                            .map(Ok),
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
    use kamaji_core::config::Config;
    use kamaji_core::db::Db;
    use kamaji_core::models::{Agent, Project};
    use std::path::{Path, PathBuf};

    fn project(id: i64, name: &str) -> Project {
        Project {
            id,
            name: name.into(),
            root_dir: PathBuf::from("/tmp/p"),
            default_agent: Some(Agent::Claude),
            created_at: String::new(),
        }
    }

    fn ticket(id: i64, status: Status) -> Ticket {
        Ticket {
            id,
            project_id: 1,
            title: format!("t{id}"),
            description: String::new(),
            initial_prompt: None,
            agent: Agent::Claude,
            status,
            session_name: None,
            worktree_path: None,
            branch: None,
            auto_reviewed: false,
            instrumented: false,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    /// A column patch is a default outer-morph (no `mode` line) carrying the
    /// rendered `#col-<status>` element. Asserts on the PURE `patch_data` wire
    /// output that the real constructors share.
    #[test]
    fn column_patch_targets_col_id() {
        let data = patch_data(
            None,
            None,
            &[column(Status::Review, &[ticket(1, Status::Review)])],
        );
        assert!(
            data.starts_with("elements "),
            "outer-morph patch must lead with `elements`: {data:?}"
        );
        assert!(
            !data.contains("mode "),
            "outer-morph patch must omit `mode`: {data:?}"
        );
        assert!(data.contains(r#"id="col-review""#), "{data:?}");
        assert!(data.contains("card-1"), "{data:?}");
    }

    /// A remove patch is exactly `mode remove\nselector #card-<id>` with no
    /// `elements` line.
    #[test]
    fn remove_card_patch_uses_remove_mode_and_selector() {
        let data = patch_data(Some("remove"), Some("#card-7"), &[]);
        assert_eq!(data, "mode remove\nselector #card-7");
    }

    /// An outer-morph card patch (TicketUpdated / session events) carries the
    /// card markup with no `mode`/`selector` line.
    #[test]
    fn card_patch_is_outer_morph() {
        let data = patch_data(None, None, &[card(&ticket(1, Status::Todo))]);
        assert!(!data.contains("mode "), "{data:?}");
        assert!(!data.contains("selector "), "{data:?}");
        assert!(data.contains("card-1"), "{data:?}");
    }

    /// The connection snapshot must honor `?project=<id>`; only when it is
    /// absent does it fall back to the first project. Returning the FIRST project
    /// unconditionally was the cross-project ticket-leak bug.
    #[test]
    fn resolve_project_id_honors_request_else_first() {
        let ps = vec![project(1, "a"), project(2, "b"), project(3, "c")];
        assert_eq!(resolve_project_id(Some(2), &ps), Some(2), "requested");
        assert_eq!(resolve_project_id(None, &ps), Some(1), "default = first");
        assert_eq!(resolve_project_id(Some(99), &ps), None, "unknown id");
        assert_eq!(resolve_project_id(None, &[]), None, "no projects");
    }

    /// Live patches must be scoped to the project the browser is viewing: an
    /// event for another project must produce no patches, while the same event
    /// for the viewed project must patch the board.
    #[tokio::test]
    async fn live_events_are_scoped_to_the_viewed_project() {
        let db = Db::open_in_memory().unwrap();
        let pa = db.create_project("a", Path::new("/tmp/a"), None).unwrap();
        let pb = db.create_project("b", Path::new("/tmp/b"), None).unwrap();
        let t = db
            .create_ticket(pa.id, "t", "", None, Agent::Claude)
            .unwrap();
        let state = AppState::new(db, Config::default());

        // Viewing project B: a ticket created in A must not touch the board.
        let other = event_to_patches(&state, Event::TicketCreated(t.clone()), Some(pb.id)).await;
        assert!(other.is_empty(), "cross-project create must not patch");

        // Viewing project A: the same event re-renders A's column.
        let same = event_to_patches(&state, Event::TicketCreated(t), Some(pa.id)).await;
        assert!(!same.is_empty(), "same-project create must patch");
    }
}
