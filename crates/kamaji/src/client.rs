//! Blocking HTTP client over the kamajid REST API. The TUI loop is sync, so
//! commands are `reqwest::blocking` round-trips to localhost (sub-ms).

// The client is not yet wired into the binary's command paths; suppress
// dead_code until later tasks connect it.
#![allow(dead_code)]

use kamaji_core::config::Config;
use kamaji_core::models::{Project, Ticket};

#[derive(Debug)]
pub enum ClientError {
    NotFound,
    BadRequest(String),
    Server(String),
    Unreachable(reqwest::Error),
    Decode(String),
}

pub type Result<T> = std::result::Result<T, ClientError>;

pub struct DaemonClient {
    http: reqwest::blocking::Client,
    base: String,
    version: String,
}

impl DaemonClient {
    /// Build a client for `base` (e.g. "http://127.0.0.1:8755") and ping
    /// `/healthz` to confirm liveness and capture the daemon version.
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

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// Map a finished response into a deserialized `T` or a `ClientError`. 2xx →
    /// decode body; 404 → NotFound; 400 → BadRequest(reason); else Server.
    fn parse<T: serde::de::DeserializeOwned>(resp: reqwest::blocking::Response) -> Result<T> {
        let status = resp.status();
        if status.is_success() {
            return resp.json().map_err(|e| ClientError::Decode(e.to_string()));
        }
        let body: serde_json::Value = resp.json().unwrap_or(serde_json::Value::Null);
        let reason = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        match status.as_u16() {
            404 => Err(ClientError::NotFound),
            400 => Err(ClientError::BadRequest(reason)),
            _ => Err(ClientError::Server(reason)),
        }
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let resp = self
            .http
            .get(format!("{}{path}", self.base))
            .send()
            .map_err(ClientError::Unreachable)?;
        Self::parse(resp)
    }

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        self.get_json("/projects")
    }

    pub fn get_project(&self, id: i64) -> Result<Project> {
        self.get_json(&format!("/projects/{id}"))
    }

    pub fn list_tickets(&self, project_id: i64) -> Result<Vec<Ticket>> {
        self.get_json(&format!("/projects/{project_id}/tickets"))
    }

    pub fn get_ticket(&self, id: i64) -> Result<Ticket> {
        self.get_json(&format!("/tickets/{id}"))
    }

    pub fn get_config(&self) -> Result<Config> {
        self.get_json("/config")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kamaji_core::models::Status;

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

    fn seed_project_and_ticket(base: &str) -> (i64, i64) {
        // Use raw HTTP to seed so the read tests are independent of create methods.
        let http = reqwest::blocking::Client::new();
        let p: serde_json::Value = http
            .post(format!("{base}/projects"))
            .json(&serde_json::json!({ "name": "acme", "root_dir": "/tmp/acme" }))
            .send()
            .unwrap()
            .json()
            .unwrap();
        let pid = p["id"].as_i64().unwrap();
        let t: serde_json::Value = http
            .post(format!("{base}/tickets"))
            .json(
                &serde_json::json!({ "project_id": pid, "title": "Add login", "agent": "claude" }),
            )
            .send()
            .unwrap()
            .json()
            .unwrap();
        (pid, t["id"].as_i64().unwrap())
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

    #[test]
    fn read_methods_round_trip() {
        let base = spawn_daemon();
        let client = DaemonClient::connect(base.clone()).unwrap();
        let (pid, tid) = seed_project_and_ticket(&base);
        assert_eq!(client.list_projects().unwrap().len(), 1);
        assert_eq!(client.get_project(pid).unwrap().name, "acme");
        let tickets = client.list_tickets(pid).unwrap();
        assert_eq!(tickets.len(), 1);
        assert_eq!(tickets[0].title, "Add login");
        assert_eq!(client.get_ticket(tid).unwrap().status, Status::Todo);
        assert_eq!(client.get_config().unwrap().default_agent, "claude");
    }

    #[test]
    fn get_ticket_missing_maps_not_found() {
        let base = spawn_daemon();
        let client = DaemonClient::connect(base).unwrap();
        assert!(matches!(client.get_ticket(999), Err(ClientError::NotFound)));
    }
}
