//! Pure planning for container mode: runtime detection, mount derivation,
//! generated config, and the `run` argv. No process execution, no I/O — every
//! function here is unit-tested by asserting its output.

// Items in this module are wired to the CLI in later tasks; suppress the
// "never used" lint while the scaffolding is being built incrementally.
#![allow(dead_code)]

/// A supported container runtime. Podman is preferred (rootless maps
/// container-root to the unprivileged host user; see the design's trust model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Podman,
    Docker,
}

impl Runtime {
    pub fn binary(self) -> &'static str {
        match self {
            Runtime::Podman => "podman",
            Runtime::Docker => "docker",
        }
    }
}

/// Choose a runtime: an explicit `preferred` wins if present; otherwise prefer
/// Podman over Docker. `exists(bin)` reports whether a runtime binary is usable.
pub fn detect_runtime(
    exists: impl Fn(&str) -> bool,
    preferred: Option<Runtime>,
) -> Option<Runtime> {
    if let Some(r) = preferred {
        return exists(r.binary()).then_some(r);
    }
    [Runtime::Podman, Runtime::Docker]
        .into_iter()
        .find(|&r| exists(r.binary()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_podman_when_both_present() {
        let got = detect_runtime(|_| true, None);
        assert_eq!(got, Some(Runtime::Podman));
    }

    #[test]
    fn falls_back_to_docker_when_only_docker() {
        let got = detect_runtime(|b| b == "docker", None);
        assert_eq!(got, Some(Runtime::Docker));
    }

    #[test]
    fn none_when_neither_present() {
        assert_eq!(detect_runtime(|_| false, None), None);
    }

    #[test]
    fn preferred_must_exist() {
        assert_eq!(
            detect_runtime(|_| true, Some(Runtime::Docker)),
            Some(Runtime::Docker)
        );
        assert_eq!(
            detect_runtime(|b| b == "podman", Some(Runtime::Docker)),
            None
        );
    }
}
