//! Diagnostics report types and local gathering primitives, shared by the
//! daemon (`GET /diagnostics`, which builds a [`DaemonReport`]) and the
//! `kamaji doctor` command (which deserializes it and merges with its own
//! [`LocalReport`]). "Local" means gathered from the *current* process's
//! environment — when the daemon calls `gather_local` the result reflects the
//! daemon's PATH/temp/env, which is the source of truth for the macOS session
//! and browser-attach failures.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Allowlisted env var keys to report verbatim, plus the `XDG_`/`ZELLIJ`
/// prefixes. Never report anything outside this set — it may hold secrets.
const ENV_EXACT: &[&str] = &["PATH", "HOME", "SHELL", "TMPDIR"];

/// Keep only allowlisted env vars from an iterator of `(key, value)` pairs.
pub(crate) fn filter_env_allowlist(vars: impl Iterator<Item = (String, String)>) -> Vec<EnvVar> {
    let mut out: Vec<EnvVar> = vars
        .filter(|(k, _)| {
            ENV_EXACT.contains(&k.as_str()) || k.starts_with("XDG_") || k.starts_with("ZELLIJ")
        })
        .map(|(key, value)| EnvVar { key, value })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

/// Resolve `name` against the entries of a `PATH`-style string, returning the
/// first existing file. Mirrors how the OS would resolve `Command::new(name)`.
pub(crate) fn resolve_in_path(name: &str, path_var: &str) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Run `<bin> --version` and classify the result. The detail carries the
/// version line and the resolved path; failure carries a PATH-oriented hint
/// (the leading suspect for the macOS reports — a GUI-spawned daemon often has
/// no Homebrew dir on PATH).
fn check_binary(name: &str) -> Check {
    let path_var = std::env::var("PATH").unwrap_or_default();
    let resolved = resolve_in_path(name, &path_var);
    match Command::new(name).arg("--version").output() {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let version = stdout.lines().next().unwrap_or("").trim();
            // Some tools print their version to stderr with exit 0; fall back.
            let stderr = String::from_utf8_lossy(&out.stderr);
            let version = if version.is_empty() {
                stderr.lines().next().unwrap_or("").trim()
            } else {
                version
            };
            let where_ = resolved
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "on PATH".to_string());
            Check::ok(format!("{name} on PATH"), format!("{version} ({where_})"))
        }
        Ok(out) => Check::fail(
            format!("{name} on PATH"),
            format!(
                "`{name} --version` exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            format!("`{name}` runs but errored; check the install"),
        ),
        Err(e) => Check::fail(
            format!("{name} on PATH"),
            format!("could not run `{name}`: {e}"),
            format!(
                "`{name}` is not on this process's PATH. On macOS a GUI/auto-spawned \
                 process often lacks /opt/homebrew/bin and /usr/local/bin — install \
                 {name} or ensure it is on PATH for the kamaji daemon."
            ),
        ),
    }
}

/// Probe whether `dir` is writable by creating, writing, and removing a unique
/// probe file. `None` → Warn (path couldn't be resolved at all).
pub(crate) fn check_dir_writable(label: &str, dir: Option<PathBuf>) -> Check {
    let name = format!("{label} dir writable");
    let Some(dir) = dir else {
        return Check::warn(
            name,
            "could not resolve path",
            "no HOME / XDG base — diagnostics limited",
        );
    };
    let probe = dir.join(format!(".kamaji-doctor-probe-{}", std::process::id()));
    let result = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&probe, b"ok"));
    let _ = std::fs::remove_file(&probe);
    match result {
        Ok(()) => Check::ok(name, dir.display().to_string()),
        Err(e) => Check::fail(
            name,
            format!("{}: {e}", dir.display()),
            "the daemon cannot write here; session layout/config files will fail to be created",
        ),
    }
}

/// Best-effort check that the user's zellij config file is readable, if present.
fn check_zellij_config() -> Check {
    match crate::zellij_config::config_file_path() {
        Some(p) if p.is_file() => match std::fs::read(&p) {
            Ok(_) => Check::ok("zellij config readable", p.display().to_string()),
            Err(e) => Check::fail(
                "zellij config readable",
                format!("{}: {e}", p.display()),
                "the daemon derives the web-sharing config from this file",
            ),
        },
        Some(p) => Check::ok(
            "zellij config readable",
            format!("{} (absent — defaults used)", p.display()),
        ),
        None => Check::warn(
            "zellij config readable",
            "no config path resolvable",
            "no HOME / XDG base",
        ),
    }
}

/// True if a TCP connect to `host:port` succeeds within a short timeout.
pub fn tcp_reachable(host: &str, port: u16, timeout: std::time::Duration) -> bool {
    use std::net::ToSocketAddrs;
    (host, port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut addrs| addrs.next())
        .map(|addr| std::net::TcpStream::connect_timeout(&addr, timeout).is_ok())
        .unwrap_or(false)
}

/// The most-recently-modified `kamajid*` file in `dir` (the daemon's rolling log
/// files carry a date suffix, so we pick the newest rather than a fixed name).
pub fn newest_log_file(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("kamajid"))
        .filter_map(|e| {
            let m = e.metadata().ok()?.modified().ok()?;
            Some((m, e.path()))
        })
        .max_by_key(|(m, _)| *m)
        .map(|(_, p)| p)
}

/// Gather everything observable from the *current process's* environment. When
/// the daemon calls this, the result reflects the daemon's PATH/temp/env.
pub fn gather_local() -> LocalReport {
    let temp = std::env::temp_dir();
    let checks = vec![
        check_binary("zellij"),
        check_binary("git"),
        check_dir_writable("temp", Some(temp.clone())),
        check_dir_writable("layout temp", Some(temp.join("kamaji-layouts"))),
        check_dir_writable("zellij-config temp", Some(temp.join("kamaji-zellij"))),
        check_dir_writable("config", crate::paths::config_dir()),
        check_dir_writable("data", crate::paths::data_dir()),
        check_dir_writable("cache", crate::paths::cache_dir()),
        check_dir_writable("runtime", crate::paths::runtime_dir()),
        check_zellij_config(),
    ];
    let env = filter_env_allowlist(std::env::vars());
    LocalReport { checks, env }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn resolve_in_path_finds_an_existing_executable_dir() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("mytool");
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
        let path_val = dir.path().to_string_lossy().to_string();
        assert_eq!(
            super::resolve_in_path("mytool", &path_val).as_deref(),
            Some(bin.as_path())
        );
        assert_eq!(super::resolve_in_path("nope", &path_val), None);
    }

    #[test]
    fn writable_check_passes_on_a_tempdir_and_fails_on_a_bogus_path() {
        let dir = tempfile::tempdir().unwrap();
        let ok = super::check_dir_writable("temp", Some(dir.path().to_path_buf()));
        assert_eq!(ok.verdict, Verdict::Ok, "{ok:?}");
        let bad = super::check_dir_writable(
            "bogus",
            Some(Path::new("/no/such/kamaji/dir").to_path_buf()),
        );
        assert_eq!(bad.verdict, Verdict::Fail, "{bad:?}");
        let none = super::check_dir_writable("unknown", None);
        assert_eq!(none.verdict, Verdict::Warn, "{none:?}");
    }

    #[test]
    fn env_allowlist_keeps_only_relevant_keys() {
        let pairs = vec![
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("HOME".to_string(), "/home/u".to_string()),
            ("XDG_CONFIG_HOME".to_string(), "/x".to_string()),
            ("ZELLIJ".to_string(), "0".to_string()),
            ("AWS_SECRET_ACCESS_KEY".to_string(), "shh".to_string()),
            ("RANDOM_OTHER".to_string(), "x".to_string()),
        ];
        let got = super::filter_env_allowlist(pairs.into_iter());
        let keys: Vec<&str> = got.iter().map(|e| e.key.as_str()).collect();
        assert!(keys.contains(&"PATH"));
        assert!(keys.contains(&"HOME"));
        assert!(keys.contains(&"XDG_CONFIG_HOME"));
        assert!(keys.contains(&"ZELLIJ"));
        assert!(
            !keys.contains(&"AWS_SECRET_ACCESS_KEY"),
            "secrets must be dropped"
        );
        assert!(!keys.contains(&"RANDOM_OTHER"));
    }

    #[test]
    fn newest_log_file_picks_the_kamajid_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("kamajid.2026-06-08.log"), b"older").unwrap();
        std::fs::write(dir.path().join("unrelated.txt"), b"x").unwrap();
        // Ensure the second kamajid file has a strictly later mtime so the
        // "pick newest" selection (not just the prefix filter) is exercised.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.path().join("kamajid.2026-06-09.log"), b"newer").unwrap();
        let chosen = super::newest_log_file(dir.path());
        let name = chosen
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        assert_eq!(name.as_deref(), Some("kamajid.2026-06-09.log"));
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(super::newest_log_file(empty.path()), None);
    }

    #[test]
    fn gather_local_returns_zellij_and_git_checks_and_some_env() {
        let report = super::gather_local();
        let names: Vec<&str> = report.checks.iter().map(|c| c.name.as_str()).collect();
        assert!(names.iter().any(|n| n.contains("zellij")), "{names:?}");
        assert!(names.iter().any(|n| n.contains("git")), "{names:?}");
        assert!(report.env.iter().any(|e| e.key == "PATH"));
    }

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
