//! Liveness probe.

use axum::Json;
use serde_json::json;

/// `GET /healthz` → `{ "ok": true, "version": "..." }`. No deep dependency
/// checks — overkill for a localhost daemon (spec §8).
pub async fn healthz() -> Json<serde_json::Value> {
    Json(json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") }))
}
