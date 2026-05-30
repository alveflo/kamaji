//! The auto-review poll task: periodically detect idle agent sessions and move
//! their tickets to Review, broadcasting the resulting events. Reuses
//! `kamaji_core::poll::PollLoop` (the same detection the TUI uses).

use std::path::Path;
use std::time::Duration;

use kamaji_core::db::Db;
use kamaji_core::poll::PollLoop;
use kamaji_core::session;

use crate::state::AppState;

/// Gather every ticket across all projects. (`PollLoop::tick` filters to the
/// in-progress/review ones with a session internally.) DB read failures are
/// logged and skipped — a transient error must not crash the poll loop.
pub(crate) fn all_tickets(db: &Db) -> Vec<kamaji_core::models::Ticket> {
    let mut out = Vec::new();
    match db.list_projects() {
        Ok(projects) => {
            for p in projects {
                match db.list_tickets(p.id) {
                    Ok(tickets) => out.extend(tickets),
                    Err(e) => {
                        tracing::warn!(project_id = p.id, error = %e, "poll: list_tickets failed")
                    }
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "poll: list_projects failed"),
    }
    out
}

/// Reconcile recorded sessions against `sessions` (a `zellij list-sessions`
/// snapshot, or `None` when zellij is unreachable) and broadcast `session.exited`
/// for each ticket whose session vanished. The DB work runs on the blocking pool
/// (mirrors `poll_round`); `sessions` is a parameter so tests can inject a list.
pub async fn reconcile_emit(state: &AppState, state_dir: &Path, sessions: Option<String>) {
    let task_state = state.clone();
    let state_dir = state_dir.to_path_buf();
    let vanished = tokio::task::spawn_blocking(move || {
        let db = task_state.db_handle();
        let db = db.lock().expect("db mutex poisoned");
        let tickets = all_tickets(&db);
        match session::reconcile(&db, &tickets, &state_dir, sessions.as_deref()) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "reconcile: failed to clear vanished sessions");
                Vec::new()
            }
        }
    })
    .await
    .unwrap_or_else(|e| {
        tracing::error!(error = %e, "reconcile spawn_blocking task panicked");
        Vec::new()
    });
    for (id, name) in vanished {
        state.emit(kamaji_core::events::Event::SessionExited {
            ticket_id: id,
            session_name: name,
        });
    }
}

/// Run ONE poll round and return the (mutated) `PollLoop` for the next round:
/// gather tickets, tick the detector, and broadcast the events that fired.
///
/// The DB lock is held while `PollLoop::tick` runs, and tick makes BLOCKING
/// zellij subprocess calls (`list-sessions`, `dump-screen`). So the whole locked
/// section runs on the blocking pool via `spawn_blocking` — exactly like the
/// routes' `with_db` — rather than on an async worker. This keeps blocking OS
/// calls off the async runtime. Public + by-value `poll` so tests can drive
/// rounds deterministically (and so the `'static` `spawn_blocking` closure can
/// own the `PollLoop`).
pub async fn poll_round(state: &AppState, poll: PollLoop, state_dir: &Path) -> PollLoop {
    let task_state = state.clone();
    let state_dir = state_dir.to_path_buf();
    let (poll, events) = tokio::task::spawn_blocking(move || {
        let mut poll = poll;
        let events = {
            let db = task_state.db_handle();
            let db = db.lock().expect("db mutex poisoned");
            let tickets = all_tickets(&db);
            // Re-sync auto-review provenance from the persisted column every round.
            // The DB is the source of truth: a manual move via POST /tickets/:id/move
            // clears `auto_reviewed`, but that route can't reach this task's in-memory
            // PollLoop — so we rehydrate here, otherwise a human-placed card would be
            // dragged back when its agent resumes. (last_level/scrape_hash are NOT
            // touched by rehydrate, so detection history persists across rounds.)
            poll.rehydrate(&tickets);
            match poll.tick(&tickets, &db, &task_state.config, &state_dir) {
                Ok(events) => events,
                Err(e) => {
                    tracing::warn!(error = %e, "poll: tick failed");
                    Vec::new()
                }
            }
        };
        (poll, events)
    })
    .await
    .expect("poll spawn_blocking task panicked");
    for ev in events {
        state.emit(ev);
    }
    poll
}

/// Spawn the background poll loop. Ticks every `interval`; each round re-syncs
/// auto-review provenance from the DB (see [`poll_round`]).
pub fn spawn_poll_task(state: AppState, interval: Duration) {
    let state_dir = state.state_dir().to_path_buf();
    tokio::spawn(async move {
        let mut poll = PollLoop::new();
        // `tokio::time::interval` fires its first tick immediately, so the first
        // poll round runs at startup (establishing baselines), then every `interval`.
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            poll = poll_round(&state, poll, &state_dir).await;
            let sessions = tokio::task::spawn_blocking(kamaji_core::zellij::list_sessions)
                .await
                .unwrap_or(None);
            reconcile_emit(&state, &state_dir, sessions).await;
        }
    });
}
