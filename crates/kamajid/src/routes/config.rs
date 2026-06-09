//! Read and edit the daemon's loaded configuration.
//!
//! - `GET /config` → the currently loaded config.
//! - `PATCH /config` → partial edit (TUI path): replaces only the present
//!   fields, persists, updates the in-memory copy.
//! - `PUT /config` → full-replace (web editor path): validates the complete
//!   body before persisting; returns 400 with a human message on validation
//!   failure.

use axum::extract::State;
use axum::Json;
use kamaji_core::config::Config;
use kamaji_core::models::Agent;
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::AppState;

/// `GET /config` → the currently loaded config.
pub async fn get_config(State(state): State<AppState>) -> Json<Config> {
    Json(state.config_async().await)
}

/// Partial config edit: any present field replaces its current value; omitted
/// fields are kept. Only the TUI-editable fields are accepted (theme,
/// default_agent, worktree_base). Persisted to config.toml + held in memory.
#[derive(Deserialize)]
pub struct PatchConfig {
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub default_agent: Option<String>,
    #[serde(default)]
    pub worktree_base: Option<String>,
}

/// `PATCH /config` → apply a partial edit and return the updated config.
pub async fn patch_config(
    State(state): State<AppState>,
    Json(body): Json<PatchConfig>,
) -> Result<Json<Config>, ApiError> {
    if let Some(ref a) = body.default_agent {
        a.parse::<Agent>()
            .map_err(|e| ApiError::BadRequest(format!("invalid default_agent: {e}")))?;
    }
    let mut guard = state.config.write().await;
    if let Some(t) = body.theme {
        guard.theme = t;
    }
    if let Some(a) = body.default_agent {
        guard.default_agent = a;
    }
    if let Some(w) = body.worktree_base {
        guard.worktree_base = Some(w);
    }
    let updated = guard.clone();
    drop(guard);

    let path = kamaji_core::config::config_path().map_err(ApiError::Internal)?;
    let to_save = updated.clone();
    tokio::task::spawn_blocking(move || kamaji_core::config::save_to(&path, &to_save))
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("config save task panicked: {e}")))?
        .map_err(ApiError::Internal)?;
    Ok(Json(updated))
}

/// `PUT /config` → full-replace the daemon's config from a complete `Config`
/// body. Used by the web config editor, which renders every field from a prior
/// `GET /config`. Validated (`Config::validate`) before persisting; on failure
/// returns 400 with a human message the modal shows inline. `PATCH /config`
/// remains the TUI's partial-edit path and is unaffected.
pub async fn put_config(
    State(state): State<AppState>,
    Json(body): Json<Config>,
) -> Result<Json<Config>, ApiError> {
    body.validate().map_err(ApiError::BadRequest)?;

    {
        let mut guard = state.config.write().await;
        *guard = body.clone();
    }

    let path = kamaji_core::config::config_path().map_err(ApiError::Internal)?;
    let to_save = body.clone();
    tokio::task::spawn_blocking(move || kamaji_core::config::save_to(&path, &to_save))
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("config save task panicked: {e}")))?
        .map_err(ApiError::Internal)?;
    Ok(Json(body))
}
