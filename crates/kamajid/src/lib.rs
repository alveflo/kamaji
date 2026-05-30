//! The kamajid daemon: a localhost HTTP API + SSE event stream over
//! `kamaji-core`. `router` builds the axum app from an `AppState`; `serve` runs
//! it on a bound listener. The binary (`main.rs`) wires config, logging, and the
//! TCP bind around these.

pub mod error;
pub mod routes;
pub mod state;

use axum::routing::get;
use axum::Router;

use state::AppState;

/// Build the full router with all routes mounted and the shared state attached.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(routes::healthz::healthz))
        .route("/config", get(routes::config::get_config))
        .route("/projects", get(routes::projects::list))
        .route("/projects/:id", get(routes::projects::get_one))
        .route(
            "/projects/:id/tickets",
            get(routes::tickets::list_for_project),
        )
        .route("/tickets/:id", get(routes::tickets::get_one))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

/// Serve the router on an already-bound listener until shutdown.
pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> anyhow::Result<()> {
    axum::serve(listener, router(state)).await?;
    Ok(())
}
