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
use axum::extract::ws::{CloseFrame as AxClose, Message as AxMsg, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header, HeaderName, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use futures::{SinkExt, StreamExt};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame as TgClose;
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

/// Translate axum's close frame (raw `u16` code) into tungstenite's typed one.
/// Only the code type differs — the reason `Cow` carries over unchanged.
fn ax_close_to_tg(cf: AxClose<'static>) -> TgClose<'static> {
    TgClose {
        code: CloseCode::from(cf.code),
        reason: cf.reason,
    }
}

/// Translate tungstenite's close frame back into axum's, preserving the code
/// and reason so a clean `1000` close reads differently from an error close.
fn tg_close_to_ax(cf: TgClose<'static>) -> AxClose<'static> {
    AxClose {
        code: cf.code.into(),
        reason: cf.reason,
    }
}

/// Pipe a browser WebSocket to the upstream `zellij web` socket, injecting the
/// auth cookie on the upstream handshake and translating between axum's and
/// tungstenite's message types.
///
/// Both directions are pumped from a single task that `select!`s over the two
/// reads. That lets us (a) answer a `Ping` with a `Pong` on the *same* socket so
/// keepalive survives even a silent peer, and (b) treat a `Close` as a half-close
/// — we forward it, then keep draining the still-open direction until it ends
/// too, instead of dropping a healthy connection the moment either side stops.
/// (Issue #97: a coupled `select!` that ended both directions on either's
/// completion, plus `Close(None)` that discarded the close code, made the inline
/// terminal loop "Reconnecting…".)
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

    // Each direction stops once its *reader* ends (Close, stream end, or error).
    // We finish only when BOTH have, so one side's half-close can't strand the
    // other. `next()` on these split streams is cancel-safe, so dropping the
    // unselected read each loop and re-issuing it next time loses no data.
    let mut client_done = false;
    let mut upstream_done = false;
    while !(client_done && upstream_done) {
        tokio::select! {
            cm = cl_rx.next(), if !client_done => match cm {
                Some(Ok(AxMsg::Text(t))) => {
                    if up_tx.send(TgMsg::Text(t)).await.is_err() {
                        upstream_done = true;
                    }
                }
                Some(Ok(AxMsg::Binary(b))) => {
                    if up_tx.send(TgMsg::Binary(b)).await.is_err() {
                        upstream_done = true;
                    }
                }
                Some(Ok(AxMsg::Ping(b))) => {
                    // Answer the client directly, then forward the ping upstream.
                    let _ = cl_tx.send(AxMsg::Pong(b.clone())).await;
                    let _ = up_tx.send(TgMsg::Ping(b)).await;
                }
                Some(Ok(AxMsg::Pong(b))) => {
                    let _ = up_tx.send(TgMsg::Pong(b)).await;
                }
                Some(Ok(AxMsg::Close(cf))) => {
                    let _ = up_tx.send(TgMsg::Close(cf.map(ax_close_to_tg))).await;
                    client_done = true;
                }
                Some(Err(_)) | None => {
                    let _ = up_tx.send(TgMsg::Close(None)).await;
                    client_done = true;
                }
            },
            um = up_rx.next(), if !upstream_done => match um {
                Some(Ok(TgMsg::Text(t))) => {
                    if cl_tx.send(AxMsg::Text(t)).await.is_err() {
                        client_done = true;
                    }
                }
                Some(Ok(TgMsg::Binary(b))) => {
                    if cl_tx.send(AxMsg::Binary(b)).await.is_err() {
                        client_done = true;
                    }
                }
                Some(Ok(TgMsg::Ping(b))) => {
                    // Answer the upstream directly, then forward the ping on.
                    let _ = up_tx.send(TgMsg::Pong(b.clone())).await;
                    let _ = cl_tx.send(AxMsg::Ping(b)).await;
                }
                Some(Ok(TgMsg::Pong(b))) => {
                    let _ = cl_tx.send(AxMsg::Pong(b)).await;
                }
                Some(Ok(TgMsg::Close(cf))) => {
                    let _ = cl_tx.send(AxMsg::Close(cf.map(tg_close_to_ax))).await;
                    upstream_done = true;
                }
                Some(Ok(TgMsg::Frame(_))) => {}
                Some(Err(_)) | None => {
                    let _ = cl_tx.send(AxMsg::Close(None)).await;
                    upstream_done = true;
                }
            },
            else => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
    use tokio_tungstenite::tungstenite::protocol::CloseFrame;
    use tokio_tungstenite::tungstenite::Message as TgMsg;

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

    // ---- proxy_ws end-to-end tests (issue #97) ----------------------------
    //
    // Topology: a mock `zellij web` upstream (raw tokio-tungstenite server) <->
    // the real proxy router <-> a tokio-tungstenite client. No cookie/login is
    // needed — the proxy injects a cookie only if it holds one, and the mock
    // doesn't check.

    type Up = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

    /// Spawn a mock upstream that runs `handler` on the first accepted socket.
    /// Returns its `http://addr` base (the proxy derives `ws://addr` from it).
    async fn mock_upstream<F, Fut>(handler: F) -> String
    where
        F: FnOnce(Up) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                handler(ws).await;
            }
        });
        format!("http://{addr}")
    }

    /// Boot the proxy router in front of `upstream_http`; return its `ws://addr`.
    async fn proxy_base(upstream_http: &str) -> String {
        let proxy = Arc::new(ZellijProxy::with_upstream(upstream_http));
        let app = router(proxy);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("ws://{addr}")
    }

    /// A Close frame from the upstream must reach the client with its code and
    /// reason intact — not flattened to a code-less close. (A clean 1000 vs a
    /// 1011 error must be distinguishable downstream.)
    #[tokio::test]
    async fn close_frame_preserves_code_and_reason() {
        let up = mock_upstream(|ws| async move {
            let (mut tx, _rx) = ws.split();
            tx.send(TgMsg::Close(Some(CloseFrame {
                code: CloseCode::Away, // 1001
                reason: "bye".into(),
            })))
            .await
            .unwrap();
        })
        .await;
        let base = proxy_base(&up).await;

        let (client, _) = tokio_tungstenite::connect_async(format!("{base}/ws/terminal/x"))
            .await
            .unwrap();
        let (_ctx, mut crx) = client.split();

        let mut frame = None;
        while let Some(Ok(msg)) = crx.next().await {
            if let TgMsg::Close(cf) = msg {
                frame = Some(cf);
                break;
            }
        }
        let cf = frame
            .expect("client should receive a Close")
            .expect("Close should carry a frame (code + reason)");
        assert_eq!(u16::from(cf.code), 1001, "close code preserved");
        assert_eq!(&*cf.reason, "bye", "close reason preserved");
    }

    /// A Ping from the upstream must be answered with a Pong even if the browser
    /// client never responds — the proxy must not depend on the far peer to keep
    /// the near connection alive. (zellij sends no pings today, but a future
    /// keepaliving upstream must not be allowed to time us out.)
    #[tokio::test]
    async fn upstream_ping_is_answered_even_if_client_is_silent() {
        let (tx_pong, rx_pong) = tokio::sync::oneshot::channel::<Vec<u8>>();
        let up = mock_upstream(|ws| async move {
            let (mut tx, mut rx) = ws.split();
            tx.send(TgMsg::Ping(vec![7, 8, 9])).await.unwrap();
            while let Some(Ok(msg)) = rx.next().await {
                if let TgMsg::Pong(b) = msg {
                    let _ = tx_pong.send(b.to_vec());
                    break;
                }
            }
        })
        .await;
        let base = proxy_base(&up).await;

        // Connect but NEVER poll the client: it can't auto-pong, so the only way
        // the upstream gets a Pong is if the proxy answers it directly.
        let _client = tokio_tungstenite::connect_async(format!("{base}/ws/terminal/x"))
            .await
            .unwrap();

        let pong = tokio::time::timeout(Duration::from_secs(3), rx_pong)
            .await
            .expect("upstream should receive a Pong within 3s")
            .expect("pong channel");
        assert_eq!(pong, vec![7, 8, 9], "pong echoes the ping payload");
    }

    /// The rewritten single-task pump must still relay data frames both ways —
    /// upstream→client and client→upstream — without dropping or reordering them.
    /// Guards the core loop against a regression in the message translation.
    #[tokio::test]
    async fn relays_data_in_both_directions() {
        let up = mock_upstream(|ws| async move {
            let (mut tx, mut rx) = ws.split();
            // Greet downstream, then echo the first client message back prefixed.
            tx.send(TgMsg::Text("from-upstream".into())).await.unwrap();
            if let Some(Ok(TgMsg::Text(t))) = rx.next().await {
                let _ = tx.send(TgMsg::Text(format!("echo:{t}"))).await;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        })
        .await;
        let base = proxy_base(&up).await;

        let (client, _) = tokio_tungstenite::connect_async(format!("{base}/ws/terminal/x"))
            .await
            .unwrap();
        let (mut ctx, mut crx) = client.split();

        // upstream -> client
        let greeting = next_text(&mut crx).await;
        assert_eq!(greeting, "from-upstream", "upstream->client relay");

        // client -> upstream -> client (round trip)
        ctx.send(TgMsg::Text("ping123".into())).await.unwrap();
        let echoed = next_text(&mut crx).await;
        assert_eq!(echoed, "echo:ping123", "client->upstream relay");
    }

    /// Read messages off a client stream until the next Text frame, or panic on
    /// a timeout / early close.
    async fn next_text<S>(crx: &mut S) -> String
    where
        S: futures::Stream<Item = Result<TgMsg, tokio_tungstenite::tungstenite::Error>> + Unpin,
    {
        tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(Ok(msg)) = crx.next().await {
                if let TgMsg::Text(t) = msg {
                    return t;
                }
            }
            panic!("stream ended before a Text frame arrived");
        })
        .await
        .expect("a Text frame within 3s")
    }
}
