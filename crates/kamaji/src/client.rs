//! Blocking HTTP client over the kamajid REST API. The TUI loop is sync, so
//! commands are `reqwest::blocking` round-trips to localhost (sub-ms).

#[allow(dead_code)]
#[derive(Debug)]
pub enum ClientError {
    NotFound,
    BadRequest(String),
    Server(String),
    Unreachable(reqwest::Error),
    Decode(String),
}

#[allow(dead_code)]
pub type Result<T> = std::result::Result<T, ClientError>;

pub struct DaemonClient {
    #[allow(dead_code)]
    http: reqwest::blocking::Client,
    base: String,
    version: String,
}

impl DaemonClient {
    /// Build a client for `base` (e.g. "http://127.0.0.1:8755") and ping
    /// `/healthz` to confirm liveness and capture the daemon version.
    #[allow(dead_code)]
    pub fn connect(base: String) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(ClientError::Unreachable)?;
        let resp = http
            .get(format!("{base}/healthz"))
            .send()
            .map_err(ClientError::Unreachable)?;
        let body: serde_json::Value = resp
            .json()
            .map_err(|e| ClientError::Decode(e.to_string()))?;
        let version = body
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        Ok(DaemonClient {
            http,
            base,
            version,
        })
    }

    #[allow(dead_code)]
    pub fn base(&self) -> &str {
        &self.base
    }

    #[allow(dead_code)]
    pub fn version(&self) -> &str {
        &self.version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Boot a real kamajid on 127.0.0.1:0, returning its base URL. The tokio
    /// runtime is kept alive in the spawned thread for the test's lifetime so
    /// the server keeps serving.
    fn spawn_daemon() -> String {
        use kamaji_core::config::Config;
        use kamaji_core::db::Db;
        use kamajid::state::AppState;

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            rt.block_on(async move {
                let state = AppState::new(Db::open_in_memory().unwrap(), Config::default());
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                tx.send(format!("http://{addr}")).unwrap();
                // kamajid::serve returns anyhow::Result; unwrap to propagate panics.
                kamajid::serve(listener, state).await.unwrap();
            });
        });
        rx.recv().unwrap()
    }

    #[test]
    fn connect_pings_healthz_and_captures_version() {
        let base = spawn_daemon();
        let client = DaemonClient::connect(base.clone()).unwrap();
        assert_eq!(client.base(), base);

        // The daemon's /healthz reports kamajid's own CARGO_PKG_VERSION.
        // `env!("CARGO_PKG_VERSION")` here expands to the *kamaji* crate version.
        // Both crates are currently pinned to the same version in this workspace
        // (neither uses [workspace.package] inheritance, but both are bumped
        // together). Because they could theoretically diverge, we assert the
        // version is non-empty and a plausible semver string rather than relying
        // on them being equal. The daemon is linked as a dev-dep so if the
        // versions ever diverge this comment (and the test) should be revisited.
        assert!(
            !client.version().is_empty(),
            "daemon should report a non-empty version"
        );
        // Verify it looks like a semver (starts with a digit).
        assert!(
            client
                .version()
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit()),
            "expected a semver version, got {:?}",
            client.version()
        );
    }
}
