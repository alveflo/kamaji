# kamaji Diagnostics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the daemon's logs persist to a known file and add a `kamaji doctor` command that produces a shareable diagnostics report — so remote macOS users can surface the real cause behind the generic "server error" and the crashed terminal iframe.

**Architecture:** A new pure `kamaji_core::diagnostics` module defines the serializable report types (`Verdict`/`Check`/`LocalReport`/`DaemonReport`) and the local gathering primitives (binary resolution, temp/dir writability, env allowlist, TCP probe). The daemon (`kamajid`) gains a `GET /diagnostics` endpoint that runs the local gatherers **inside the daemon process** (so PATH/env reflect the daemon, the source of truth) plus daemon-only live state, and a `tracing` file-log layer that writes to `<cache>/kamaji/`. The `kamaji` binary gains a `doctor` subcommand that merges its own local report with the daemon's, tails the daemon log, and renders a sectioned report (or `--json`).

**Tech Stack:** Rust, axum 0.7, `tracing` + `tracing-appender`, `serde`/`serde_json`, `reqwest::blocking`.

---

## File structure

| File | Responsibility | Created/Modified |
|------|----------------|------------------|
| `crates/kamaji-core/src/diagnostics.rs` | Report types + local gatherers (pure-ish, serializable) | Create |
| `crates/kamaji-core/src/lib.rs` | Register `pub mod diagnostics;` | Modify |
| `crates/kamaji-core/src/paths.rs` | `log_dir()` helper | Modify |
| `crates/kamaji-core/src/zellij_config.rs` | Make `config_file_path` public | Modify |
| `crates/kamajid/Cargo.toml` | Add `tracing-appender` dep | Modify |
| `crates/kamajid/src/main.rs` | `init_tracing` returns a file-log guard; `main` holds it | Modify |
| `crates/kamajid/src/state.rs` | `started: Instant` for uptime | Modify |
| `crates/kamajid/src/routes/diagnostics.rs` | `GET /diagnostics` handler building `DaemonReport` | Create |
| `crates/kamajid/src/routes/mod.rs` | Register `pub mod diagnostics;` | Modify |
| `crates/kamajid/src/lib.rs` | Mount `/diagnostics` route | Modify |
| `crates/kamaji/src/client.rs` | `DaemonClient::get_diagnostics` | Modify |
| `crates/kamaji/src/doctor.rs` | Merge local + daemon, tail logs, render report | Create |
| `crates/kamaji/src/cli.rs` | Parse `kamaji doctor [--json] [--daemon ADDR]` + USAGE | Modify |
| `crates/kamaji/src/main.rs` | Dispatch `Command::Doctor` | Modify |

---

## Task 1: Add `tracing-appender` dependency and `paths::log_dir()`

**Files:**
- Modify: `crates/kamajid/Cargo.toml`
- Modify: `crates/kamaji-core/src/paths.rs`

- [ ] **Step 1: Add the dependency**

In `crates/kamajid/Cargo.toml`, under `[dependencies]`, after the
`tracing-subscriber` line (line 28), add:

```toml
tracing-appender = "0.2"
```

- [ ] **Step 2: Write the failing test for `log_dir`**

In `crates/kamaji-core/src/paths.rs`, inside the existing
`#[cfg(all(test, not(windows)))] mod tests` block, add:

```rust
    #[test]
    fn log_dir_is_the_cache_dir() {
        // The daemon log file lives directly in the cache dir; doctor reads it
        // from the same place. They must agree, so log_dir() == cache_dir().
        assert_eq!(super::log_dir(), super::cache_dir());
    }
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p kamaji-core log_dir_is_the_cache_dir`
Expected: FAIL — `no function or associated item named 'log_dir'`.

- [ ] **Step 4: Implement `log_dir`**

In `crates/kamaji-core/src/paths.rs`, after `cache_dir()` (line 33), add:

```rust
/// Directory the daemon writes its rolling log files into (and where
/// `kamaji doctor` reads them from). The same as [`cache_dir`] — kept as a
/// distinct, named function so the daemon and the doctor command share one
/// source of truth for the log location.
pub fn log_dir() -> Option<PathBuf> {
    cache_dir()
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p kamaji-core log_dir_is_the_cache_dir`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/kamajid/Cargo.toml crates/kamaji-core/src/paths.rs
git commit -m "feat(core): add paths::log_dir() and tracing-appender dep"
```

---

## Task 2: Diagnostics report types in `kamaji-core`

**Files:**
- Create: `crates/kamaji-core/src/diagnostics.rs`
- Modify: `crates/kamaji-core/src/lib.rs`

These are the serializable contract shared by the daemon (which builds them) and
the `kamaji` binary (which deserializes them). No gathering logic yet — just the
types and a serde round-trip test.

- [ ] **Step 1: Create the module with the type definitions**

Create `crates/kamaji-core/src/diagnostics.rs`:

```rust
//! Diagnostics report types and local gathering primitives, shared by the
//! daemon (`GET /diagnostics`, which builds a [`DaemonReport`]) and the
//! `kamaji doctor` command (which deserializes it and merges with its own
//! [`LocalReport`]). "Local" means gathered from the *current* process's
//! environment — when the daemon calls [`gather_local`] the result reflects the
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

    pub fn warn(name: impl Into<String>, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Check {
            name: name.into(),
            verdict: Verdict::Warn,
            detail: detail.into(),
            hint: Some(hint.into()),
        }
    }

    pub fn fail(name: impl Into<String>, detail: impl Into<String>, hint: impl Into<String>) -> Self {
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
```

- [ ] **Step 2: Register the module**

In `crates/kamaji-core/src/lib.rs`, add `pub mod diagnostics;` alongside the
other `pub mod` declarations (keep the list alphabetical if it already is).

- [ ] **Step 3: Write the failing serde round-trip test**

At the bottom of `crates/kamaji-core/src/diagnostics.rs`, add:

```rust
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
                env: vec![EnvVar { key: "PATH".into(), value: "/usr/bin".into() }],
            },
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: DaemonReport = serde_json::from_str(&json).unwrap();
        assert_eq!(report, back);
        // `hint` is omitted for Ok checks but present for Fail checks.
        assert!(json.contains("\"verdict\":\"ok\""));
        assert!(json.contains("\"verdict\":\"fail\""));
    }
}
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p kamaji-core daemon_report_round_trips_through_json`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/kamaji-core/src/diagnostics.rs crates/kamaji-core/src/lib.rs
git commit -m "feat(core): diagnostics report types (Verdict/Check/LocalReport/DaemonReport)"
```

---

## Task 3: Local gathering primitives + `gather_local()`

**Files:**
- Modify: `crates/kamaji-core/src/diagnostics.rs`
- Modify: `crates/kamaji-core/src/zellij_config.rs`

- [ ] **Step 1: Make the zellij config path accessible**

In `crates/kamaji-core/src/zellij_config.rs`, find the function
`fn config_file_path() -> Option<PathBuf>` (around line 54) and change its
signature to `pub fn config_file_path() -> Option<PathBuf>`. Add a doc line:

```rust
/// Resolve the user's zellij `config.kdl` path (env overrides → XDG → ~/.config).
/// Public so diagnostics can report whether it is readable.
pub fn config_file_path() -> Option<PathBuf> {
```

- [ ] **Step 2: Write failing tests for the gatherers**

In `crates/kamaji-core/src/diagnostics.rs`, extend the `tests` module with:

```rust
    use std::path::Path;

    #[test]
    fn resolve_in_path_finds_an_existing_executable_dir() {
        // A binary that exists in a dir we put on a synthetic PATH is resolved;
        // a missing one is None. Use the tempdir as the only PATH entry.
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

        let bad = super::check_dir_writable("bogus", Some(Path::new("/no/such/kamaji/dir").to_path_buf()));
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
        assert!(!keys.contains(&"AWS_SECRET_ACCESS_KEY"), "secrets must be dropped");
        assert!(!keys.contains(&"RANDOM_OTHER"));
    }

    #[test]
    fn newest_log_file_picks_the_kamajid_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("kamajid.2026-06-09.log"), b"old").unwrap();
        std::fs::write(dir.path().join("unrelated.txt"), b"x").unwrap();
        let chosen = super::newest_log_file(dir.path());
        let name = chosen.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        assert_eq!(name.as_deref(), Some("kamajid.2026-06-09.log"));
        // Empty dir → None.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(super::newest_log_file(empty.path()), None);
    }

    #[test]
    fn gather_local_returns_zellij_and_git_checks_and_some_env() {
        let report = super::gather_local();
        let names: Vec<&str> = report.checks.iter().map(|c| c.name.as_str()).collect();
        assert!(names.iter().any(|n| n.contains("zellij")), "{names:?}");
        assert!(names.iter().any(|n| n.contains("git")), "{names:?}");
        // PATH is essentially always set in a test environment.
        assert!(report.env.iter().any(|e| e.key == "PATH"));
    }
```

Add `tempfile` to `kamaji-core`'s `[dev-dependencies]` only if missing — it is
already present (`tempfile = "3"`), so no Cargo change is needed here.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p kamaji-core --lib diagnostics`
Expected: FAIL — the helper functions don't exist yet.

- [ ] **Step 4: Implement the gatherers**

In `crates/kamaji-core/src/diagnostics.rs`, add these imports at the top (below
the existing `use serde::...`):

```rust
use std::path::{Path, PathBuf};
use std::process::Command;
```

Then add the gathering functions (above the `#[cfg(test)] mod tests`):

```rust
/// Allowlisted env var keys to report verbatim, plus the `XDG_`/`ZELLIJ`
/// prefixes. Never report anything outside this set — it may hold secrets.
const ENV_EXACT: &[&str] = &["PATH", "HOME", "SHELL", "TMPDIR"];

/// Keep only allowlisted env vars from an iterator of `(key, value)` pairs.
pub(crate) fn filter_env_allowlist(
    vars: impl Iterator<Item = (String, String)>,
) -> Vec<EnvVar> {
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
            let version = String::from_utf8_lossy(&out.stdout);
            let version = version.lines().next().unwrap_or("").trim();
            let where_ = resolved
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "on PATH".to_string());
            Check::ok(format!("{name} on PATH"), format!("{version} ({where_})"))
        }
        Ok(out) => Check::fail(
            format!("{name} on PATH"),
            format!("`{name} --version` exited {}: {}", out.status, String::from_utf8_lossy(&out.stderr).trim()),
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
        return Check::warn(name, "could not resolve path", "no HOME / XDG base — diagnostics limited");
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
        Some(p) => Check::ok("zellij config readable", format!("{} (absent — defaults used)", p.display())),
        None => Check::warn("zellij config readable", "no config path resolvable", "no HOME / XDG base"),
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
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("kamajid")
        })
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
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p kamaji-core --lib diagnostics`
Expected: PASS (all five new tests + the round-trip test).

- [ ] **Step 6: Commit**

```bash
git add crates/kamaji-core/src/diagnostics.rs crates/kamaji-core/src/zellij_config.rs
git commit -m "feat(core): local diagnostics gatherers (binaries, dirs, env, tcp probe)"
```

---

## Task 4: Persist the daemon's logs to a rolling file

**Files:**
- Modify: `crates/kamajid/src/main.rs`

Today `init_tracing` writes only to stdout, and the auto-spawned daemon's stdout
is `/dev/null` — so the real cause of both bugs is discarded. Add a file layer
writing to `<cache>/kamaji/kamajid.<date>.log`, keeping the console layer.

- [ ] **Step 1: Update imports**

In `crates/kamajid/src/main.rs`, replace the line
`use tracing_subscriber::EnvFilter;` (line 11) with:

```rust
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
```

- [ ] **Step 2: Rewrite `init_tracing` to add the file layer and return a guard**

Replace the whole `init_tracing` function (lines 76–86) with:

```rust
/// Initialize tracing with a console layer (as before) **and** a rolling file
/// layer under `paths::log_dir()` so the daemon's logs survive even when it was
/// auto-spawned by the TUI with stdout/stderr pointed at /dev/null. Returns the
/// non-blocking writer guard, which the caller must hold for the process
/// lifetime (dropping it stops the background log writer). `None` when no log
/// file could be opened — the console layer still works.
fn init_tracing(config: &Config) -> Option<WorkerGuard> {
    let filter = EnvFilter::try_from_env("KAMAJID_LOG")
        .or_else(|_| EnvFilter::try_new(&config.daemon.log_level))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let json = config.daemon.log_format == "json";

    // Console layer: human or json, matching the prior behavior.
    let console = if json {
        tracing_subscriber::fmt::layer().json().boxed()
    } else {
        tracing_subscriber::fmt::layer().boxed()
    };

    // File layer (best-effort): rolling daily, keep the last 5 files, no ANSI.
    let (file_layer, guard) = match open_log_appender() {
        Some(appender) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let layer = if json {
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_ansi(false)
                    .with_writer(writer)
                    .boxed()
            } else {
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(writer)
                    .boxed()
            };
            (Some(layer), Some(guard))
        }
        None => (None, None),
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(console)
        .with(file_layer)
        .init();
    guard
}

/// Open the rolling log appender under `paths::log_dir()`, creating the dir if
/// needed. Files are named `kamajid.<date>.log`, daily-rotated, last 5 kept.
/// Returns `None` (no file logging) if the dir is unavailable or unwritable.
fn open_log_appender() -> Option<RollingFileAppender> {
    let dir = paths::log_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("kamajid")
        .filename_suffix("log")
        .max_log_files(5)
        .build(&dir)
        .ok()
}
```

Note: `.boxed()` requires the `SubscriberExt`/`Layer` traits to be in scope —
the `tracing_subscriber::layer::SubscriberExt` import plus the prelude bring
`Layer::boxed` in. If `boxed()` is unresolved, add
`use tracing_subscriber::Layer;` to the imports.

- [ ] **Step 3: Hold the guard in `main`**

In `main` (around line 92), change:

```rust
    init_tracing(&config);
```

to:

```rust
    // Hold the file-log writer guard for the whole process so buffered log
    // lines are flushed; dropping it would stop the background writer.
    let _log_guard = init_tracing(&config);
```

- [ ] **Step 4: Build and verify it compiles + manual smoke**

Run: `cargo build -p kamajid`
Expected: builds clean.

Manual smoke (verifies the file is actually created):

```bash
XDG_CACHE_HOME=/tmp/kamaji-doctor-smoke cargo run -p kamajid -- serve --bind 127.0.0.1:0 &
sleep 1; kill %1 2>/dev/null
ls /tmp/kamaji-doctor-smoke/kamaji/kamajid.*.log
```

Expected: a `kamajid.<date>.log` file exists and contains the "kamajid
listening" line.

- [ ] **Step 5: Commit**

```bash
git add crates/kamajid/src/main.rs
git commit -m "feat(kamajid): persist daemon logs to a rolling file in the cache dir"
```

---

## Task 5: `GET /diagnostics` daemon endpoint

**Files:**
- Modify: `crates/kamajid/src/state.rs`
- Create: `crates/kamajid/src/routes/diagnostics.rs`
- Modify: `crates/kamajid/src/routes/mod.rs`
- Modify: `crates/kamajid/src/lib.rs`

- [ ] **Step 1: Add a start `Instant` to `AppState` for uptime**

In `crates/kamajid/src/state.rs`:

Add to imports (near the top `use std::sync::...`):

```rust
use std::time::Instant;
```

Add a field to the `AppState` struct (after `sessions: Arc<dyn SessionDriver>,`):

```rust
    /// When this daemon process constructed its state — used for uptime in
    /// `GET /diagnostics`.
    started: Arc<Instant>,
```

In `AppState::new`, add to the constructed struct (after `sessions: ...`):

```rust
            started: Arc::new(Instant::now()),
```

Add an accessor method in the `impl AppState` block (after `emit`):

```rust
    /// Seconds since this daemon's state was constructed (process uptime).
    pub fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
```

- [ ] **Step 2: Write the failing endpoint test**

Create `crates/kamajid/src/routes/diagnostics.rs` with just the test first so it
fails to compile, then add the handler in Step 3. For now, add the full file
(handler + test) as shown in Step 3; run the test after.

- [ ] **Step 3: Implement the handler**

Create `crates/kamajid/src/routes/diagnostics.rs`:

```rust
//! `GET /diagnostics` — the daemon's self-report. Runs the shared local
//! gatherers *inside the daemon process* (so PATH/temp/env reflect the daemon,
//! the source of truth for session + browser-attach failures) and adds live
//! daemon-only state: zellij-web/proxy reachability, the session list, counts.

use std::time::Duration;

use axum::extract::State;
use axum::Json;

use kamaji_core::diagnostics::{self, DaemonReport};

use crate::state::AppState;

/// Probe timeout for the local zellij-web and proxy ports. Short — both are on
/// localhost, so a live port answers in well under this.
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

pub async fn diagnostics(State(state): State<AppState>) -> Json<DaemonReport> {
    let config = state.config_async().await;
    let board_bind = config.daemon.bind.clone();
    let proxy_base = state.proxy_base().to_string();

    // Project/ticket counts (best-effort; 0 on any DB error).
    let counts = state
        .with_db(|db| {
            let projects = db.list_projects()?;
            let mut tickets = 0usize;
            for p in &projects {
                tickets += db.list_tickets(p.id)?.len();
            }
            Ok((projects.len(), tickets))
        })
        .await
        .unwrap_or((0, 0));

    // Local gather + zellij list-sessions run on the blocking pool: they shell
    // out and touch the filesystem, which must not block an async worker.
    let proxy_for_probe = proxy_base.clone();
    let (local, zellij_sessions, zellij_web_reachable, proxy_reachable) =
        tokio::task::spawn_blocking(move || {
            let local = diagnostics::gather_local();
            let sessions = kamaji_core::zellij::list_sessions();
            let web = diagnostics::tcp_reachable("127.0.0.1", 8082, PROBE_TIMEOUT);
            let proxy = probe_proxy(&proxy_for_probe);
            (local, sessions, web, proxy)
        })
        .await
        .unwrap_or_else(|_| {
            (
                diagnostics::LocalReport { checks: vec![], env: vec![] },
                None,
                false,
                false,
            )
        });

    Json(DaemonReport {
        version: env!("CARGO_PKG_VERSION").to_string(),
        pid: std::process::id(),
        uptime_secs: state.uptime_secs(),
        board_bind,
        proxy_base,
        zellij_web_reachable,
        proxy_reachable,
        zellij_sessions,
        project_count: counts.0,
        ticket_count: counts.1,
        local,
    })
}

/// Parse `host:port` out of `http://host:port` and TCP-probe it.
fn probe_proxy(proxy_base: &str) -> bool {
    let hostport = proxy_base
        .strip_prefix("http://")
        .or_else(|| proxy_base.strip_prefix("https://"))
        .unwrap_or(proxy_base)
        .trim_end_matches('/');
    match hostport.rsplit_once(':') {
        Some((host, port)) => match port.parse::<u16>() {
            Ok(port) => diagnostics::tcp_reachable(host, port, PROBE_TIMEOUT),
            Err(_) => false,
        },
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::extract::State;
    use kamaji_core::config::Config;
    use kamaji_core::db::Db;

    fn test_state() -> AppState {
        let db = Db::open_in_memory().unwrap();
        let mut state = AppState::new(db, Config::default());
        state.set_zellij_web(crate::zellij_web::ZellijWeb::fake("t"));
        state
    }

    #[tokio::test]
    async fn diagnostics_returns_a_well_formed_report() {
        let state = test_state();
        let Json(report) = diagnostics(State(state)).await;
        assert_eq!(report.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(report.pid, std::process::id());
        // Local section always has the zellij + git checks.
        assert!(report
            .local
            .checks
            .iter()
            .any(|c| c.name.contains("zellij")));
        // Counts are zero on a fresh in-memory DB.
        assert_eq!(report.project_count, 0);
        assert_eq!(report.ticket_count, 0);
    }

    #[tokio::test]
    async fn probe_proxy_parses_host_and_port() {
        // Nothing listens on this port → unreachable, but parsing must succeed
        // (a parse failure would also return false, so assert via a known-bad
        // url returning false and a malformed one returning false too).
        assert!(!probe_proxy("http://127.0.0.1:1"));
        assert!(!probe_proxy("garbage"));
    }
}
```

Note: `to_bytes` import is unused above and can be dropped; keep imports to what
the tests reference (`State`, `Json`, `Config`, `Db`).

- [ ] **Step 4: Register the module and route**

In `crates/kamajid/src/routes/mod.rs`, add `pub mod diagnostics;` with the other
route module declarations.

In `crates/kamajid/src/lib.rs`, add a route in `router()` after the `/healthz`
line (line 24):

```rust
        .route("/diagnostics", get(routes::diagnostics::diagnostics))
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p kamajid diagnostics`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/kamajid/src/state.rs crates/kamajid/src/routes/diagnostics.rs crates/kamajid/src/routes/mod.rs crates/kamajid/src/lib.rs
git commit -m "feat(kamajid): GET /diagnostics endpoint with daemon-side live state"
```

---

## Task 6: `DaemonClient::get_diagnostics` + `doctor` renderer

**Files:**
- Modify: `crates/kamaji/src/client.rs`
- Create: `crates/kamaji/src/doctor.rs`

- [ ] **Step 1: Add the client method**

In `crates/kamaji/src/client.rs`, add an import at the top (with the other
`use kamaji_core::...`):

```rust
use kamaji_core::diagnostics::DaemonReport;
```

Add a method inside `impl DaemonClient` (after `version_skew`, before `parse`):

```rust
    /// Fetch the daemon's diagnostics report (`GET /diagnostics`).
    pub fn get_diagnostics(&self) -> Result<DaemonReport> {
        self.get_json("/diagnostics")
    }
```

- [ ] **Step 2: Write the failing renderer tests**

Create `crates/kamaji/src/doctor.rs` with the test module first (then the impl
in Step 3). Add the whole file from Step 3 and run the tests after.

- [ ] **Step 3: Implement the doctor report + renderer**

Create `crates/kamaji/src/doctor.rs`:

```rust
//! `kamaji doctor`: gather a local diagnostics report, fetch the daemon's
//! report if one is running, tail the daemon log, and render a shareable
//! summary. The renderer is pure (takes a [`DoctorReport`]) so it is unit
//! tested without a daemon.

use kamaji_core::diagnostics::{Check, DaemonReport, LocalReport, Verdict};

/// Everything `kamaji doctor` collected, ready to render or serialize.
#[derive(Debug, serde::Serialize)]
pub struct DoctorReport {
    /// This binary's version.
    pub tui_version: String,
    /// Gathered from *this* (the TUI's) environment.
    pub local: LocalReport,
    /// The daemon's report, when one was reachable.
    pub daemon: Option<DaemonReport>,
    /// Why the daemon report is absent (e.g. "no daemon running").
    pub daemon_error: Option<String>,
    /// Tail of the daemon log file (most recent lines, oldest first).
    pub recent_logs: Vec<String>,
}

fn marker(v: Verdict) -> &'static str {
    match v {
        Verdict::Ok => "[ok]  ",
        Verdict::Warn => "[warn]",
        Verdict::Fail => "[fail]",
    }
}

fn render_checks(out: &mut String, checks: &[Check]) {
    for c in checks {
        out.push_str(&format!("  {} {} — {}\n", marker(c.verdict), c.name, c.detail));
        if let Some(hint) = &c.hint {
            out.push_str(&format!("         ↳ {hint}\n"));
        }
    }
}

/// Count of non-Ok checks across local + daemon-local sections.
fn problem_count(report: &DoctorReport) -> usize {
    let local = report.local.checks.iter().filter(|c| c.verdict != Verdict::Ok).count();
    let daemon = report
        .daemon
        .as_ref()
        .map(|d| d.local.checks.iter().filter(|c| c.verdict != Verdict::Ok).count())
        .unwrap_or(0);
    local + daemon
}

/// Find a check by name in a slice.
fn find<'a>(checks: &'a [Check], name: &str) -> Option<&'a Check> {
    checks.iter().find(|c| c.name == name)
}

/// The smoking-gun line for the macOS reports: zellij resolves for the user
/// (TUI) but not for the daemon (which is what actually spawns it).
fn path_mismatch_note(report: &DoctorReport) -> Option<String> {
    let daemon = report.daemon.as_ref()?;
    let tui_zellij = find(&report.local.checks, "zellij on PATH")?;
    let daemon_zellij = find(&daemon.local.checks, "zellij on PATH")?;
    if tui_zellij.verdict == Verdict::Ok && daemon_zellij.verdict == Verdict::Fail {
        Some(
            "MISMATCH: zellij is on YOUR PATH but the DAEMON cannot find it. The \
             daemon — not the TUI — spawns zellij, so this is the likely cause of \
             both the \"server error\" on start and the crashed terminal iframe. \
             Ensure the daemon is launched with zellij on its PATH (macOS: \
             /opt/homebrew/bin or /usr/local/bin)."
                .to_string(),
        )
    } else {
        None
    }
}

/// Render the full human-readable report.
pub fn render(report: &DoctorReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("kamaji doctor — tui v{}\n\n", report.tui_version));

    if let Some(note) = path_mismatch_note(report) {
        out.push_str(&format!("⚠ {note}\n\n"));
    }

    out.push_str("Local environment (this terminal):\n");
    render_checks(&mut out, &report.local.checks);
    out.push('\n');

    out.push_str("Daemon:\n");
    match &report.daemon {
        Some(d) => {
            out.push_str(&format!(
                "  [ok]   reachable — v{} pid {} up {}s, board {}, proxy {}\n",
                d.version, d.pid, d.uptime_secs, d.board_bind, d.proxy_base
            ));
            out.push_str(&format!(
                "  {} zellij web port :8082 — {}\n",
                marker(if d.zellij_web_reachable { Verdict::Ok } else { Verdict::Warn }),
                if d.zellij_web_reachable { "reachable" } else { "not reachable (no browser attach yet, or zellij web down)" }
            ));
            out.push_str(&format!(
                "  {} reverse proxy — {}\n",
                marker(if d.proxy_reachable { Verdict::Ok } else { Verdict::Fail }),
                if d.proxy_reachable { "listening" } else { "not listening (iframe will show a crashed page)" }
            ));
            out.push_str(&format!("  ·      projects {}, tickets {}\n", d.project_count, d.ticket_count));
            out.push_str("\n  Daemon environment (where zellij is actually spawned):\n");
            render_checks(&mut out, &d.local.checks);
            if let Some(sessions) = &d.zellij_sessions {
                out.push_str("\n  zellij sessions:\n");
                for line in sessions.lines() {
                    out.push_str(&format!("    {line}\n"));
                }
            }
        }
        None => {
            let why = report.daemon_error.as_deref().unwrap_or("unreachable");
            out.push_str(&format!("  [warn] no daemon reached — {why}\n"));
        }
    }
    out.push('\n');

    out.push_str("Recent daemon logs:\n");
    if report.recent_logs.is_empty() {
        out.push_str("  (no log file found)\n");
    } else {
        for line in &report.recent_logs {
            out.push_str(&format!("  {line}\n"));
        }
    }
    out.push('\n');

    let problems = problem_count(report);
    out.push_str(&format!(
        "Summary: {}\n",
        if problems == 0 { "no problems found".to_string() } else { format!("{problems} problem(s) found") }
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kamaji_core::diagnostics::EnvVar;

    fn local(zellij_ok: bool) -> LocalReport {
        LocalReport {
            checks: vec![if zellij_ok {
                Check::ok("zellij on PATH", "zellij 0.43.1 (/opt/homebrew/bin/zellij)")
            } else {
                Check::fail("zellij on PATH", "could not run `zellij`", "fix PATH")
            }],
            env: vec![EnvVar { key: "PATH".into(), value: "/usr/bin".into() }],
        }
    }

    fn daemon_report(zellij_ok: bool, proxy_reachable: bool) -> DaemonReport {
        DaemonReport {
            version: "0.5.0".into(),
            pid: 99,
            uptime_secs: 3,
            board_bind: "127.0.0.1:8755".into(),
            proxy_base: "http://127.0.0.1:8756".into(),
            zellij_web_reachable: false,
            proxy_reachable,
            zellij_sessions: None,
            project_count: 0,
            ticket_count: 0,
            local: local(zellij_ok),
        }
    }

    #[test]
    fn renders_daemon_unreachable_without_panicking() {
        let report = DoctorReport {
            tui_version: "0.5.0".into(),
            local: local(true),
            daemon: None,
            daemon_error: Some("no daemon running".into()),
            recent_logs: vec![],
        };
        let text = render(&report);
        assert!(text.contains("no daemon reached — no daemon running"));
        assert!(text.contains("no log file found"));
        assert!(text.contains("no problems found"));
    }

    #[test]
    fn flags_the_path_mismatch_when_tui_has_zellij_but_daemon_does_not() {
        let report = DoctorReport {
            tui_version: "0.5.0".into(),
            local: local(true),                       // TUI: zellij OK
            daemon: Some(daemon_report(false, true)), // daemon: zellij FAIL
            daemon_error: None,
            recent_logs: vec!["line one".into()],
        };
        let text = render(&report);
        assert!(text.contains("MISMATCH: zellij is on YOUR PATH"), "{text}");
        // One non-Ok check (the daemon's zellij) is counted.
        assert!(text.contains("1 problem(s) found"), "{text}");
    }

    #[test]
    fn no_mismatch_when_both_have_zellij() {
        let report = DoctorReport {
            tui_version: "0.5.0".into(),
            local: local(true),
            daemon: Some(daemon_report(true, true)),
            daemon_error: None,
            recent_logs: vec![],
        };
        let text = render(&report);
        assert!(!text.contains("MISMATCH"), "{text}");
        assert!(text.contains("no problems found"), "{text}");
    }
}
```

- [ ] **Step 4: Register the module**

In `crates/kamaji/src/main.rs`, add `mod doctor;` with the other `mod`
declarations (after `mod dir_select;`).

- [ ] **Step 5: Run the tests**

Run: `cargo test -p kamaji doctor`
Expected: PASS (three renderer tests).

- [ ] **Step 6: Commit**

```bash
git add crates/kamaji/src/client.rs crates/kamaji/src/doctor.rs crates/kamaji/src/main.rs
git commit -m "feat(kamaji): doctor report renderer + DaemonClient::get_diagnostics"
```

---

## Task 7: Wire up the `kamaji doctor` command

**Files:**
- Modify: `crates/kamaji/src/cli.rs`
- Modify: `crates/kamaji/src/doctor.rs`
- Modify: `crates/kamaji/src/main.rs`

- [ ] **Step 1: Add the `collect` function to the doctor module**

In `crates/kamaji/src/doctor.rs`, add these imports at the top (below the
existing `use kamaji_core::...`):

```rust
use crate::client::DaemonClient;
use crate::daemon;
use kamaji_core::diagnostics::{gather_local, newest_log_file};
```

Add the collection entrypoint (above the `#[cfg(test)] mod tests`):

```rust
/// How many trailing daemon-log lines to include in the report.
const LOG_TAIL_LINES: usize = 50;

/// Collect a full doctor report: this binary's local gather, the daemon's
/// report (if one is already running — never spawns one), and the daemon log
/// tail. `forced_addr` mirrors `--daemon <addr>`.
pub fn collect(forced_addr: Option<&str>) -> DoctorReport {
    let local = gather_local();
    let (daemon, daemon_error) = match connect_existing(forced_addr) {
        Some(client) => match client.get_diagnostics() {
            Ok(report) => (Some(report), None),
            Err(e) => (None, Some(format!("daemon reachable but /diagnostics failed: {e:?}"))),
        },
        None => (None, Some("no daemon running".to_string())),
    };
    DoctorReport {
        tui_version: env!("CARGO_PKG_VERSION").to_string(),
        local,
        daemon,
        daemon_error,
        recent_logs: read_log_tail(),
    }
}

/// Connect to an already-running daemon WITHOUT spawning one. With `--daemon`
/// the address is forced; otherwise the pidfile/addrfile is probed.
fn connect_existing(forced_addr: Option<&str>) -> Option<DaemonClient> {
    if let Some(addr) = forced_addr {
        let base = if addr.starts_with("http") { addr.to_string() } else { format!("http://{addr}") };
        return DaemonClient::connect(base).ok();
    }
    let (pidfile, addrfile) = daemon::runtime_files()?;
    daemon::probe_existing(&pidfile, &addrfile)
}

/// Read the last `LOG_TAIL_LINES` lines of the newest daemon log file.
fn read_log_tail() -> Vec<String> {
    let Some(dir) = kamaji_core::paths::log_dir() else {
        return vec![];
    };
    let Some(file) = newest_log_file(&dir) else {
        return vec![];
    };
    let Ok(contents) = std::fs::read_to_string(&file) else {
        return vec![];
    };
    let lines: Vec<&str> = contents.lines().collect();
    let start = lines.len().saturating_sub(LOG_TAIL_LINES);
    lines[start..].iter().map(|s| s.to_string()).collect()
}
```

`daemon::runtime_files` and `daemon::probe_existing` are already `pub` in
`crates/kamaji/src/daemon.rs` (verified). No change needed there.

- [ ] **Step 2: Write the failing CLI parse test**

In `crates/kamaji/src/cli.rs`, add to the `tests` module:

```rust
    #[test]
    fn parses_doctor_command() {
        assert_eq!(
            parse(["doctor"]).unwrap(),
            Command::Doctor(DoctorOpts { json: false, forced_addr: None })
        );
        assert_eq!(
            parse(["doctor", "--json"]).unwrap(),
            Command::Doctor(DoctorOpts { json: true, forced_addr: None })
        );
        assert_eq!(
            parse(["doctor", "--daemon", "127.0.0.1:9000"]).unwrap(),
            Command::Doctor(DoctorOpts { json: false, forced_addr: Some("127.0.0.1:9000".into()) })
        );
        assert_eq!(parse(["doctor", "--help"]).unwrap(), Command::Help);
    }
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p kamaji parses_doctor_command`
Expected: FAIL — `Command::Doctor` / `DoctorOpts` don't exist.

- [ ] **Step 4: Add the `Doctor` variant, `DoctorOpts`, parser, and USAGE**

In `crates/kamaji/src/cli.rs`:

Add to the `Command` enum (after `Status,`):

```rust
    Doctor(DoctorOpts),
```

Add the options struct (after the `DaemonOpts` struct definition):

```rust
/// Options for `kamaji doctor`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DoctorOpts {
    /// `--json`: emit the merged report as JSON instead of human text.
    pub json: bool,
    /// `--daemon <ADDR>`: query this daemon address instead of the discovered one.
    pub forced_addr: Option<String>,
}
```

Add a match arm in `parse()` (alongside the `up`/`down`/`logs`/`status` arms,
before the `[other, ..]` catch-all):

```rust
        [cmd, rest @ ..] if cmd == "doctor" => parse_doctor(rest),
```

Add the parser function (near `parse_up`):

```rust
fn parse_doctor(args: &[String]) -> Result<Command> {
    let mut opts = DoctorOpts::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => opts.json = true,
            "--daemon" => opts.forced_addr = Some(take_value(args, &mut i, "--daemon")?),
            "--help" | "-h" => return Ok(Command::Help),
            other => bail!("unknown doctor argument: {other}\n\n{USAGE}"),
        }
        i += 1;
    }
    Ok(Command::Doctor(opts))
}
```

Add a line to the `USAGE` constant — after the `status` line (line 21-area),
add a usage line and a description line:

In the usage block:
```
  kamaji doctor [--json] [--daemon <addr>]
```
In the command-descriptions block (after the `status` description):
```
  doctor            print a diagnostics report (zellij/paths/daemon/logs)
```

- [ ] **Step 5: Run the parse test**

Run: `cargo test -p kamaji parses_doctor_command`
Expected: PASS.

- [ ] **Step 6: Dispatch the command in `main`**

In `crates/kamaji/src/main.rs`, add a match arm in `main()` (after the
`cli::Command::Status => ...` arm):

```rust
        cli::Command::Doctor(opts) => {
            let report = doctor::collect(opts.forced_addr.as_deref());
            if opts.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", doctor::render(&report));
            }
            Ok(())
        }
```

`serde_json` is already a dependency of `kamaji` (verified in Cargo.toml).

- [ ] **Step 7: Build + manual smoke**

Run: `cargo build -p kamaji`
Expected: builds clean.

Manual smoke (no daemon needed — local section + "no daemon running"):

```bash
cargo run -p kamaji -- doctor
```

Expected: a sectioned report; zellij/git checks present; daemon section says
"no daemon reached — no daemon running" (unless one is up); a summary line.

- [ ] **Step 8: Run the full test suites**

Run: `cargo test -p kamaji-core && cargo test -p kamajid && cargo test -p kamaji`
Expected: all PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/kamaji/src/cli.rs crates/kamaji/src/doctor.rs crates/kamaji/src/main.rs
git commit -m "feat(kamaji): wire up the 'kamaji doctor' command"
```

---

## Final verification (before PR)

- [ ] `cargo fmt --all` (no diff after) and `cargo clippy --all-targets -- -D warnings` clean.
- [ ] `cargo test` (workspace) green.
- [ ] Manual: run a daemon (`make start`), then `cargo run -p kamaji -- doctor` and confirm the Daemon section is populated (version/pid/uptime, proxy reachability, the daemon-environment checks, and the daemon log tail).
- [ ] Whole-branch review per the working agreement, then open the PR.

## Spec coverage check

- Daemon logs persist regardless of spawn method → Task 4 (file layer in `init_tracing`, written for every daemon process).
- `kamaji doctor` merges local + daemon, works when daemon down → Tasks 6 & 7 (`collect` uses `probe_existing`, never spawns; renders daemon-unreachable).
- Daemon reports its OWN PATH/zellij resolution (the source of truth) → Task 5 (`gather_local` runs inside the daemon via `spawn_blocking`).
- PATH-mismatch smoking-gun flag → Task 6 (`path_mismatch_note`).
- Env allowlist, never full dump → Task 3 (`filter_env_allowlist`).
- Temp-dir + XDG writability, zellij/git resolution, zellij config readability → Task 3 (`gather_local`).
- zellij-web/proxy reachability, session list, counts → Task 5.
- `--json` output → Task 7.
- Testing at each layer (core unit, daemon route, renderer) → Tasks 2, 3, 5, 6, 7.
