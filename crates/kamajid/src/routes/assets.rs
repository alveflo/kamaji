//! Serve the embedded static assets (`/assets/*path`). The Datastar runtime and
//! the stylesheets (`tokens.css`, `layout.css`, etc.) are compiled into the
//! binary via `rust-embed`, so `kamajid` stays a single self-contained binary.
//! Content-type is derived from the extension; an ETag from the embedded content
//! hash drives conditional requests.
//!
//! Assets are served `Cache-Control: no-cache` — i.e. the browser may cache but
//! must revalidate every load. It sends `If-None-Match` with the cached ETag; we
//! answer `304 Not Modified` when unchanged (cheap) or fresh bytes when the
//! embedded asset changed. This means a daemon restart with new code shows the
//! new `tokens.css`/`datastar.js` on a normal reload — no hard-refresh needed (#96).
//! A bare `max-age` (the old behavior) let the browser skip revalidation and
//! serve a stale asset for up to a day.

use axum::extract::Path;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "src/assets/"]
struct Assets;

/// `GET /assets/*path` → the embedded file (200 or 304), or 404.
pub async fn serve(Path(path): Path<String>, headers: HeaderMap) -> Response {
    match Assets::get(&path) {
        Some(file) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            let etag = format!(
                "\"{:x}\"",
                u128::from_le_bytes(file.metadata.sha256_hash()[..16].try_into().unwrap())
            );

            // Conditional request: if the browser's cached copy is current,
            // skip the bytes and return 304.
            let if_none_match = headers
                .get(header::IF_NONE_MATCH)
                .and_then(|v| v.to_str().ok());
            if if_none_match.is_some_and(|inm| etag_matches(inm, &etag)) {
                return (
                    StatusCode::NOT_MODIFIED,
                    [
                        (header::ETAG, etag),
                        (header::CACHE_CONTROL, "no-cache".to_string()),
                    ],
                )
                    .into_response();
            }

            (
                [
                    (header::CONTENT_TYPE, mime.as_ref().to_string()),
                    (header::ETAG, etag),
                    (header::CACHE_CONTROL, "no-cache".to_string()),
                ],
                file.data,
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "asset not found").into_response(),
    }
}

/// True if an `If-None-Match` header value matches our ETag. Handles `*`, a
/// comma-separated list of tags, and the `W/` weak-validator prefix.
fn etag_matches(if_none_match: &str, etag: &str) -> bool {
    if_none_match.split(',').any(|candidate| {
        let candidate = candidate.trim().trim_start_matches("W/");
        candidate == "*" || candidate == etag
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn header_value(resp: &Response, name: header::HeaderName) -> Option<String> {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }

    async fn body_len(resp: Response) -> usize {
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .len()
    }

    #[tokio::test]
    async fn serves_bytes_with_revalidating_cache_control() {
        let resp = serve(Path("tokens.css".to_string()), HeaderMap::new()).await;

        assert_eq!(resp.status(), StatusCode::OK);
        // `no-cache` forces the browser to revalidate every load, so a daemon
        // restart with new assets is picked up without a hard-refresh (#96).
        assert_eq!(
            header_value(&resp, header::CACHE_CONTROL).as_deref(),
            Some("no-cache")
        );
        assert!(header_value(&resp, header::ETAG).is_some(), "etag present");
        assert!(body_len(resp).await > 0, "full bytes served");
    }

    #[tokio::test]
    async fn unchanged_asset_returns_304_without_body() {
        // First load to learn the current ETag.
        let first = serve(Path("tokens.css".to_string()), HeaderMap::new()).await;
        let etag = header_value(&first, header::ETAG).expect("etag");

        // Reload with that ETag → 304, no bytes on the wire.
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_str(&etag).unwrap());
        let resp = serve(Path("tokens.css".to_string()), headers).await;

        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(
            header_value(&resp, header::ETAG).as_deref(),
            Some(etag.as_str()),
            "304 echoes the etag"
        );
        assert_eq!(body_len(resp).await, 0, "no body on 304");
    }

    #[tokio::test]
    async fn changed_asset_returns_full_bytes() {
        // A stale ETag (asset changed since the browser cached it) → 200 + bytes.
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("\"deadbeef\""),
        );
        let resp = serve(Path("tokens.css".to_string()), headers).await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert!(body_len(resp).await > 0, "fresh bytes served");
    }

    #[tokio::test]
    async fn missing_asset_is_404() {
        let resp = serve(Path("nope.txt".to_string()), HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn etag_matches_handles_list_wildcard_and_weak_prefix() {
        assert!(etag_matches("\"abc\"", "\"abc\""));
        assert!(etag_matches("\"x\", \"abc\"", "\"abc\""));
        assert!(etag_matches("W/\"abc\"", "\"abc\""));
        assert!(etag_matches("*", "\"abc\""));
        assert!(!etag_matches("\"other\"", "\"abc\""));
    }
}
