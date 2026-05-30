//! Shared daemon state: the SQLite handle (accessed on the blocking pool since
//! rusqlite is sync), the loaded config, and the event broadcast channel.

use std::sync::{Arc, Mutex};

use kamaji_core::config::Config;
use kamaji_core::db::Db;
use kamaji_core::events::Event;
use tokio::sync::broadcast;

use crate::error::ApiError;

/// Capacity of the per-daemon event broadcast. A slow SSE client that lags past
/// this drops events and reconnects (lossy by design — see the spec §5).
const EVENT_CHANNEL_CAPACITY: usize = 64;

#[derive(Clone)]
pub struct AppState {
    db: Arc<Mutex<Db>>,
    pub config: Arc<Config>,
    pub tx: broadcast::Sender<Event>,
}

impl AppState {
    pub fn new(db: Db, config: Config) -> Self {
        let (tx, _rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        AppState {
            db: Arc::new(Mutex::new(db)),
            config: Arc::new(config),
            tx,
        }
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

    /// Broadcast an event to all SSE subscribers. Returns immediately; a send
    /// with no current subscribers is a no-op (not an error).
    pub fn emit(&self, event: Event) {
        let _ = self.tx.send(event);
    }
}
