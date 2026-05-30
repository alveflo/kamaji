//! Read the daemon's loaded configuration. (Mutation is deferred to a later
//! phase; the browser does not edit config in Phase 1.)

use axum::extract::State;
use axum::Json;
use kamaji_core::config::Config;

use crate::state::AppState;

/// `GET /config` → the currently loaded config.
pub async fn get_config(State(state): State<AppState>) -> Json<Config> {
    Json((*state.config).clone())
}
