//! The host-side marker that records a running containerized daemon. Written by
//! `kamaji up`, read by `daemon::ensure_daemon` (to connect instead of spawning
//! a local daemon) and removed by `kamaji down`.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Persisted record of the container daemon `kamaji up` started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerState {
    /// The container name (for `down`/`logs`).
    pub name: String,
    /// The host-reachable board address as a bare `host:port` (e.g.
    /// `127.0.0.1:8755`) — NO scheme. Callers prepend `http://` (see
    /// `status_report`); a scheme-bearing value here would double up.
    pub board_addr: String,
    /// The runtime binary used, e.g. `podman`.
    pub runtime: String,
}

/// Marker path under the runtime dir, next to the daemon's pid/addr files.
pub fn path() -> Option<PathBuf> {
    Some(kamaji_core::paths::runtime_dir()?.join("kamaji-container.json"))
}

impl ContainerState {
    pub fn save_to(&self, file: &Path) -> anyhow::Result<()> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(file, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn load_from(file: &Path) -> Option<ContainerState> {
        serde_json::from_slice(&std::fs::read(file).ok()?).ok()
    }
}

/// Save to the canonical [`path`].
pub fn save(state: &ContainerState) -> anyhow::Result<()> {
    let p = path().context("no runtime dir")?;
    state.save_to(&p)
}

/// Load from the canonical [`path`], if present and valid.
pub fn load() -> Option<ContainerState> {
    ContainerState::load_from(&path()?)
}

/// Remove the marker (best-effort).
pub fn clear() {
    if let Some(p) = path() {
        let _ = std::fs::remove_file(p);
    }
}

/// The active run mode, derived from the marker's presence.
#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    Native,
    Container(ContainerState),
}

/// Native unless `kamaji up` recorded a container.
pub fn active_mode() -> Mode {
    match load() {
        Some(s) => Mode::Container(s),
        None => Mode::Native,
    }
}

/// Human-readable status. `native_base` is the board URL native mode would use
/// (from `daemon.bind`); `healthy(base)` probes a board's `/healthz`.
pub fn status_report(mode: &Mode, native_base: &str, healthy: impl Fn(&str) -> bool) -> String {
    match mode {
        Mode::Native => {
            let up = if healthy(native_base) {
                "running"
            } else {
                "not running"
            };
            format!("mode: native\nboard: {native_base} ({up})")
        }
        Mode::Container(s) => {
            let base = format!("http://{}", s.board_addr);
            let up = if healthy(&base) { "up" } else { "down" };
            format!(
                "mode: container ({rt}, container {name})\nboard: {base} ({up})",
                rt = s.runtime,
                name = s.name
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("kamaji-container.json");
        let st = ContainerState {
            name: "kamaji".into(),
            board_addr: "127.0.0.1:8755".into(),
            runtime: "podman".into(),
        };
        st.save_to(&f).unwrap();
        assert_eq!(ContainerState::load_from(&f), Some(st));
    }

    #[test]
    fn load_from_absent_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            ContainerState::load_from(&dir.path().join("nope.json")),
            None
        );
    }

    #[test]
    fn status_native_reports_native_and_health() {
        let s = status_report(&Mode::Native, "http://127.0.0.1:8755", |_| false);
        assert!(s.contains("mode: native"), "{s}");
        assert!(s.contains("not running"), "{s}");
    }

    #[test]
    fn status_container_reports_runtime_and_up() {
        let st = ContainerState {
            name: "kamaji".into(),
            board_addr: "127.0.0.1:8755".into(),
            runtime: "podman".into(),
        };
        let s = status_report(&Mode::Container(st), "http://127.0.0.1:8755", |_| true);
        assert!(s.contains("mode: container"), "{s}");
        assert!(s.contains("podman"), "{s}");
        assert!(s.contains("(up)"), "{s}");
    }

    #[test]
    fn status_container_reports_down_when_unhealthy() {
        let st = ContainerState {
            name: "kamaji".into(),
            board_addr: "127.0.0.1:8755".into(),
            runtime: "podman".into(),
        };
        let s = status_report(&Mode::Container(st), "http://127.0.0.1:8755", |_| false);
        assert!(s.contains("(down)"), "{s}");
    }
}
