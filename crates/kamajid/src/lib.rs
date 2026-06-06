//! The kamajid daemon: a localhost HTTP API + SSE event stream over
//! `kamaji-core`. `router` builds the axum app from an `AppState`; `serve` runs
//! it on a bound listener. The binary (`main.rs`) wires config, logging, and the
//! TCP bind around these.

pub mod error;
pub mod poll_task;
pub mod routes;
pub mod session_driver;
pub mod state;
pub mod views;
pub mod zellij_proxy;
pub mod zellij_web;

use axum::routing::get;
use axum::Router;

use state::AppState;

/// Build the full router with all routes mounted and the shared state attached.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(routes::ui::board))
        .route("/healthz", get(routes::healthz::healthz))
        .route("/events", get(routes::events::events))
        .route("/ui/events", get(routes::ui_events::events))
        .route("/ui/tickets/new", get(routes::ui::new_ticket))
        .route("/ui/tickets/cancel", get(routes::ui::cancel_ticket))
        .route("/ui/projects/new", get(routes::ui::new_project))
        .route("/ui/tickets/:id/edit", get(routes::ui::edit_ticket))
        .route("/ui/tickets/:id/terminal", get(routes::ui::terminal))
        .route("/ui/sessions/manage", get(routes::ui::manage_sessions))
        .route(
            "/config",
            get(routes::config::get_config).patch(routes::config::patch_config),
        )
        .route(
            "/projects",
            get(routes::projects::list).post(routes::projects::create),
        )
        .route("/projects/:id", get(routes::projects::get_one))
        .route(
            "/projects/:id/main-session",
            axum::routing::post(routes::projects::main_session),
        )
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
        .route(
            "/tickets/:id/attach",
            axum::routing::post(routes::tickets::attach),
        )
        .route(
            "/sessions/delete",
            axum::routing::post(routes::sessions::delete),
        )
        .route("/assets/*path", get(routes::assets::serve))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

/// Serve the router on an already-bound listener until shutdown.
pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> anyhow::Result<()> {
    axum::serve(listener, router(state)).await?;
    Ok(())
}

/// The authenticating `zellij web` reverse proxy router (served on its own
/// listener — see [`crate::zellij_proxy`]).
pub fn proxy_router(state: &AppState) -> Router {
    zellij_proxy::router(state.zellij_proxy())
}

/// Serve the proxy router on its own bound listener until shutdown.
pub async fn serve_proxy(listener: tokio::net::TcpListener, state: AppState) -> anyhow::Result<()> {
    axum::serve(listener, proxy_router(&state)).await?;
    Ok(())
}
