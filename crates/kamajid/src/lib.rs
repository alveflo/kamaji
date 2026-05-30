//! The kamajid daemon: a localhost HTTP API + SSE event stream over
//! `kamaji-core`. `router` builds the axum app from an `AppState`; `serve` runs
//! it on a bound listener. The binary (`main.rs`) wires config, logging, and the
//! TCP bind around these.

pub mod error;
pub mod poll_task;
pub mod routes;
pub mod state;
pub mod zellij_web;

use axum::routing::get;
use axum::Router;

use state::AppState;

/// Build the full router with all routes mounted and the shared state attached.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(routes::healthz::healthz))
        .route("/events", get(routes::events::events))
        .route("/config", get(routes::config::get_config))
        .route(
            "/projects",
            get(routes::projects::list).post(routes::projects::create),
        )
        .route("/projects/:id", get(routes::projects::get_one))
        .route(
            "/projects/:id/tickets",
            get(routes::tickets::list_for_project),
        )
        .route("/tickets", axum::routing::post(routes::tickets::create))
        .route(
            "/tickets/:id",
            get(routes::tickets::get_one)
                .patch(routes::tickets::update)
                .delete(routes::tickets::delete),
        )
        .route(
            "/tickets/:id/move",
            axum::routing::post(routes::tickets::move_ticket),
        )
        .route(
            "/tickets/:id/start",
            axum::routing::post(routes::tickets::start),
        )
        .route(
            "/tickets/:id/done",
            axum::routing::post(routes::tickets::done),
        )
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

/// Serve the router on an already-bound listener until shutdown.
pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> anyhow::Result<()> {
    axum::serve(listener, router(state)).await?;
    Ok(())
}
