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
//!
//! The proxy also rewrites a couple of zellij's served JS assets in flight: the
//! reconnect module (see [`RECONNECT_SHIM`]) and `terminal.js`, into which it
//! splices a bundled Nerd Font and (when `web_theme = "match"`) the board's
//! palette (see [`rewrite_terminal_js`]) so the embedded xterm terminal renders
//! icon glyphs instead of tofu and matches the board's colors.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::ws::{CloseFrame as AxClose, Message as AxMsg, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header, HeaderName, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::Router;
use futures::{SinkExt, StreamExt};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame as TgClose;
use tokio_tungstenite::tungstenite::Message as TgMsg;

/// The default base `zellij web` serves on (matches [`crate::zellij_web`]).
const DEFAULT_UPSTREAM_HTTP: &str = "http://127.0.0.1:8082";

/// HTTP requests to zellij web are expected to be small control calls; terminal
/// traffic uses WebSockets and static assets are fetched with empty GET bodies.
const MAX_PROXY_REQUEST_BODY_BYTES: usize = 1024 * 1024;

/// Only the handful of JS assets kamaji rewrites in flight (`connection.js`,
/// `terminal.js` — see [`is_rewritable_asset`]) are buffered, since rewriting
/// needs the whole body. Bound it separately so a bad upstream cannot force an
/// unbounded allocation on these exceptional paths.
const MAX_REWRITABLE_ASSET_BYTES: usize = 1024 * 1024;

/// Family name kamaji's bundled Nerd Font is registered under, in both the
/// rewritten `terminal.js` xterm config and the `FontFace` it constructs. It is
/// an arbitrary label that only has to match between those two — the glyphs come
/// from [`NERD_FONT_WOFF2`], not from any font of this name on the client.
const NERD_FONT_FAMILY: &str = "CaskaydiaCove Nerd Font Mono";

/// Same-origin path the proxy serves [`NERD_FONT_WOFF2`] at, referenced by the
/// `FontFace` URL injected into `terminal.js`. It sits outside zellij's own
/// `/assets/` and `/ws/` namespaces so it cannot collide with an upstream route.
const NERD_FONT_PATH: &str = "/kamaji-assets/nerd-font.woff2";

/// CaskaydiaCove Nerd Font Mono (Cascadia Code, Nerd-Font-patched), Regular, as
/// woff2. zellij web renders the terminal with xterm.js configured for a bare
/// `"Monospace"` family, which has no Private-Use-Area glyphs, so Nerd Font icons
/// fall back to tofu. We bundle this patched font and rewrite `terminal.js`
/// ([`rewrite_terminal_js`]) to load and use it.
const NERD_FONT_WOFF2: &[u8] =
    include_bytes!("proxy_assets/CaskaydiaCoveNerdFontMono-Regular.woff2");

/// The xterm.js `theme` object (a JS object literal) that matches the kamaji
/// board's palette — Catppuccin Mocha with the board's darker base background
/// (`--bg #16161f`). Injected into `terminal.js` only when `web_theme = "match"`
/// (see [`ZellijProxy::inject_xterm_theme`]), so the browser terminal's default
/// background and ANSI palette line up with the board around it. Values mirror
/// `crates/kamajid/src/assets/tokens.css`; the three colors absent there
/// (yellow/cyan/magenta) come from Catppuccin Mocha, which the tokens already are.
const KAMAJI_XTERM_THEME: &str = r##"{
            background: "#16161f", foreground: "#cdd6f4",
            cursor: "#89b4fa", cursorAccent: "#16161f",
            selectionBackground: "rgba(137,180,250,0.28)",
            black: "#45475a", red: "#f38ba8", green: "#a6e3a1", yellow: "#f9e2af",
            blue: "#89b4fa", magenta: "#f5c2e7", cyan: "#94e2d5", white: "#bac2de",
            brightBlack: "#585b70", brightRed: "#f38ba8", brightGreen: "#a6e3a1", brightYellow: "#f9e2af",
            brightBlue: "#89b4fa", brightMagenta: "#f5c2e7", brightCyan: "#94e2d5", brightWhite: "#a6adc8"
        }"##;

/// Reverse proxy state: the upstream `zellij web` bases plus the cached session
/// cookie obtained by logging in once with the token.
pub struct ZellijProxy {
    upstream_http: String,
    upstream_ws: String,
    http: reqwest::Client,
    /// `session_token=<uuid>` — the authenticating cookie, set once.
    cookie: Mutex<Option<String>>,
    /// When true (`web_theme = "match"`), splice [`KAMAJI_XTERM_THEME`] into the
    /// rewritten `terminal.js` so the browser terminal matches the board palette.
    /// When false (`"auto"` / an explicit zellij theme name), the terminal keeps
    /// xterm's default palette — only the in-config zellij theme, if any, applies.
    inject_xterm_theme: bool,
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
            inject_xterm_theme: false,
        }
    }

    /// Set whether the rewritten `terminal.js` carries the kamaji xterm theme.
    /// Call at startup from `web_theme` before the proxy is shared.
    pub fn set_inject_xterm_theme(&mut self, inject: bool) {
        self.inject_xterm_theme = inject;
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
        .route(NERD_FONT_PATH, get(serve_nerd_font))
        .fallback(any(http_proxy))
        .with_state(proxy)
}

/// Serve the bundled Nerd Font woff2 ([`NERD_FONT_WOFF2`]) the rewritten
/// `terminal.js` loads. Immutable + far-future cache: the bytes only change when
/// kamaji ships a new binary, and the URL is internal to this proxy.
async fn serve_nerd_font() -> Response {
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        NERD_FONT_WOFF2,
    )
        .into_response()
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

/// Replacement for zellij web's served `connection.js`.
///
/// Why: a session created with `zellij … attach --create-background` (how kamaji
/// makes them) is *transiently* contended in `zellij web` — it cleanly closes
/// the first few fresh web viewers (`Close(Normal, "Connection closed")` ~150 ms
/// after the terminal socket opens), then a viewer sticks. zellij's own client
/// already heals this: every `handleReconnection()` does a full `location.reload`
/// and `auth.js` re-`POST`s `/session` for a *fresh* `web_client_id`, which is
/// exactly what clears the bump. But it heals *slowly and loudly* — a
/// "Reconnecting…" modal plus a `[1,2,4,8,16]s` backoff, repeated per bump — so
/// it reads as an endless loop. This shim keeps the identical heal path (a reload
/// → fresh id) but makes it **instant and silent**, and **bounds** it via
/// `sessionStorage` so a genuinely dead session can't fast-reload forever.
///
/// It preserves the original module's full export surface so it's a true drop-in
/// swap; of those, the bundle currently imports only `handleReconnection` +
/// `markConnectionEstablished` (`websockets.js`) and `initConnectionHandlers`
/// (`index.js`) — the rest are kept verbatim in case a future zellij imports them.
///
/// The marker gate in [`reconnect_shim`] fails *safe*: a future zellij that ships
/// a different `connection.js` is forwarded unchanged (the loop's slow/loud heal
/// returns, but the page is not broken). Re-verify this shim after a zellij bump.
const RECONNECT_SHIM: &str = r#"// Injected by the kamaji reverse proxy — replaces zellij web's connection.js.
// See crates/kamajid/src/zellij_proxy.rs (RECONNECT_SHIM) for the rationale.
const ATTEMPTS_KEY = 'kamaji_reconnect_attempts';
const MAX_RECONNECT_ATTEMPTS = 12; // fast silent reloads before we surface a failure
const STABLE_MS = 3000;            // alive this long => bump is past, reset the budget

let hasConnectedBefore = false;
let isPageUnloading = false;
let reloading = false;

export function getReconnectionDelay() {
    return 0;
}

export async function checkConnection() {
    try {
        const prefix = window.location.protocol === 'https:' ? 'https' : 'http';
        const r = await fetch(`${prefix}://${window.location.host}/info/version`, { method: 'GET' });
        return r.ok;
    } catch (e) {
        return false;
    }
}

function readAttempts() {
    try { return parseInt(window.sessionStorage.getItem(ATTEMPTS_KEY) || '0', 10) || 0; }
    catch (e) { return 0; }
}
function writeAttempts(n) {
    try { window.sessionStorage.setItem(ATTEMPTS_KEY, String(n)); } catch (e) {}
}
function clearAttempts() {
    try { window.sessionStorage.removeItem(ATTEMPTS_KEY); } catch (e) {}
}

export async function handleReconnection() {
    // Only reconnect once per page load, only if we actually connected, and never
    // while the page is being torn down (closing the panel must not trigger a reload).
    if (reloading || isPageUnloading || !hasConnectedBefore) {
        return;
    }
    reloading = true;

    const attempts = readAttempts();
    if (attempts >= MAX_RECONNECT_ATTEMPTS) {
        clearAttempts();
        document.title = 'Terminal disconnected';
        // Overlay rather than replace the DOM: zellij's still-live control
        // handlers reference #terminal/body, so tearing those out throws.
        try {
            const overlay = document.createElement('div');
            overlay.setAttribute(
                'style',
                'position:fixed;inset:0;z-index:9999;background:#111;color:#ddd;' +
                'font-family:monospace;padding:1rem;box-sizing:border-box'
            );
            overlay.innerHTML =
                'Terminal disconnected. <a href="" style="color:#6cf">Reload</a> to retry.';
            document.body.appendChild(overlay);
        } catch (e) {}
        return;
    }
    writeAttempts(attempts + 1);
    // Heal by reloading: the page re-auths and POSTs /session for a FRESH
    // web_client_id, which is what clears zellij web's transient contention bump.
    window.location.reload();
}

export function initConnectionHandlers() {
    window.addEventListener('beforeunload', () => { isPageUnloading = true; });
    window.addEventListener('pagehide', () => { isPageUnloading = true; });
}

export function markConnectionEstablished() {
    hasConnectedBefore = true;
    // Survive STABLE_MS without a drop => the transient bump is behind us; reset
    // the budget so a later, unrelated disconnect gets its own fresh allowance.
    window.setTimeout(clearAttempts, STABLE_MS);
}

export function resetConnectionState() {
    hasConnectedBefore = false;
    isPageUnloading = false;
    reloading = false;
    clearAttempts();
}
"#;

/// If `path` is zellij web's reconnect module *and* `body` matches the version
/// this shim was written against (so we don't silently break a future zellij),
/// return the replacement JS; otherwise `None` and the asset is forwarded as-is.
fn reconnect_shim(path: &str, body: &[u8]) -> Option<&'static str> {
    if path != "/assets/connection.js" {
        return None;
    }
    let body = std::str::from_utf8(body).ok()?;
    if body.contains("handleReconnection") && body.contains("location.reload") {
        Some(RECONNECT_SHIM)
    } else {
        None
    }
}

/// xterm bootstrap kamaji splices into zellij web's `terminal.js` immediately
/// after `term.focus();`. zellij constructs the `Terminal` before any webfont is
/// loaded, and the WebGL renderer rasterizes its glyph atlas from whatever font
/// is resolved at that instant — so a font loaded *later* never reaches already
/// cached cells. This loads the bundled Nerd Font via the CSS Font Loading API,
/// then rebuilds the WebGL atlas (and repaints) once it is ready, so the icon
/// (Private-Use-Area) glyphs render instead of first-paint tofu. Everything is
/// guarded: a missing `webglAddon`/method or a failed load degrades to the
/// comma-fallback `Monospace`, never a thrown error. (See the xterm webfont
/// first-load caveat zellij's own index.html links: xtermjs/xterm.js#5164.)
///
/// Kept in sync with [`NERD_FONT_FAMILY`]/[`NERD_FONT_PATH`] by a unit test.
const NERD_FONT_BOOTSTRAP: &str = r#"
    // Injected by the kamaji reverse proxy — load the bundled Nerd Font, then
    // rebuild the WebGL glyph atlas so Private-Use-Area icon glyphs repaint with
    // it instead of the first-paint fallback. See
    // crates/kamajid/src/zellij_proxy.rs (NERD_FONT_BOOTSTRAP).
    try {
        const kamajiFont = new FontFace("CaskaydiaCove Nerd Font Mono", 'url("/kamaji-assets/nerd-font.woff2") format("woff2")');
        kamajiFont.load().then((loaded) => {
            document.fonts.add(loaded);
            try { webglAddon.clearTextureAtlas(); } catch (e) {}
            try { term.refresh(0, term.rows - 1); } catch (e) {}
        }).catch(() => {});
    } catch (e) {}
"#;

/// If `path` is zellij web's `terminal.js` *and* it still configures the bare
/// `"Monospace"` xterm family this rewrite was written against, return a version
/// that (1) prepends kamaji's bundled Nerd Font to the family and (2) splices in
/// [`NERD_FONT_BOOTSTRAP`] to load that font and rebuild the WebGL atlas — plus,
/// when `inject_theme` is set (`web_theme = "match"`), (3) splices [`KAMAJI_XTERM_THEME`]
/// into the `new Terminal({…})` options so the terminal matches the board palette.
/// Otherwise `None`, and the asset is forwarded unchanged — failing *safe* so a
/// future zellij that restructures `terminal.js` keeps working (just without the
/// icon font/theme). Re-verify this rewrite after a zellij bump.
fn rewrite_terminal_js(path: &str, body: &[u8], inject_theme: bool) -> Option<String> {
    if path != "/assets/terminal.js" {
        return None;
    }
    let body = std::str::from_utf8(body).ok()?;
    // Both anchors must be present, else a restructured terminal.js would be
    // half-rewritten (font set but never loaded, or vice versa). All-or-nothing.
    const FAMILY_ANCHOR: &str = "fontFamily: \"Monospace\"";
    const FOCUS_ANCHOR: &str = "term.focus();";
    if !body.contains(FAMILY_ANCHOR) || !body.contains(FOCUS_ANCHOR) {
        return None;
    }
    let mut rewritten = body
        .replacen(
            FAMILY_ANCHOR,
            &format!("fontFamily: \"{NERD_FONT_FAMILY}, Monospace\""),
            1,
        )
        .replacen(
            FOCUS_ANCHOR,
            &format!("{FOCUS_ANCHOR}{NERD_FONT_BOOTSTRAP}"),
            1,
        );
    // Optionally tint the terminal to the board palette by adding a `theme:`
    // property to the xterm options. Anchored on the line the font rewrite just
    // touched, so it lands inside `new Terminal({…})`. Its own gate keeps it
    // fail-safe independent of the font anchors above.
    if inject_theme {
        let family_line = format!("fontFamily: \"{NERD_FONT_FAMILY}, Monospace\",");
        if rewritten.contains(&family_line) {
            rewritten = rewritten.replacen(
                &family_line,
                &format!("{family_line}\n        theme: {KAMAJI_XTERM_THEME},"),
                1,
            );
        }
    }
    Some(rewritten)
}

/// Paths whose response body kamaji rewrites in flight, so they must be buffered
/// (bounded) rather than streamed. Everything else streams straight through.
fn is_rewritable_asset(path: &str) -> bool {
    matches!(path, "/assets/connection.js" | "/assets/terminal.js")
}

/// A JS response with the content-type browsers require for an ES module, used
/// for every in-flight asset rewrite ([`RECONNECT_SHIM`], [`nerd_font_terminal_js`]).
fn js_response(body: impl Into<Body>) -> Response {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        body.into(),
    )
        .into_response()
}

async fn limited_upstream_bytes(
    upstream: reqwest::Response,
    limit: usize,
) -> Result<axum::body::Bytes, UpstreamBodyError> {
    let mut stream = upstream.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| UpstreamBodyError::Read)?;
        let new_len = bytes
            .len()
            .checked_add(chunk.len())
            .ok_or(UpstreamBodyError::TooLarge)?;
        if new_len > limit {
            return Err(UpstreamBodyError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes.into())
}

enum UpstreamBodyError {
    Read,
    TooLarge,
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
    let path = parts.uri.path().to_string();
    let url = format!("{}{}", proxy.upstream_http, pq);
    let bytes = match axum::body::to_bytes(body, MAX_PROXY_REQUEST_BODY_BYTES).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response(),
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

    let status = upstream.status();
    let headers = upstream.headers().clone();
    if is_rewritable_asset(&path) {
        let body_bytes = match limited_upstream_bytes(upstream, MAX_REWRITABLE_ASSET_BYTES).await {
            Ok(b) => b,
            Err(UpstreamBodyError::TooLarge) => {
                return (StatusCode::BAD_GATEWAY, "upstream asset too large").into_response()
            }
            Err(UpstreamBodyError::Read) => {
                return (StatusCode::BAD_GATEWAY, "upstream body error").into_response()
            }
        };

        // Apply any in-flight rewrite. Each re-derives content headers (the body
        // changed), so a JS response also drops the upstream
        // content-encoding/length that would no longer match.
        if let Some(shim) = reconnect_shim(&path, &body_bytes) {
            return js_response(shim);
        }
        if let Some(js) = rewrite_terminal_js(&path, &body_bytes, proxy.inject_xterm_theme) {
            return js_response(js);
        }

        // Buffered but not rewritten (marker gate failed) — forward the bytes.
        let mut builder = Response::builder().status(status);
        for (k, v) in headers.iter() {
            // Drop framing headers (axum re-derives them) and upstream Set-Cookie —
            // the proxy owns auth, so the browser never needs the cookie.
            if is_hop(k) || k == header::SET_COOKIE {
                continue;
            }
            builder = builder.header(k, v);
        }
        return builder.body(Body::from(body_bytes)).unwrap_or_else(|_| {
            (StatusCode::BAD_GATEWAY, "bad upstream response").into_response()
        });
    }

    let mut builder = Response::builder().status(status);
    for (k, v) in headers.iter() {
        // Drop framing headers (axum re-derives them) and upstream Set-Cookie —
        // the proxy owns auth, so the browser never needs the cookie.
        if is_hop(k) || k == header::SET_COOKIE {
            continue;
        }
        builder = builder.header(k, v);
    }
    builder
        .body(Body::from_stream(upstream.bytes_stream()))
        .unwrap_or_else(|_| (StatusCode::BAD_GATEWAY, "bad upstream response").into_response())
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

    /// A trimmed stand-in for zellij web's served `connection.js` carrying the
    /// markers the shim keys off (the reconnect entry point + the page reload it
    /// performs to heal).
    const STUB_CONNECTION_JS: &str = "export async function handleReconnection() { \
        window.location.reload(); } \
        export function markConnectionEstablished() {} \
        export function initConnectionHandlers() {}";

    #[test]
    fn rewrites_connection_js_into_the_reconnect_shim() {
        let shim = reconnect_shim("/assets/connection.js", STUB_CONNECTION_JS.as_bytes())
            .expect("connection.js with reconnect markers is rewritten");
        // Keeps the exact import surface websockets.js / index.js depend on.
        assert!(shim.contains("export async function handleReconnection"));
        assert!(shim.contains("export function markConnectionEstablished"));
        assert!(shim.contains("export function initConnectionHandlers"));
        // The behavior change: heal via an instant, bounded fresh-id reload.
        assert!(shim.contains("location.reload"), "still heals by reloading");
        assert!(
            shim.contains("sessionStorage"),
            "bounds attempts across reloads"
        );
        assert!(
            shim.contains("MAX_RECONNECT_ATTEMPTS"),
            "caps the silent fast-reload loop"
        );
    }

    #[test]
    fn leaves_other_assets_untouched() {
        assert!(reconnect_shim("/assets/websockets.js", STUB_CONNECTION_JS.as_bytes()).is_none());
        assert!(reconnect_shim("/assets/index.js", b"whatever").is_none());
    }

    /// If a future zellij web ships a connection.js the shim wasn't written
    /// against, fall back to forwarding it unchanged rather than break the page.
    #[test]
    fn passes_through_connection_js_without_known_markers() {
        assert!(reconnect_shim("/assets/connection.js", b"export const x = 1;").is_none());
    }

    /// A trimmed stand-in for zellij web's `terminal.js` carrying the two anchors
    /// the Nerd Font rewrite keys off: the bare `"Monospace"` xterm family and the
    /// `term.focus();` line the font bootstrap is spliced in after.
    const STUB_TERMINAL_JS: &str = "export function initTerminal() { \
        const term = new Terminal({ fontFamily: \"Monospace\", allowProposedApi: true }); \
        const webglAddon = new WebglAddon.WebglAddon(); \
        term.open(document.getElementById(\"terminal\")); \
        term.focus(); \
        return { term }; }";

    #[test]
    fn rewrites_terminal_js_to_use_the_bundled_nerd_font() {
        let out = rewrite_terminal_js("/assets/terminal.js", STUB_TERMINAL_JS.as_bytes(), false)
            .expect("terminal.js with the known anchors is rewritten");
        // The bare Monospace family now leads with the bundled Nerd Font.
        assert!(out.contains(&format!("fontFamily: \"{NERD_FONT_FAMILY}, Monospace\"")));
        assert!(
            !out.contains("fontFamily: \"Monospace\""),
            "the bare family is replaced, not appended to"
        );
        // The font-load + atlas-rebuild bootstrap is spliced in, after focus.
        assert!(out.contains("new FontFace"), "font is loaded");
        assert!(out.contains("clearTextureAtlas"), "WebGL atlas is rebuilt");
        assert!(
            out.contains(NERD_FONT_PATH),
            "points at the proxy-served font"
        );
        let focus = out.find("term.focus();").expect("focus anchor kept");
        let face = out.find("new FontFace").expect("bootstrap present");
        assert!(focus < face, "bootstrap is injected after term.focus()");
        // With inject_theme=false (auto), no xterm theme is added.
        assert!(
            !out.contains("theme:"),
            "no xterm theme injected in auto mode"
        );
    }

    /// `web_theme = "match"` additionally splices the board palette into the
    /// xterm options, on top of the always-on font rewrite.
    #[test]
    fn injects_the_board_xterm_theme_when_requested() {
        let off = rewrite_terminal_js("/assets/terminal.js", STUB_TERMINAL_JS.as_bytes(), false)
            .expect("rewritten");
        let on = rewrite_terminal_js("/assets/terminal.js", STUB_TERMINAL_JS.as_bytes(), true)
            .expect("rewritten");
        assert!(!off.contains("theme:"), "auto: no theme");
        assert!(on.contains("theme:"), "match: theme present");
        assert!(
            on.contains("#16161f") && on.contains("#cdd6f4"),
            "match: theme carries the board palette (bg + fg)"
        );
        // The theme sits inside the Terminal options, after the font family.
        let family = on.find("fontFamily:").expect("family present");
        let theme = on.find("theme:").expect("theme present");
        assert!(
            family < theme,
            "theme follows fontFamily inside new Terminal({{…}})"
        );
        // Font rewrite still happens in match mode too.
        assert!(on.contains("new FontFace"), "match: font still loaded");
    }

    /// All-or-nothing: a restructured terminal.js missing either anchor is
    /// forwarded unchanged rather than half-rewritten (font set but never loaded).
    /// True even when a theme injection was requested.
    #[test]
    fn leaves_terminal_js_untouched_without_known_anchors() {
        assert!(
            rewrite_terminal_js(
                "/assets/terminal.js",
                b"const term = new Terminal({ fontFamily: \"Monospace\" });",
                true
            )
            .is_none(),
            "missing the focus anchor => not rewritten, even with theme requested"
        );
        assert!(
            rewrite_terminal_js("/assets/index.js", STUB_TERMINAL_JS.as_bytes(), true).is_none(),
            "wrong path => not rewritten"
        );
    }

    /// The bootstrap is a raw string literal; guard it against drifting out of
    /// sync with the family/path constants the font route and xterm config share.
    #[test]
    fn bootstrap_references_the_font_family_and_path() {
        assert!(NERD_FONT_BOOTSTRAP.contains(NERD_FONT_FAMILY));
        assert!(NERD_FONT_BOOTSTRAP.contains(NERD_FONT_PATH));
    }

    /// The bundled font is a real woff2 so the browser can actually load it.
    #[test]
    fn bundled_font_is_a_woff2() {
        assert_eq!(&NERD_FONT_WOFF2[..4], b"wOF2", "woff2 magic");
    }

    /// End-to-end through the proxy router against a mock `zellij web`: the
    /// reconnect module is swapped for the shim (with a JS content-type), while a
    /// sibling asset is forwarded byte-for-byte.
    #[tokio::test]
    async fn http_proxy_rewrites_connection_js_and_forwards_others() {
        use axum::routing::get;

        // Mock upstream serving the two assets.
        let upstream = Router::new()
            .route(
                "/assets/connection.js",
                get(|| async { STUB_CONNECTION_JS }),
            )
            .route(
                "/assets/terminal.js",
                get(|| async { "export const TERMINAL = 1;" }),
            );
        let up_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let up_addr = up_listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(up_listener, upstream).await.unwrap() });

        // Proxy pointed at the mock upstream.
        let proxy = Arc::new(ZellijProxy::with_upstream(&format!("http://{up_addr}")));
        let px_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let px_addr = px_listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(px_listener, router(proxy)).await.unwrap() });

        let client = reqwest::Client::new();
        let base = format!("http://{px_addr}");

        let conn = client
            .get(format!("{base}/assets/connection.js"))
            .send()
            .await
            .unwrap();
        assert!(conn
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("javascript"));
        let conn_body = conn.text().await.unwrap();
        // The shim is served (its banner + cap), and the upstream stub body is
        // gone — i.e. the asset was replaced, not passed through or appended to.
        assert!(
            conn_body.contains("Injected by the kamaji reverse proxy"),
            "served the shim"
        );
        assert!(conn_body.contains("MAX_RECONNECT_ATTEMPTS"));
        assert!(
            !conn_body.contains("export function initConnectionHandlers() {}"),
            "upstream stub body replaced, not forwarded"
        );

        let term = client
            .get(format!("{base}/assets/terminal.js"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        // This stub lacks the Nerd Font anchors, so it is buffered but forwarded
        // unchanged — proving the rewrite path fails safe.
        assert_eq!(term, "export const TERMINAL = 1;", "untouched passthrough");
    }

    /// End-to-end through the proxy router: a real-shaped `terminal.js` comes back
    /// rewritten to load the bundled Nerd Font, and the font itself is served by
    /// the proxy's own route (not forwarded upstream).
    #[tokio::test]
    async fn rewrites_terminal_js_and_serves_the_font() {
        use axum::routing::get;

        let upstream =
            Router::new().route("/assets/terminal.js", get(|| async { STUB_TERMINAL_JS }));
        let up_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let up_addr = up_listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(up_listener, upstream).await.unwrap() });

        let proxy = Arc::new(ZellijProxy::with_upstream(&format!("http://{up_addr}")));
        let px_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let px_addr = px_listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(px_listener, router(proxy)).await.unwrap() });

        let client = reqwest::Client::new();
        let base = format!("http://{px_addr}");

        // terminal.js is rewritten to use the bundled Nerd Font.
        let term = client
            .get(format!("{base}/assets/terminal.js"))
            .send()
            .await
            .unwrap();
        assert!(term
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("javascript"));
        let term_body = term.text().await.unwrap();
        assert!(term_body.contains(NERD_FONT_FAMILY), "family injected");
        assert!(
            term_body.contains("new FontFace"),
            "font bootstrap injected"
        );

        // The font is served by the proxy's own route, with a woff2 content-type —
        // never forwarded to (and 404'd by) the upstream.
        let font = client
            .get(format!("{base}{NERD_FONT_PATH}"))
            .send()
            .await
            .unwrap();
        assert_eq!(font.status(), StatusCode::OK);
        assert_eq!(
            font.headers().get(header::CONTENT_TYPE).unwrap(),
            "font/woff2"
        );
        let bytes = font.bytes().await.unwrap();
        assert_eq!(&bytes[..4], b"wOF2", "served a real woff2");
        assert!(bytes.len() > 1000, "served the whole font");
    }

    #[tokio::test]
    async fn http_proxy_rejects_request_bodies_over_limit() {
        use axum::routing::post;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let hits = Arc::new(AtomicUsize::new(0));
        let route_hits = hits.clone();
        let upstream = Router::new().route(
            "/echo",
            post(move || {
                let route_hits = route_hits.clone();
                async move {
                    route_hits.fetch_add(1, Ordering::SeqCst);
                    "ok"
                }
            }),
        );
        let up_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let up_addr = up_listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(up_listener, upstream).await.unwrap() });

        let proxy = Arc::new(ZellijProxy::with_upstream(&format!("http://{up_addr}")));
        let px_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let px_addr = px_listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(px_listener, router(proxy)).await.unwrap() });

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{px_addr}/echo"))
            .body(vec![b'x'; MAX_PROXY_REQUEST_BODY_BYTES + 1])
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(resp.text().await.unwrap(), "request body too large");
        assert_eq!(hits.load(Ordering::SeqCst), 0, "upstream not called");
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
