//! `zellij web` lifecycle + browser-attach info. Owns the optional `zellij web`
//! subprocess (lazy-spawned on first attach), a cached auth token, and the
//! assembly of the per-session attach URL. A daemon concern — `kamaji-core`
//! knows nothing about `zellij web`.

use std::sync::Mutex;

use serde::Serialize;

/// The default base URL `zellij web` serves on (spec §6).
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8082";

/// What a client needs to attach to a ticket's session in the browser.
#[derive(Debug, Clone, Serialize)]
pub struct AttachInfo {
    pub session_name: String,
    /// `<base>/<session>` — the browser opens/iframes this; `zellij web` creates,
    /// attaches, or resurrects the named session.
    pub web_url: String,
    /// The `zellij web` login token (consumed by the login page).
    pub token: String,
}

/// Build the per-session attach URL, tolerating a trailing slash on the base.
pub fn web_url(base: &str, session_name: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), session_name)
}

/// Manages the `zellij web` server + its auth token. `new()` is the real,
/// lazy-spawning manager; `fake()` returns canned attach info without touching
/// any subprocess (for tests and CI, which have no `zellij`).
pub struct ZellijWeb {
    base_url: String,
    /// Cached login token (created lazily via `zellij web --create-token`).
    token: Mutex<Option<String>>,
    /// In `fake` mode this token is returned directly and no subprocess runs.
    fake_token: Option<String>,
}

impl ZellijWeb {
    /// The real manager: lazily spawns `zellij web` and creates a token on the
    /// first `attach_info` call.
    pub fn new() -> Self {
        ZellijWeb {
            base_url: DEFAULT_BASE_URL.to_string(),
            token: Mutex::new(None),
            fake_token: None,
        }
    }

    /// A test double: every `attach_info` returns `token` and the assembled URL,
    /// with no `zellij` subprocess. Used by integration tests and CI. The real
    /// `token` cache stays empty — fake mode returns `fake_token` directly and
    /// never reaches `ensure_running` (the only reader of the cache).
    pub fn fake(token: &str) -> Self {
        ZellijWeb {
            base_url: DEFAULT_BASE_URL.to_string(),
            token: Mutex::new(None),
            fake_token: Some(token.to_string()),
        }
    }

    /// Ensure `zellij web` is running with a valid token and return the attach
    /// info for `session_name`. In `fake` mode this is pure; in real mode it may
    /// spawn the server and create a token (see [`Self::ensure_running`]).
    pub fn attach_info(&self, session_name: &str) -> anyhow::Result<AttachInfo> {
        let token = if let Some(t) = &self.fake_token {
            t.clone()
        } else {
            self.ensure_running()?
        };
        Ok(AttachInfo {
            session_name: session_name.to_string(),
            web_url: web_url(&self.base_url, session_name),
            token,
        })
    }

    /// Ensure the `zellij web` server is reachable and we hold a login token.
    /// Returns the token. Steps (spec §6): (1) create + cache a token if we have
    /// none, (2) probe the server's port; if unreachable, spawn `zellij web` and
    /// poll until it answers (≤3s). Best-effort: a spawn we don't own (server
    /// already running) is fine — we just reuse it.
    fn ensure_running(&self) -> anyhow::Result<String> {
        // (1) Token: create once, then cache. Tokens persist in zellij's own
        // store across server restarts, so a cached one keeps working.
        let token = {
            let mut guard = self.token.lock().expect("token mutex poisoned");
            if guard.is_none() {
                *guard = Some(create_token()?);
            }
            guard.clone().expect("token just set")
        };

        // (2) Ensure the server is up.
        if !port_reachable(&self.base_url) {
            spawn_zellij_web()?;
            wait_until_reachable(&self.base_url, std::time::Duration::from_secs(3))?;
        }
        Ok(token)
    }
}

/// Run `zellij web --create-token` once and parse the printed token.
///
/// NOTE: the parse takes the last whitespace-delimited token of stdout. Verified
/// end-to-end against zellij 0.43.1 (the `#[ignore]`d `zellij_web_real_attach_info`
/// test passes there); if a future zellij changes the `--create-token` output
/// format, re-run that test with `--ignored` and adjust this parser.
fn create_token() -> anyhow::Result<String> {
    let out = std::process::Command::new("zellij")
        .args(["web", "--create-token"])
        .output()?;
    if !out.status.success() {
        anyhow::bail!(
            "zellij web --create-token failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    // `split_whitespace` skips empty runs, so `last()` is a non-empty token
    // (or `None` when stdout is blank → the error below).
    let token = stdout
        .split_whitespace()
        .last()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("could not parse a token from: {stdout:?}"))?;
    Ok(token)
}

/// Spawn a detached `zellij web` server. We do not hold the child (the spec
/// accepts that the server outlives the daemon and is reused on next start).
fn spawn_zellij_web() -> anyhow::Result<()> {
    std::process::Command::new("zellij")
        .arg("web")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

/// True if the host:port of `base_url` accepts a TCP connection right now.
fn port_reachable(base_url: &str) -> bool {
    if let Some(addr) = base_url_to_socket_addr(base_url) {
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok()
    } else {
        false
    }
}

/// Poll the port until reachable or the deadline elapses.
fn wait_until_reachable(base_url: &str, timeout: std::time::Duration) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if port_reachable(base_url) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    anyhow::bail!("zellij web did not become reachable within {timeout:?}")
}

/// Parse the `host:port` out of a `http://host:port` base URL into a SocketAddr.
fn base_url_to_socket_addr(base_url: &str) -> Option<std::net::SocketAddr> {
    let hostport = base_url
        .strip_prefix("http://")
        .or_else(|| base_url.strip_prefix("https://"))?;
    let hostport = hostport.trim_end_matches('/');
    use std::net::ToSocketAddrs;
    hostport.to_socket_addrs().ok()?.next()
}

impl Default for ZellijWeb {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_url_joins_base_and_session() {
        assert_eq!(
            web_url("http://127.0.0.1:8082", "kamaji-7-add-login"),
            "http://127.0.0.1:8082/kamaji-7-add-login"
        );
        // A trailing slash on the base must not double up.
        assert_eq!(
            web_url("http://127.0.0.1:8082/", "s"),
            "http://127.0.0.1:8082/s"
        );
    }

    #[test]
    fn fake_attach_info_returns_canned_token_and_url() {
        let zw = ZellijWeb::fake("test-token");
        let info = zw.attach_info("kamaji-1-x").unwrap();
        assert_eq!(info.session_name, "kamaji-1-x");
        assert_eq!(info.web_url, "http://127.0.0.1:8082/kamaji-1-x");
        assert_eq!(info.token, "test-token");
    }

    #[test]
    fn base_url_parses_to_socket_addr() {
        let addr = base_url_to_socket_addr("http://127.0.0.1:8082").unwrap();
        assert_eq!(addr.port(), 8082);
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        // Trailing slash tolerated.
        assert!(base_url_to_socket_addr("http://127.0.0.1:8082/").is_some());
        // Garbage → None.
        assert!(base_url_to_socket_addr("not a url").is_none());
    }

    #[test]
    fn unreachable_port_is_false() {
        // Port 1 on localhost is not listening in any sane test environment.
        assert!(!port_reachable("http://127.0.0.1:1"));
    }

    /// Live test: requires a real `zellij` ≥ 0.43 on PATH. Not run in CI.
    /// `cargo test -p kamajid -- --ignored zellij_web_real_attach_info`
    #[test]
    #[ignore = "requires a real zellij binary; run manually with --ignored"]
    fn zellij_web_real_attach_info() {
        let zw = ZellijWeb::new();
        let info = zw.attach_info("kamaji-smoke-test").unwrap();
        assert_eq!(info.session_name, "kamaji-smoke-test");
        assert_eq!(info.web_url, "http://127.0.0.1:8082/kamaji-smoke-test");
        assert!(
            !info.token.is_empty(),
            "real attach must yield a non-empty token"
        );
    }
}
