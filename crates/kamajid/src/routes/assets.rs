//! Serve the embedded static assets (`/assets/*path`). The Datastar runtime and
//! `app.css` are compiled into the binary via `rust-embed`, so `kamajid` stays a
//! single self-contained binary. Content-type is derived from the extension; a
//! weak ETag from the embedded content hash enables browser caching.

use axum::extract::Path;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "src/assets/"]
struct Assets;

/// `GET /assets/*path` → the embedded file, or 404.
pub async fn serve(Path(path): Path<String>) -> Response {
    match Assets::get(&path) {
        Some(file) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            let etag = format!(
                "\"{:x}\"",
                u128::from_le_bytes(file.metadata.sha256_hash()[..16].try_into().unwrap())
            );
            (
                [
                    (header::CONTENT_TYPE, mime.as_ref().to_string()),
                    (header::ETAG, etag),
                    (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
                ],
                file.data,
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "asset not found").into_response(),
    }
}
