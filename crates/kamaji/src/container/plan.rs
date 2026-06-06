//! Pure planning for container mode: runtime detection, mount derivation,
//! generated config, and the `run` argv. No process execution, no I/O — every
//! function here is unit-tested by asserting its output.

// Items in this module are wired to the CLI in later tasks; suppress the
// "never used" lint while the scaffolding is being built incrementally.
#![allow(dead_code)]

use kamaji_core::config::Config;
use kamaji_core::models::Project;
use std::path::{Component, Path, PathBuf};

/// The default container worktree base when the user has not set one. A sibling
/// of each project root; the launcher mounts it explicitly (see
/// `derive_project_mounts`).
pub const DEFAULT_WORKTREE_BASE: &str = "{root}/../kamaji-worktrees";

/// Produce the config the *containerized* daemon should load: the user's config
/// with a `worktree_base` guaranteed to be set (the headless container has no TUI
/// to prompt for one). `daemon.bind` is deliberately left untouched — the
/// container binds the wildcard host via its image CMD, so native runs keep their
/// own (loopback) bind. All other settings carry through unchanged.
pub fn render_container_config(base: &Config) -> Config {
    let mut cfg = base.clone();
    if cfg.worktree_base.is_none() {
        cfg.worktree_base = Some(DEFAULT_WORKTREE_BASE.to_string());
    }
    cfg
}

/// A bind mount. `source` and `target` are identical for container mode so paths
/// resolve the same inside the container as on the host (see plan refinement #2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub source: PathBuf,
    pub target: PathBuf,
    pub read_only: bool,
}

impl Mount {
    /// An identical-path bind mount of `path`.
    pub fn bind(path: impl Into<PathBuf>, read_only: bool) -> Mount {
        let path = path.into();
        Mount {
            source: path.clone(),
            target: path,
            read_only,
        }
    }

    /// The `-v` argument value: `source:target` (+ `:ro` when read-only).
    pub fn arg(&self) -> String {
        let base = format!("{}:{}", self.source.display(), self.target.display());
        if self.read_only {
            format!("{base}:ro")
        } else {
            base
        }
    }
}

/// Collapse `.` and `..` lexically (no filesystem access). Used to turn
/// `/home/u/dev/kamaji/../kamaji-worktrees` into `/home/u/dev/kamaji-worktrees`.
fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The resolved worktree-base directory for a project root, expanding `{root}`
/// in `template` and collapsing `..`.
fn resolved_worktree_base(root: &Path, template: &str) -> PathBuf {
    let expanded = template.replace("{root}", &root.to_string_lossy());
    lexical_normalize(Path::new(&expanded))
}

/// Bind mounts for the agents' code: each project root **and** its resolved
/// worktree-base directory, all read-write at identical paths, deduplicated.
pub fn derive_project_mounts(projects: &[Project], worktree_base_template: &str) -> Vec<Mount> {
    let mut seen = std::collections::BTreeSet::new();
    let mut mounts = Vec::new();
    let push =
        |path: PathBuf, mounts: &mut Vec<Mount>, seen: &mut std::collections::BTreeSet<PathBuf>| {
            if seen.insert(path.clone()) {
                mounts.push(Mount::bind(path, false));
            }
        };
    for p in projects {
        let root = lexical_normalize(&p.root_dir);
        let wt = resolved_worktree_base(&root, worktree_base_template);
        push(root, &mut mounts, &mut seen);
        push(wt, &mut mounts, &mut seen);
    }
    mounts
}

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

    use kamaji_core::models::{Agent, Project};
    use std::path::PathBuf;

    fn proj(id: i64, root: &str) -> Project {
        Project {
            id,
            name: format!("p{id}"),
            root_dir: PathBuf::from(root),
            default_agent: Some(Agent::Claude),
            created_at: "2026-06-06T00:00:00Z".into(),
        }
    }

    #[test]
    fn derives_root_and_worktree_mounts_identical_paths() {
        let projects = [proj(1, "/home/u/dev/kamaji")];
        let mounts = derive_project_mounts(&projects, "{root}/../kamaji-worktrees");
        let args: Vec<String> = mounts.iter().map(Mount::arg).collect();
        // Root mounted at the identical path, read-write.
        assert!(
            args.contains(&"/home/u/dev/kamaji:/home/u/dev/kamaji".to_string()),
            "{args:?}"
        );
        // Worktree sibling dir resolved (.. collapsed) and mounted, read-write.
        assert!(
            args.contains(&"/home/u/dev/kamaji-worktrees:/home/u/dev/kamaji-worktrees".to_string()),
            "{args:?}"
        );
    }

    #[test]
    fn dedupes_shared_worktree_base() {
        // Two projects under the same parent share one worktree base dir.
        let projects = [proj(1, "/home/u/dev/a"), proj(2, "/home/u/dev/b")];
        let mounts = derive_project_mounts(&projects, "{root}/../kamaji-worktrees");
        let wt = mounts
            .iter()
            .filter(|m| m.source == *Path::new("/home/u/dev/kamaji-worktrees"))
            .count();
        assert_eq!(wt, 1, "shared worktree base mounted once");
    }

    use kamaji_core::config::Config;

    #[test]
    fn container_config_leaves_bind_untouched() {
        // Native must keep binding loopback; the container overrides via its CMD.
        let cfg = render_container_config(&Config::default());
        assert_eq!(cfg.daemon.bind, "127.0.0.1:8755");
    }

    #[test]
    fn container_config_sets_worktree_base_when_unset() {
        // Default config leaves worktree_base None; container mode must pick one.
        let cfg = render_container_config(&Config::default());
        assert_eq!(
            cfg.worktree_base.as_deref(),
            Some("{root}/../kamaji-worktrees")
        );
    }

    #[test]
    fn container_config_keeps_existing_worktree_base() {
        let base = Config {
            worktree_base: Some("/custom/wt".into()),
            ..Config::default()
        };
        let cfg = render_container_config(&base);
        assert_eq!(cfg.worktree_base.as_deref(), Some("/custom/wt"));
    }
}
