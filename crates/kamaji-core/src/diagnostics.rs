//! Diagnostics report types and local gathering primitives, shared by the
//! daemon (`GET /diagnostics`, which builds a [`DaemonReport`]) and the
//! `kamaji doctor` command (which deserializes it and merges with its own
//! [`LocalReport`]). "Local" means gathered from the *current* process's
//! environment — when the daemon calls `gather_local` the result reflects the
//! daemon's PATH/temp/env, which is the source of truth for the macOS session
//! and browser-attach failures.

use serde::{Deserialize, Serialize};

/// Outcome of a single diagnostic check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Ok,
    Warn,
    Fail,
}

/// One named check with its verdict, a human-readable detail, and an optional
/// remediation hint (shown only on `Warn`/`Fail`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub verdict: Verdict,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Check {
    pub fn ok(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Check {
            name: name.into(),
            verdict: Verdict::Ok,
            detail: detail.into(),
            hint: None,
        }
    }

    pub fn warn(
        name: impl Into<String>,
        detail: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Check {
            name: name.into(),
            verdict: Verdict::Warn,
            detail: detail.into(),
            hint: Some(hint.into()),
        }
    }

    pub fn fail(
        name: impl Into<String>,
        detail: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Check {
            name: name.into(),
            verdict: Verdict::Fail,
            detail: detail.into(),
            hint: Some(hint.into()),
        }
    }
}

/// One allowlisted environment variable (never a full env dump).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

/// Everything gatherable from a single process's own environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalReport {
    pub checks: Vec<Check>,
    pub env: Vec<EnvVar>,
}

/// The daemon's diagnostics: its own [`LocalReport`] plus live daemon-only
/// state. `kamaji doctor` fetches this over HTTP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonReport {
    pub version: String,
    pub pid: u32,
    pub uptime_secs: u64,
    pub board_bind: String,
    pub proxy_base: String,
    pub zellij_web_reachable: bool,
    pub proxy_reachable: bool,
    /// Raw `zellij list-sessions` output, or `None` if zellij couldn't be asked.
    pub zellij_sessions: Option<String>,
    pub project_count: usize,
    pub ticket_count: usize,
    /// Gathered *inside the daemon process* — the daemon's PATH/temp/env.
    pub local: LocalReport,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_report_round_trips_through_json() {
        let report = DaemonReport {
            version: "0.5.0".into(),
            pid: 1234,
            uptime_secs: 42,
            board_bind: "127.0.0.1:8755".into(),
            proxy_base: "http://127.0.0.1:8756".into(),
            zellij_web_reachable: true,
            proxy_reachable: true,
            zellij_sessions: Some("kamaji-1-x [Created 1h ago]\n".into()),
            project_count: 2,
            ticket_count: 5,
            local: LocalReport {
                checks: vec![
                    Check::ok("zellij on PATH", "zellij 0.43.1 (/opt/homebrew/bin/zellij)"),
                    Check::fail("git on PATH", "not found", "install git / fix PATH"),
                ],
                env: vec![EnvVar {
                    key: "PATH".into(),
                    value: "/usr/bin".into(),
                }],
            },
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: DaemonReport = serde_json::from_str(&json).unwrap();
        assert_eq!(report, back);
        assert!(json.contains("\"verdict\":\"ok\""));
        assert!(json.contains("\"verdict\":\"fail\""));
    }
}
