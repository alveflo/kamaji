//! Shared daemon state: the SQLite handle (accessed on the blocking pool since
//! rusqlite is sync), the loaded config, and the event broadcast channel.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use kamaji_core::config::Config;
use kamaji_core::db::Db;
use kamaji_core::events::Event;
use tokio::sync::broadcast;
use tokio::sync::RwLock as TokioRwLock;

use crate::error::ApiError;
use crate::session_driver::{RealSessionDriver, SessionDriver};
use crate::zellij_proxy::ZellijProxy;
use crate::zellij_web::ZellijWeb;

/// Default public base URL of the `zellij web` reverse proxy (board port + 1).
const DEFAULT_PROXY_BASE: &str = "http://127.0.0.1:8756";

/// Capacity of the per-daemon event broadcast. A slow SSE client that lags past
/// this drops events and reconnects (lossy by design — see the spec §5).
const EVENT_CHANNEL_CAPACITY: usize = 64;

#[derive(Clone)]
pub struct AppState {
    db: Arc<Mutex<Db>>,
    pub config: Arc<TokioRwLock<Config>>,
    pub tx: broadcast::Sender<Event>,
    state_dir: Arc<PathBuf>,
    zellij_web: Arc<ZellijWeb>,
    zellij_proxy: Arc<ZellijProxy>,
    /// Public base URL of the reverse proxy, used to build iframe `src`s.
    proxy_base: Arc<String>,
    sessions: Arc<dyn SessionDriver>,
    /// When this daemon process constructed its state — used for uptime in
    /// `GET /diagnostics`.
    started: Arc<Instant>,
    /// The daemon's actual bound board address, set once after binding. Reported
    /// by `GET /diagnostics`. Empty until set (e.g. in tests).
    bound_addr: Arc<OnceLock<SocketAddr>>,
}

impl AppState {
    pub fn new(db: Db, config: Config) -> Self {
        let (tx, _rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        // `web_theme = "match"` tints the browser terminal to the board palette;
        // every other value (incl. "auto" and explicit zellij theme names) leaves
        // xterm's default palette — only the in-config zellij theme, if any, applies.
        let mut proxy = ZellijProxy::new();
        proxy.set_inject_xterm_theme(config.daemon.web_theme.trim() == "match");
        AppState {
            db: Arc::new(Mutex::new(db)),
            config: Arc::new(TokioRwLock::new(config)),
            tx,
            state_dir: Arc::new(kamaji_core::detect::default_state_dir()),
            zellij_web: Arc::new(ZellijWeb::new()),
            zellij_proxy: Arc::new(proxy),
            proxy_base: Arc::new(DEFAULT_PROXY_BASE.to_string()),
            sessions: Arc::new(RealSessionDriver),
            started: Arc::new(Instant::now()),
            bound_addr: Arc::new(OnceLock::new()),
        }
    }

    /// A cloned snapshot of the current config. Cheap; taken per request/round
    /// so a PATCH is observed on the next read. Uses `blocking_read`, so it must
    /// only be called from a blocking thread (e.g. inside `spawn_blocking`),
    /// never on an async runtime worker — use [`AppState::config_async`] there.
    pub fn config_snapshot(&self) -> Config {
        self.config.blocking_read().clone()
    }

    /// A cloned snapshot of the current config, awaited on the async runtime.
    /// Use this from async route bodies; use [`AppState::config_snapshot`] only
    /// inside `spawn_blocking` closures.
    pub async fn config_async(&self) -> Config {
        self.config.read().await.clone()
    }

    /// Override the per-session idle-marker directory. Call before sharing the
    /// state (tests use a temp dir; production uses the default).
    pub fn set_state_dir(&mut self, dir: PathBuf) {
        self.state_dir = Arc::new(dir);
    }

    /// The per-session idle-marker directory.
    pub fn state_dir(&self) -> &std::path::Path {
        &self.state_dir
    }

    /// Override the `zellij web` manager (tests inject `ZellijWeb::fake(...)`).
    pub fn set_zellij_web(&mut self, zw: ZellijWeb) {
        self.zellij_web = Arc::new(zw);
    }

    /// The `zellij web` manager (lazy server + token).
    pub fn zellij_web(&self) -> &ZellijWeb {
        &self.zellij_web
    }

    /// The reverse proxy in front of `zellij web` (served on its own listener).
    pub fn zellij_proxy(&self) -> Arc<ZellijProxy> {
        self.zellij_proxy.clone()
    }

    /// Override the proxy's public base URL (set at startup from the bind addr).
    pub fn set_proxy_base(&mut self, base: String) {
        self.proxy_base = Arc::new(base);
    }

    /// Public base URL of the reverse proxy, e.g. `http://127.0.0.1:8756`.
    /// Iframe `src`s are `<proxy_base>/<session>`.
    pub fn proxy_base(&self) -> &str {
        &self.proxy_base
    }

    /// Override the session-lifecycle driver (tests inject a
    /// [`crate::session_driver::FakeSessionDriver`]).
    pub fn set_session_driver(&mut self, driver: Arc<dyn SessionDriver>) {
        self.sessions = driver;
    }

    /// The session-lifecycle driver used by the resume-on-attach path.
    pub fn sessions(&self) -> &dyn SessionDriver {
        self.sessions.as_ref()
    }

    /// A clone of the shared DB handle, for code that locks it directly (the
    /// poll task) rather than going through the async `with_db` helper.
    pub fn db_handle(&self) -> Arc<Mutex<Db>> {
        self.db.clone()
    }

    /// Run a DB operation on the blocking thread pool. rusqlite is synchronous,
    /// so we must not call it directly on an async worker.
    pub async fn with_db<T, F>(&self, f: F) -> Result<T, ApiError>
    where
        F: FnOnce(&Db) -> anyhow::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let db = db.lock().expect("db mutex poisoned");
            f(&db)
        })
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("db task panicked: {e}")))?
        .map_err(ApiError::Internal)
    }

    /// Seconds since this daemon's state was constructed (process uptime).
    pub fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// Record the daemon's actual bound board address (call once, after bind).
    pub fn set_bound_addr(&self, addr: SocketAddr) {
        let _ = self.bound_addr.set(addr);
    }

    /// The actual bound board address, if it has been recorded yet.
    pub fn bound_addr(&self) -> Option<SocketAddr> {
        self.bound_addr.get().copied()
    }

    /// Broadcast an event to all SSE subscribers. Returns immediately; a send
    /// with no current subscribers is a no-op (not an error).
    pub fn emit(&self, event: Event) {
        let _ = self.tx.send(event);
    }
}
