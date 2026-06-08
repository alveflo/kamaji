//! Pure planning for container mode: runtime detection, mount derivation,
//! generated config, and the `run` argv. No process execution, no I/O — every
//! function here is unit-tested by asserting its output.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use kamaji_core::config::Config;
use kamaji_core::models::Project;

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
    let mut seen = BTreeSet::new();
    let mut mounts = Vec::new();
    let push = |path: PathBuf, mounts: &mut Vec<Mount>, seen: &mut BTreeSet<PathBuf>| {
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

/// Everything `build_run_argv` needs. Assembled by the orchestrator from the
/// host environment + the pure derivations above.
#[derive(Debug, Clone)]
pub struct RunSpec {
    pub image: String,
    pub container_name: String,
    pub home: PathBuf,
    pub data_dir: PathBuf,
    pub config_dir: PathBuf,
    pub zellij_volume: String,
    pub code_mounts: Vec<Mount>,
    pub cred_mounts: Vec<Mount>,
    pub env: Vec<(String, String)>,
    pub memory: String,
    pub cpus: String,
    pub pids_limit: u32,
}

/// Build the `run` argv (everything after the runtime binary). Runtime-agnostic:
/// detached, named, both ports published to host loopback, identical-path
/// mounts, HOME + env, and resource limits. The image is always the final arg;
/// the image's own CMD (`kamajid serve --bind 0.0.0.0:8755`) needs no override.
pub fn build_run_argv(spec: &RunSpec) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let push2 = |flag: &str, val: String, args: &mut Vec<String>| {
        args.push(flag.to_string());
        args.push(val);
    };

    args.push("run".into());
    args.push("-d".into());
    push2("--name", spec.container_name.clone(), &mut args);

    // No --userns flag: rootless Podman already maps container-root to the
    // unprivileged host user (agents are root in the box, files come back owned
    // by you). Docker's host-root mapping is documented in deploy/.
    push2("-p", "127.0.0.1:8755:8755".into(), &mut args);
    push2("-p", "127.0.0.1:8756:8756".into(), &mut args);

    // `home` is rendered via `display()` (lossy for non-UTF-8 paths), which is
    // acceptable since HOME is a conventional path and always UTF-8 in practice.
    push2("-e", format!("HOME={}", spec.home.display()), &mut args);
    for (k, v) in &spec.env {
        push2("-e", format!("{k}={v}"), &mut args);
    }

    // State + persistence.
    push2(
        "-v",
        Mount::bind(spec.data_dir.clone(), false).arg(),
        &mut args,
    );
    push2(
        "-v",
        Mount::bind(spec.config_dir.clone(), false).arg(),
        &mut args,
    );
    push2(
        "-v",
        format!(
            "{}:{}/.cache/zellij",
            spec.zellij_volume,
            spec.home.display()
        ),
        &mut args,
    );

    for m in spec.code_mounts.iter().chain(spec.cred_mounts.iter()) {
        push2("-v", m.arg(), &mut args);
    }

    push2("--memory", spec.memory.clone(), &mut args);
    push2("--cpus", spec.cpus.clone(), &mut args);
    push2("--pids-limit", spec.pids_limit.to_string(), &mut args);

    args.push(spec.image.clone());
    args
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
    use kamaji_core::config::Config;
    use kamaji_core::models::{Agent, Project};
    use std::path::PathBuf;

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
        // Both project roots plus the shared worktree base = 3 total mounts.
        assert_eq!(mounts.len(), 3, "{mounts:?}");
    }

    #[test]
    fn mount_arg_read_only_branch() {
        assert_eq!(Mount::bind("/x", true).arg(), "/x:/x:ro");
    }

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

    fn sample_spec() -> RunSpec {
        RunSpec {
            image: "ghcr.io/alveflo/kamaji:v0.1.0".into(),
            container_name: "kamaji".into(),
            home: PathBuf::from("/home/u"),
            data_dir: PathBuf::from("/home/u/.local/share/kamaji"),
            config_dir: PathBuf::from("/home/u/.config/kamaji"),
            zellij_volume: "kamaji-zellij-cache".into(),
            code_mounts: vec![Mount::bind("/home/u/dev/kamaji", false)],
            cred_mounts: vec![Mount::bind("/home/u/.claude", false)],
            env: vec![("ANTHROPIC_API_KEY".into(), "sk-xxx".into())],
            memory: "8g".into(),
            cpus: "4".into(),
            pids_limit: 2048,
        }
    }

    #[test]
    fn run_argv_publishes_both_ports_to_loopback() {
        let argv = build_run_argv(&sample_spec());
        assert_eq!(argv[0], "run");
        assert!(
            argv.windows(2).any(|w| w == ["-p", "127.0.0.1:8755:8755"]),
            "{argv:?}"
        );
        assert!(
            argv.windows(2).any(|w| w == ["-p", "127.0.0.1:8756:8756"]),
            "{argv:?}"
        );
    }

    #[test]
    fn run_argv_sets_home_volumes_limits_and_image_last() {
        let argv = build_run_argv(&sample_spec());
        assert!(
            argv.windows(2).any(|w| w == ["-e", "HOME=/home/u"]),
            "{argv:?}"
        );
        assert!(
            argv.windows(2)
                .any(|w| w == ["-e", "ANTHROPIC_API_KEY=sk-xxx"]),
            "{argv:?}"
        );
        assert!(
            argv.windows(2)
                .any(|w| w == ["-v", "kamaji-zellij-cache:/home/u/.cache/zellij"]),
            "{argv:?}"
        );
        assert!(
            argv.windows(2)
                .any(|w| w == ["-v", "/home/u/dev/kamaji:/home/u/dev/kamaji"]),
            "{argv:?}"
        );
        assert!(
            argv.windows(2)
                .any(|w| w == ["-v", "/home/u/.claude:/home/u/.claude"]),
            "{argv:?}"
        );
        assert!(argv.windows(2).any(|w| w == ["--memory", "8g"]), "{argv:?}");
        assert!(
            argv.windows(2).any(|w| w == ["--pids-limit", "2048"]),
            "{argv:?}"
        );
        assert_eq!(
            argv.last().unwrap(),
            "ghcr.io/alveflo/kamaji:v0.1.0",
            "image is the final arg"
        );
        assert!(
            argv.windows(2).any(|w| w == ["--name", "kamaji"]),
            "{argv:?}"
        );
        assert!(argv.contains(&"-d".to_string()), "runs detached");
    }

    #[test]
    fn run_argv_empty_mounts_image_last() {
        // Empty code_mounts and cred_mounts must still produce a well-formed argv
        // with the image as the final argument.
        let spec = RunSpec {
            image: "ghcr.io/alveflo/kamaji:v0.1.0".into(),
            container_name: "kamaji".into(),
            home: PathBuf::from("/home/u"),
            data_dir: PathBuf::from("/home/u/.local/share/kamaji"),
            config_dir: PathBuf::from("/home/u/.config/kamaji"),
            zellij_volume: "kamaji-zellij-cache".into(),
            code_mounts: vec![],
            cred_mounts: vec![],
            env: vec![],
            memory: "8g".into(),
            cpus: "4".into(),
            pids_limit: 2048,
        };
        let argv = build_run_argv(&spec);
        assert_eq!(argv[0], "run", "starts with run");
        assert_eq!(
            argv.last().unwrap(),
            "ghcr.io/alveflo/kamaji:v0.1.0",
            "image is the final arg even with empty mounts"
        );
    }
}
