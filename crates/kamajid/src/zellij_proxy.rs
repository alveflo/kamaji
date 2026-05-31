//! An authenticating reverse proxy in front of `zellij web` (:8082), served on
//! its own listener so the browser board can embed a session in a same-origin
//! iframe with no token prompt.
//!
//! Why a separate listener: the `zellij web` client uses host-relative URLs
//! (`/assets/*`, `/session`, `/command/login`, `/ws/terminal/<s>`, `/ws/control`)
//! that collide with the board's own routes, so it must own an origin. Why a
//! proxy at all: the only way to skip zellij's per-browser token modal is for the
//! iframe's entry request to already carry a valid `session_token` cookie — and
//! only the origin the iframe loads from can set that cookie. So the proxy logs
//! in once server-side (with the `zellij web` token), caches the resulting
//! cookie, and injects it into every upstream request (HTTP and the WebSocket
//! handshake). The browser never needs the cookie or sees the prompt: zellij
//! renders `window.is_authenticated = true` and connects straight through.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::ws::{Message as AxMsg, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header, HeaderName, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use futures::{SinkExt, StreamExt};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as TgMsg;

/// The default base `zellij web` serves on (matches [`crate::zellij_web`]).
const DEFAULT_UPSTREAM_HTTP: &str = "http://127.0.0.1:8082";

/// Reverse proxy state: the upstream `zellij web` bases plus the cached session
/// cookie obtained by logging in once with the token.
pub struct ZellijProxy {
    upstream_http: String,
    upstream_ws: String,
    http: reqwest::Client,
    /// `session_token=<uuid>` — the authenticating cookie, set once.
    cookie: Mutex<Option<String>>,
}

impl ZellijProxy {
    pub fn new() -> Self {
        Self::with_upstream(DEFAULT_UPSTREAM_HTTP)
    }

    /// Build a proxy for a specific upstream base (e.g. `http://127.0.0.1:8082`).
    pub fn with_upstream(upstream_http: &str) -> Self {
        let http = upstream_http.trim_end_matches('/').to_string();
        let ws = http
            .replacen("http://", "ws://", 1)
            .replacen("https://", "wss://", 1);
        ZellijProxy {
            upstream_http: http,
            upstream_ws: ws,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("reqwest client"),
            cookie: Mutex::new(None),
        }
    }

    /// Ensure we hold a `session_token` cookie, logging in with `token` if not.
    /// Idempotent: a cached cookie short-circuits. Best-effort — a failure here
    /// just means the iframe falls back to zellij's own token prompt.
    pub async fn ensure_authenticated(&self, token: &str) -> anyhow::Result<()> {
        {
            let guard = self.cookie.lock().await;
            if guard.is_some() {
                return Ok(());
            }
        }
        let resp = self
            .http
            .post(format!("{}/command/login", self.upstream_http))
            .json(&serde_json::json!({ "auth_token": token, "remember_me": true }))
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("zellij web login failed: {}", resp.status());
        }
        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .ok_or_else(|| anyhow::anyhow!("login response carried no Set-Cookie"))?
            .to_str()?;
        // Keep only the `name=value` pair, dropping attributes (HttpOnly, …).
        let cookie = set_cookie.split(';').next().unwrap_or_default().to_string();
        *self.cookie.lock().await = Some(cookie);
        Ok(())
    }

    async fn cookie(&self) -> Option<String> {
        self.cookie.lock().await.clone()
    }
}

impl Default for ZellijProxy {
    fn default() -> Self {
        Self::new()
    }
}

/// The proxy router (own listener). WebSocket upgrades on `/ws/*` are piped
/// frame-for-frame; everything else is a plain HTTP forward.
pub fn router(proxy: Arc<ZellijProxy>) -> Router {
    Router::new()
        .route("/ws/*rest", any(ws_handler))
        .fallback(any(http_proxy))
        .with_state(proxy)
}

/// Hop-by-hop headers (RFC 7230 §6.1) plus framing headers we must not forward.
fn is_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "content-length"
            | "host"
    )
}

async fn http_proxy(
    State(proxy): State<Arc<ZellijProxy>>,
    req: axum::extract::Request,
) -> Response {
    let (parts, body) = req.into_parts();
    let pq = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let url = format!("{}{}", proxy.upstream_http, pq);
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(_) => return (axum::http::StatusCode::BAD_REQUEST, "bad body").into_response(),
    };

    let mut rb = proxy.http.request(parts.method.clone(), &url);
    for (k, v) in parts.headers.iter() {
        if is_hop(k) || k == header::COOKIE {
            continue;
        }
        rb = rb.header(k, v);
    }
    if let Some(cookie) = proxy.cookie().await {
        rb = rb.header(header::COOKIE, cookie);
    }

    let upstream = match rb.body(bytes).send().await {
        Ok(r) => r,
        Err(e) => {
            return (
                axum::http::StatusCode::BAD_GATEWAY,
                format!("zellij web unreachable: {e}"),
            )
                .into_response()
        }
    };

    let mut builder = Response::builder().status(upstream.status());
    for (k, v) in upstream.headers().iter() {
        // Drop framing headers (axum re-derives them) and upstream Set-Cookie —
        // the proxy owns auth, so the browser never needs the cookie.
        if is_hop(k) || k == header::SET_COOKIE {
            continue;
        }
        builder = builder.header(k, v);
    }
    match upstream.bytes().await {
        Ok(b) => builder.body(Body::from(b)).unwrap_or_else(|_| {
            (axum::http::StatusCode::BAD_GATEWAY, "bad upstream response").into_response()
        }),
        Err(_) => (axum::http::StatusCode::BAD_GATEWAY, "upstream body error").into_response(),
    }
}

async fn ws_handler(
    State(proxy): State<Arc<ZellijProxy>>,
    uri: Uri,
    ws: WebSocketUpgrade,
) -> Response {
    let pq = uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/")
        .to_string();
    ws.on_upgrade(move |client| proxy_ws(proxy, pq, client))
}

/// Pipe a browser WebSocket to the upstream `zellij web` socket, injecting the
/// auth cookie on the upstream handshake and translating between axum's and
/// tungstenite's message types.
async fn proxy_ws(proxy: Arc<ZellijProxy>, pq: String, client: WebSocket) {
    let url = format!("{}{}", proxy.upstream_ws, pq);
    let mut req = match url.into_client_request() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%pq, error = %e, "ws: bad upstream request");
            return;
        }
    };
    if let Some(cookie) = proxy.cookie().await {
        if let Ok(v) = cookie.parse() {
            req.headers_mut().insert("Cookie", v);
        }
    }
    let upstream = match tokio_tungstenite::connect_async(req).await {
        Ok((stream, _resp)) => stream,
        Err(e) => {
            tracing::warn!(%pq, error = %e, "ws: upstream connect failed");
            return;
        }
    };
    let (mut up_tx, mut up_rx) = upstream.split();
    let (mut cl_tx, mut cl_rx) = client.split();

    let client_to_upstream = async {
        while let Some(Ok(msg)) = cl_rx.next().await {
            let out = match msg {
                AxMsg::Text(t) => TgMsg::Text(t),
                AxMsg::Binary(b) => TgMsg::Binary(b),
                AxMsg::Ping(b) => TgMsg::Ping(b),
                AxMsg::Pong(b) => TgMsg::Pong(b),
                AxMsg::Close(_) => {
                    let _ = up_tx.send(TgMsg::Close(None)).await;
                    break;
                }
            };
            if up_tx.send(out).await.is_err() {
                break;
            }
        }
    };
    let upstream_to_client = async {
        while let Some(Ok(msg)) = up_rx.next().await {
            let out = match msg {
                TgMsg::Text(t) => AxMsg::Text(t),
                TgMsg::Binary(b) => AxMsg::Binary(b),
                TgMsg::Ping(b) => AxMsg::Ping(b),
                TgMsg::Pong(b) => AxMsg::Pong(b),
                TgMsg::Close(_) => {
                    let _ = cl_tx.send(AxMsg::Close(None)).await;
                    break;
                }
                TgMsg::Frame(_) => continue,
            };
            if cl_tx.send(out).await.is_err() {
                break;
            }
        }
    };
    tokio::select! {
        _ = client_to_upstream => {}
        _ = upstream_to_client => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_ws_scheme_is_derived_from_http() {
        let p = ZellijProxy::with_upstream("http://127.0.0.1:8082/");
        assert_eq!(p.upstream_http, "http://127.0.0.1:8082");
        assert_eq!(p.upstream_ws, "ws://127.0.0.1:8082");
    }

    #[tokio::test]
    async fn cookie_is_empty_until_authenticated() {
        let p = ZellijProxy::new();
        assert!(p.cookie().await.is_none());
    }
}
