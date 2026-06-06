//! PWA plumbing: serve the web app manifest and service worker from dedicated
//! routes. Both reuse the embedded `Assets` folder (see `routes::assets`).
//!
//! The manifest needs an explicit `application/manifest+json` content type
//! (`mime_guess` does not map `.webmanifest`). The service worker MUST be served
//! at the site root (`/sw.js`) so its default scope (`/`) covers the board page.

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::routes::assets::Assets;

/// `GET /manifest.webmanifest` → the embedded manifest, correctly typed.
pub async fn manifest() -> Response {
    match Assets::get("manifest.webmanifest") {
        Some(file) => (
            [(header::CONTENT_TYPE, "application/manifest+json")],
            file.data,
        )
            .into_response(),
        None => (StatusCode::INTERNAL_SERVER_ERROR, "manifest missing").into_response(),
    }
}

/// `GET /sw.js` → the embedded service worker. Served at root for `/` scope.
///
/// NOTE: throwaway stub — Task 3 replaces this with the real headers and wires
/// the `/sw.js` route. Not wired yet; only here so the module compiles.
pub async fn service_worker() -> Response {
    match Assets::get("sw.js") {
        Some(file) => ([(header::CONTENT_TYPE, "text/javascript")], file.data).into_response(),
        None => (StatusCode::INTERNAL_SERVER_ERROR, "sw missing").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_bytes(resp: Response) -> Vec<u8> {
        to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    fn content_type(resp: &Response) -> String {
        resp.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    #[tokio::test]
    async fn manifest_route_serves_manifest_json() {
        let resp = manifest().await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(content_type(&resp), "application/manifest+json");

        let body = body_bytes(resp).await;
        let v: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        assert_eq!(v["name"], "kamaji");
        assert_eq!(v["start_url"], "/");
        assert!(
            ["standalone", "fullscreen", "minimal-ui"].contains(&v["display"].as_str().unwrap()),
            "display must be an installable mode"
        );
        let sizes: Vec<&str> = v["icons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["sizes"].as_str().unwrap())
            .collect();
        assert!(sizes.contains(&"192x192"), "192 icon present");
        assert!(sizes.contains(&"512x512"), "512 icon present");
    }
}
