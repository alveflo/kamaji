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
    /// Task 2 reads this in `ensure_running`; suppress dead_code until then.
    #[allow(dead_code)]
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
    /// with no `zellij` subprocess. Used by integration tests and CI.
    pub fn fake(token: &str) -> Self {
        ZellijWeb {
            base_url: DEFAULT_BASE_URL.to_string(),
            token: Mutex::new(Some(token.to_string())),
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

    fn ensure_running(&self) -> anyhow::Result<String> {
        anyhow::bail!("zellij web management not yet implemented")
    }
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
}
