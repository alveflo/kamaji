# Containerized kamaji + `kamaji up`/`down` Launcher — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship kamaji as a sandbox container (daemon + zellij + all agents in one box) plus a `kamaji up`/`down` launcher so an agent can run with root *inside the container* without risking the host.

**Architecture:** A multi-stage image holds `kamajid`, `zellij`, `git`, and the agent CLIs. New `container` module in the `kamaji` binary holds pure planning logic (runtime detection, mount derivation, run-argv assembly, generated config) and thin orchestration (`up`/`down`/`logs`) that shells out to `podman`/`docker`. The host client learns to connect to a containerized daemon via a small state file instead of auto-spawning a local one. Identical-path bind mounts (and matching `HOME`) keep git worktrees and XDG paths valid both inside the container and to host tooling.

**Tech Stack:** Rust (existing `kamaji`/`kamaji-core` crates, hand-rolled CLI, `std::process::Command`, `serde_json`, `reqwest::blocking`), a Debian/Node-based container image, Podman (rootless, first-class) + Docker, GitHub Actions, Podman Quadlet + Docker Compose.

**Spec:** `docs/superpowers/specs/2026-06-06-containerize-kamaji-design.md`

---

## Design notes & refinements (read first)

The spec is correct in intent; two details are made precise here:

1. **Worktree mounts.** The default `worktree_base` (`{root}/../kamaji-worktrees`) is a *sibling* of the project root, not a child. So mounting only each project root would leave worktrees off-mount (invisible to the host, and with absolute-path links that don't resolve there). The launcher therefore mounts **both** the project root **and** the resolved worktree-base directory, each at its identical path (Task 4).
2. **`HOME` + identical paths.** For agent credentials and kamaji's XDG paths to resolve to the *same* locations inside the container as on the host, the container runs with `-e HOME=<host $HOME>` and every mount uses the identical host path. This makes `~/.config/kamaji`, `~/.local/share/kamaji`, `~/.claude`, and worktrees line up on both sides (Tasks 4, 6, 8).
3. **Native stays the default; container is opt-in.** The choice is global (whole app): run `kamaji` / `make start` for native (zero-config, unchanged), or `kamaji up` to run everything in the container; `kamaji down` returns to native. The container-aware `ensure_daemon` step (Task 10) is a no-op without a marker, so native is untouched, and `kamaji status` (Tasks 7, 8, 9) makes the active mode visible. Container mode must **not** mutate the user's `daemon.bind` (Task 5) — the container binds `0.0.0.0` via its image CMD, so native keeps binding loopback.

## File structure

| File | Responsibility |
|------|----------------|
| `Dockerfile` | Multi-stage image: build `kamajid`, assemble runtime with zellij + git + agent CLIs. |
| `.dockerignore` | Keep `target/` and VCS noise out of the build context. |
| `.github/workflows/image.yml` | Build + smoke-test the image; push to GHCR on version tags. |
| `crates/kamaji/src/container/mod.rs` | Orchestration: `up()`, `down()`, `logs()`. Wires planning + state, shells out to the runtime. |
| `crates/kamaji/src/container/plan.rs` | **Pure**: `Runtime`, `detect_runtime`, `Mount`, `derive_project_mounts`, `render_container_config`, `RunSpec`, `build_run_argv`. Fully unit-tested. |
| `crates/kamaji/src/container/state.rs` | The host-side container-state marker file (`ContainerState`: save/load/clear). |
| `crates/kamaji/src/cli.rs` | Add `Up`/`Down`/`Logs` to `Command` + arg parsing + usage. |
| `crates/kamaji/src/main.rs` | Register `mod container`; dispatch the new commands. |
| `crates/kamaji/src/daemon.rs` | `ensure_daemon` learns to connect to a containerized daemon via the state file. |
| `deploy/kamaji.container` | Podman Quadlet unit (raw escape hatch). |
| `deploy/docker-compose.yml` | Compose file (raw escape hatch). |
| `README.md`, `ARCHITECTURE.md` | Document container mode + install/launch/shutdown UX. |

## Conventions for this plan

- Run all commands from the worktree root unless a path says otherwise.
- The binary crate has **no `--lib`**; run crate tests with `cargo test -p kamaji`.
- After each task: `cargo fmt --all` and `cargo clippy --all-targets -- -D warnings` must be clean before the commit step.
- Commit messages end with the repo's co-author trailer:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`

---

## Phase A — The container image

### Task 1: Multi-stage Dockerfile + .dockerignore

**Files:**
- Create: `Dockerfile`
- Create: `.dockerignore`

- [ ] **Step 1: Write `.dockerignore`**

```
target/
.git/
docs/
crates/*/target/
**/*.log
```

- [ ] **Step 2: Write the `Dockerfile`**

```dockerfile
# syntax=docker/dockerfile:1

# ---- builder: compile kamajid (release) ----
FROM rust:1-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p kamajid

# ---- runtime: node base (gives node+npm on debian-slim) ----
FROM node:22-bookworm-slim AS runtime

ARG ZELLIJ_VERSION=v0.43.1

# git for worktrees; curl/ca-certificates to fetch zellij; tini for PID 1 reaping.
RUN apt-get update \
 && apt-get install -y --no-install-recommends git curl ca-certificates tini \
 && rm -rf /var/lib/apt/lists/*

# zellij prebuilt (musl static runs fine on glibc). amd64 only in v1 (see plan).
RUN curl -fsSL "https://github.com/zellij-org/zellij/releases/download/${ZELLIJ_VERSION}/zellij-x86_64-unknown-linux-musl.tar.gz" \
      | tar -xz -C /usr/local/bin \
 && zellij --version

# Agent CLIs. Package names are verified by the --version smoke below; if any
# changes upstream, the build fails loudly here rather than at runtime.
RUN npm install -g @anthropic-ai/claude-code @openai/codex @github/copilot \
 && claude --version && codex --version && copilot --version

COPY --from=builder /src/target/release/kamajid /usr/local/bin/kamajid

EXPOSE 8755 8756
# tini reaps zombie agent/zellij children. --bind 0.0.0.0 so the board is
# reachable from the host; the proxy auto-derives 0.0.0.0:8756.
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["kamajid", "serve", "--bind", "0.0.0.0:8755"]
```

- [ ] **Step 3: Build the image**

Run: `docker build -t kamaji:dev .` (or `podman build -t kamaji:dev .`)
Expected: build completes; the `zellij --version`, `claude --version`, `codex --version`, `copilot --version` lines all succeed during the build.

- [ ] **Step 4: Smoke-test the binaries inside the image**

Run:
```bash
docker run --rm kamaji:dev kamajid --version
docker run --rm --entrypoint zellij kamaji:dev --version
```
Expected: prints `kamajid <version>` and `zellij 0.43.x`.

- [ ] **Step 5: Commit**

```bash
git add Dockerfile .dockerignore
git commit -m "feat(image): multi-stage Dockerfile with kamajid, zellij, agent CLIs

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: CI workflow — build + smoke the image, push on tags

**Files:**
- Create: `.github/workflows/image.yml`

- [ ] **Step 1: Write the workflow**

```yaml
name: image
on:
  push:
    branches: [main]
    tags: ["v*"]
  pull_request:
permissions:
  contents: read
  packages: write
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Build image
        run: docker build -t kamaji:ci .
      - name: Smoke-test binaries
        run: |
          docker run --rm kamaji:ci kamajid --version
          docker run --rm --entrypoint zellij kamaji:ci --version
          docker run --rm --entrypoint claude kamaji:ci --version
      - name: Log in to GHCR
        if: startsWith(github.ref, 'refs/tags/v')
        uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}
      - name: Push tagged image
        if: startsWith(github.ref, 'refs/tags/v')
        run: |
          REF="ghcr.io/${{ github.repository_owner }}/kamaji:${GITHUB_REF_NAME}"
          docker tag kamaji:ci "$REF"
          docker push "$REF"
```

- [ ] **Step 2: Validate the workflow YAML**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/image.yml')); print('ok')"`
Expected: `ok`

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/image.yml
git commit -m "ci(image): build, smoke-test, and push the container image on tags

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Phase B — Launcher pure core

> All of Phase B lives under `crates/kamaji/src/container/`. Create the module dir as you add files; it is registered in `main.rs` in Task 8.

### Task 3: `Runtime` enum + `detect_runtime`

**Files:**
- Create: `crates/kamaji/src/container/plan.rs`
- Test: same file (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

In a new `crates/kamaji/src/container/plan.rs`:

```rust
//! Pure planning for container mode: runtime detection, mount derivation,
//! generated config, and the `run` argv. No process execution, no I/O — every
//! function here is unit-tested by asserting its output.

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
pub fn detect_runtime(exists: impl Fn(&str) -> bool, preferred: Option<Runtime>) -> Option<Runtime> {
    if let Some(r) = preferred {
        return exists(r.binary()).then_some(r);
    }
    for r in [Runtime::Podman, Runtime::Docker] {
        if exists(r.binary()) {
            return Some(r);
        }
    }
    None
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
        assert_eq!(detect_runtime(|_| true, Some(Runtime::Docker)), Some(Runtime::Docker));
        assert_eq!(detect_runtime(|b| b == "podman", Some(Runtime::Docker)), None);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails (module not wired)**

Run: `cargo test -p kamaji container::plan 2>&1 | tail -20`
Expected: FAIL — `file not found for module container` / unresolved module, because `mod container;` isn't declared yet.

- [ ] **Step 3: Make the module compile**

Create `crates/kamaji/src/container/mod.rs` with a single line so the test target sees `plan`:

```rust
pub mod plan;
```

Add `mod container;` to `crates/kamaji/src/main.rs` immediately after the existing `mod cli;` line (line 8):

```rust
mod cli;
mod client;
mod container;
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p kamaji container::plan::tests -- --nocapture 2>&1 | tail -20`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add crates/kamaji/src/container/mod.rs crates/kamaji/src/container/plan.rs crates/kamaji/src/main.rs
git commit -m "feat(container): Runtime enum + detect_runtime (podman-first)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: `Mount` + `derive_project_mounts` (root + worktree base, identical paths, deduped)

**Files:**
- Modify: `crates/kamaji/src/container/plan.rs`

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `plan.rs`:

```rust
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
        assert!(args.contains(&"/home/u/dev/kamaji:/home/u/dev/kamaji".to_string()), "{args:?}");
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
            .filter(|m| m.source == PathBuf::from("/home/u/dev/kamaji-worktrees"))
            .count();
        assert_eq!(wt, 1, "shared worktree base mounted once");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p kamaji container::plan::tests::derives_root_and_worktree_mounts_identical_paths 2>&1 | tail -20`
Expected: FAIL — `cannot find type Mount` / `function derive_project_mounts not found`.

- [ ] **Step 3: Write the implementation**

Add to `plan.rs` (above the tests module):

```rust
use kamaji_core::models::Project;
use std::path::{Component, Path, PathBuf};

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
        Mount { source: path.clone(), target: path, read_only }
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
    let push = |path: PathBuf, mounts: &mut Vec<Mount>, seen: &mut std::collections::BTreeSet<PathBuf>| {
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
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p kamaji container::plan::tests 2>&1 | tail -20`
Expected: PASS (all plan tests, including the two new ones).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add crates/kamaji/src/container/plan.rs
git commit -m "feat(container): derive identical-path root + worktree mounts from projects

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: `render_container_config` (ensure worktree_base; bind comes from the image CMD)

**Files:**
- Modify: `crates/kamaji/src/container/plan.rs`

- [ ] **Step 1: Write the failing test**

Append to the `tests` module:

```rust
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
        assert_eq!(cfg.worktree_base.as_deref(), Some("{root}/../kamaji-worktrees"));
    }

    #[test]
    fn container_config_keeps_existing_worktree_base() {
        let mut base = Config::default();
        base.worktree_base = Some("/custom/wt".into());
        let cfg = render_container_config(&base);
        assert_eq!(cfg.worktree_base.as_deref(), Some("/custom/wt"));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p kamaji container::plan::tests::container_config_leaves_bind_untouched 2>&1 | tail -20`
Expected: FAIL — `function render_container_config not found`.

- [ ] **Step 3: Write the implementation**

Add to `plan.rs`:

```rust
use kamaji_core::config::Config;

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
```

(Remove the now-duplicate `use kamaji_core::config::Config;` if Task 4's block already imported it — keep a single import at the top of the file.)

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p kamaji container::plan::tests 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add crates/kamaji/src/container/plan.rs
git commit -m "feat(container): ensure worktree_base in generated config (bind via image CMD)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: `RunSpec` + `build_run_argv`

**Files:**
- Modify: `crates/kamaji/src/container/plan.rs`

- [ ] **Step 1: Write the failing test**

Append to the `tests` module:

```rust
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
        assert!(argv.windows(2).any(|w| w == ["-p", "127.0.0.1:8755:8755"]), "{argv:?}");
        assert!(argv.windows(2).any(|w| w == ["-p", "127.0.0.1:8756:8756"]), "{argv:?}");
    }

    #[test]
    fn run_argv_sets_home_volumes_limits_and_image_last() {
        let argv = build_run_argv(&sample_spec());
        assert!(argv.windows(2).any(|w| w == ["-e", "HOME=/home/u"]), "{argv:?}");
        assert!(argv.windows(2).any(|w| w == ["-e", "ANTHROPIC_API_KEY=sk-xxx"]), "{argv:?}");
        assert!(argv.windows(2).any(|w| w == ["-v", "kamaji-zellij-cache:/home/u/.cache/zellij"]), "{argv:?}");
        assert!(argv.windows(2).any(|w| w == ["-v", "/home/u/dev/kamaji:/home/u/dev/kamaji"]), "{argv:?}");
        assert!(argv.windows(2).any(|w| w == ["-v", "/home/u/.claude:/home/u/.claude"]), "{argv:?}");
        assert!(argv.windows(2).any(|w| w == ["--memory", "8g"]), "{argv:?}");
        assert!(argv.windows(2).any(|w| w == ["--pids-limit", "2048"]), "{argv:?}");
        assert_eq!(argv.last().unwrap(), "ghcr.io/alveflo/kamaji:v0.1.0", "image is the final arg");
        assert!(argv.contains(&"--name".to_string()) && argv.contains(&"kamaji".to_string()));
        assert!(argv.contains(&"-d".to_string()), "runs detached");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p kamaji container::plan::tests::run_argv_publishes_both_ports_to_loopback 2>&1 | tail -20`
Expected: FAIL — `cannot find type RunSpec` / `build_run_argv not found`.

- [ ] **Step 3: Write the implementation**

Add to `plan.rs`:

```rust
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
    let mut a: Vec<String> = Vec::new();
    let push2 = |flag: &str, val: String, a: &mut Vec<String>| {
        a.push(flag.to_string());
        a.push(val);
    };

    a.push("run".into());
    a.push("-d".into());
    push2("--name", spec.container_name.clone(), &mut a);

    // No --userns flag: rootless Podman already maps container-root to the
    // unprivileged host user (agents are root in the box, files come back owned
    // by you). Docker's host-root mapping is documented in deploy/.
    push2("-p", "127.0.0.1:8755:8755".into(), &mut a);
    push2("-p", "127.0.0.1:8756:8756".into(), &mut a);

    push2("-e", format!("HOME={}", spec.home.display()), &mut a);
    for (k, v) in &spec.env {
        push2("-e", format!("{k}={v}"), &mut a);
    }

    // State + persistence.
    push2("-v", Mount::bind(spec.data_dir.clone(), false).arg(), &mut a);
    push2("-v", Mount::bind(spec.config_dir.clone(), false).arg(), &mut a);
    push2(
        "-v",
        format!("{}:{}/.cache/zellij", spec.zellij_volume, spec.home.display()),
        &mut a,
    );

    for m in spec.code_mounts.iter().chain(spec.cred_mounts.iter()) {
        push2("-v", m.arg(), &mut a);
    }

    push2("--memory", spec.memory.clone(), &mut a);
    push2("--cpus", spec.cpus.clone(), &mut a);
    push2("--pids-limit", spec.pids_limit.to_string(), &mut a);

    a.push(spec.image.clone());
    a
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p kamaji container::plan::tests 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add crates/kamaji/src/container/plan.rs
git commit -m "feat(container): RunSpec + build_run_argv (ports, mounts, HOME, limits)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: `ContainerState` marker + mode/status helpers

**Files:**
- Create: `crates/kamaji/src/container/state.rs`
- Modify: `crates/kamaji/src/container/mod.rs` (add `pub mod state;`)

- [ ] **Step 1: Write the failing test**

Create `crates/kamaji/src/container/state.rs`:

```rust
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
    /// The host-reachable board address, e.g. `127.0.0.1:8755`.
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
            let up = if healthy(native_base) { "running" } else { "not running" };
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
        assert_eq!(ContainerState::load_from(&dir.path().join("nope.json")), None);
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
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p kamaji container::state 2>&1 | tail -20`
Expected: FAIL — module `state` not declared in `container`.

- [ ] **Step 3: Wire the module**

Edit `crates/kamaji/src/container/mod.rs`:

```rust
pub mod plan;
pub mod state;
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p kamaji container::state::tests 2>&1 | tail -20`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add crates/kamaji/src/container/state.rs crates/kamaji/src/container/mod.rs
git commit -m "feat(container): ContainerState marker + mode/status helpers

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Phase C — Wiring & integration

### Task 8: Orchestration — `container::{up, down, logs}`

**Files:**
- Modify: `crates/kamaji/src/container/mod.rs`

This module shells out to the runtime, so its correctness is verified by the end-to-end smoke (Task 13) rather than unit tests; the logic it composes is already tested in Tasks 3–7. Keep it thin.

- [ ] **Step 1: Write the orchestration**

Replace `crates/kamaji/src/container/mod.rs` with:

```rust
//! Container-mode orchestration: start (`up`), stop (`down`), and follow logs
//! for the single sandbox container that holds the daemon + zellij + all agents.
//! Pure planning lives in [`plan`]; the host-side marker in [`state`].

pub mod plan;
pub mod state;

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use kamaji_core::config::{self, Config};
use kamaji_core::db::Db;
use kamaji_core::paths;

use self::plan::{
    build_run_argv, derive_project_mounts, render_container_config, Mount, Runtime, RunSpec,
};
use self::state::ContainerState;

const CONTAINER_NAME: &str = "kamaji";
const ZELLIJ_VOLUME: &str = "kamaji-zellij-cache";
const BOARD_ADDR: &str = "127.0.0.1:8755";

/// Options parsed from `kamaji up`.
#[derive(Debug, Clone, Default)]
pub struct UpArgs {
    pub runtime: Option<Runtime>,
    pub build: bool,
    pub memory: Option<String>,
    pub cpus: Option<String>,
    pub pids_limit: Option<u32>,
}

fn binary_exists(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn image_ref() -> String {
    format!("ghcr.io/alveflo/kamaji:v{}", env!("CARGO_PKG_VERSION"))
}

fn host_home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

/// Credential dirs to mount when present on the host (best-effort).
fn cred_mounts(home: &std::path::Path) -> Vec<Mount> {
    [".claude", ".codex", ".config/github-copilot"]
        .iter()
        .map(|rel| home.join(rel))
        .filter(|p| p.exists())
        .map(|p| Mount::bind(p, false))
        .collect()
}

/// Agent API keys to pass through when set in the host environment.
fn env_passthrough() -> Vec<(String, String)> {
    ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GITHUB_TOKEN", "GH_TOKEN"]
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect()
}

/// Read the registered projects from the shared DB (empty if it doesn't exist
/// yet — the very first `up` runs with the board only).
fn registered_projects() -> Result<Vec<kamaji_core::models::Project>> {
    let db_path = paths::data_dir().context("cannot determine data dir")?.join("kamaji.db");
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    Db::open(&db_path)?.list_projects()
}

/// Start (or restart) the sandbox container.
pub fn up(args: &UpArgs) -> Result<()> {
    let runtime = plan::detect_runtime(binary_exists, args.runtime)
        .ok_or_else(|| anyhow!("no container runtime found — install podman (recommended) or docker"))?;
    let bin = runtime.binary();
    let image = image_ref();

    // 1. Ensure the image is available.
    if args.build {
        run_checked(bin, &["build", "-t", &image, "."], "building image")?;
    } else if !run_ok(bin, &["image", "exists", &image]) && !run_ok(bin, &["pull", &image]) {
        bail!("could not pull {image}; re-run with --build to build it locally");
    }

    // 2. Ensure worktree_base in the shared config (bind comes from the image CMD).
    let base = config::load_or_init()?;
    let cfg = render_container_config(&base);
    let cfg_path = config::config_path()?;
    config::save_to(&cfg_path, &cfg).with_context(|| format!("writing {}", cfg_path.display()))?;

    // 3. Pre-create worktree-base dirs on the host so their bind mounts exist.
    let projects = registered_projects()?;
    let wt_template = cfg.worktree_base.as_deref().unwrap_or(plan::DEFAULT_WORKTREE_BASE);
    let code_mounts = derive_project_mounts(&projects, wt_template);
    for m in &code_mounts {
        std::fs::create_dir_all(&m.source)
            .with_context(|| format!("creating mount source {}", m.source.display()))?;
    }

    // 4. Assemble the spec and run.
    let home = host_home()?;
    let spec = RunSpec {
        image: image.clone(),
        container_name: CONTAINER_NAME.into(),
        home: home.clone(),
        data_dir: paths::data_dir().context("data dir")?,
        config_dir: paths::config_dir().context("config dir")?,
        zellij_volume: ZELLIJ_VOLUME.into(),
        code_mounts,
        cred_mounts: cred_mounts(&home),
        env: env_passthrough(),
        memory: args.memory.clone().unwrap_or_else(|| "8g".into()),
        cpus: args.cpus.clone().unwrap_or_else(|| "4".into()),
        pids_limit: args.pids_limit.unwrap_or(2048),
    };

    // Recreate idempotently so new mounts take effect.
    let _ = run_ok(bin, &["rm", "-f", CONTAINER_NAME]);
    let argv = build_run_argv(&spec);
    run_checked(bin, &argv.iter().map(String::as_str).collect::<Vec<_>>(), "starting container")?;

    // 5. Wait for health on the published port, then record state.
    crate::daemon::wait_for_health(&format!("http://{BOARD_ADDR}"), Duration::from_secs(30))
        .map_err(|e| anyhow!("container started but board never became healthy: {e}"))?;
    state::save(&ContainerState {
        name: CONTAINER_NAME.into(),
        board_addr: BOARD_ADDR.into(),
        runtime: bin.into(),
    })?;

    println!("kamaji is up — open http://{BOARD_ADDR}");
    Ok(())
}

/// Stop and remove the sandbox container (volumes persist).
pub fn down() -> Result<()> {
    let runtime = state::load().map(|s| s.runtime).unwrap_or_else(|| {
        plan::detect_runtime(binary_exists, None).map(|r| r.binary().to_string()).unwrap_or_default()
    });
    if runtime.is_empty() {
        bail!("no container runtime found");
    }
    let _ = run_ok(&runtime, &["rm", "-f", CONTAINER_NAME]);
    state::clear();
    println!("kamaji is down");
    Ok(())
}

/// Follow the container's logs.
pub fn logs() -> Result<()> {
    let st = state::load().context("no running container — run `kamaji up` first")?;
    let status = Command::new(&st.runtime).args(["logs", "-f", &st.name]).status()?;
    if !status.success() {
        bail!("{} logs exited with {status}", st.runtime);
    }
    Ok(())
}

/// Print whether kamaji is running natively or in a container, and where.
pub fn status() -> Result<()> {
    let cfg = config::load_or_init()?;
    let native_base = format!("http://{}", cfg.daemon.bind);
    let mode = state::active_mode();
    let report = state::status_report(&mode, &native_base, |base| {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(300))
            .build()
            .ok()
            .and_then(|c| c.get(format!("{base}/healthz")).send().ok())
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    });
    println!("{report}");
    Ok(())
}

fn run_ok(bin: &str, args: &[&str]) -> bool {
    Command::new(bin).args(args).output().map(|o| o.status.success()).unwrap_or(false)
}

fn run_checked(bin: &str, args: &[&str], what: &str) -> Result<()> {
    let status = Command::new(bin).args(args).status().with_context(|| format!("{what}: spawning {bin}"))?;
    if !status.success() {
        bail!("{what}: {bin} exited with {status}");
    }
    Ok(())
}
```

> Note: this requires `wait_for_health` to be reachable from `container`. It is already `pub fn` in `daemon.rs` (`crate::daemon::wait_for_health`). No change needed there.

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p kamaji 2>&1 | tail -20`
Expected: builds clean (warnings about unused `up`/`down`/`logs` are fine until Task 9 wires them).

- [ ] **Step 3: Run the existing tests to confirm nothing broke**

Run: `cargo test -p kamaji 2>&1 | tail -20`
Expected: PASS (all existing + Phase B tests).

- [ ] **Step 4: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add crates/kamaji/src/container/mod.rs
git commit -m "feat(container): up/down/logs orchestration over podman/docker

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 9: CLI parsing + dispatch for `up`/`down`/`logs`/`status`

**Files:**
- Modify: `crates/kamaji/src/cli.rs`
- Modify: `crates/kamaji/src/main.rs`

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `cli.rs`:

```rust
    #[test]
    fn parses_up_with_flags() {
        let parsed = parse(["up", "--build", "--runtime", "docker", "--memory", "4g"]).unwrap();
        let Command::Up(args) = parsed else { panic!("expected Up") };
        assert!(args.build);
        assert_eq!(args.runtime, Some(crate::container::plan::Runtime::Docker));
        assert_eq!(args.memory.as_deref(), Some("4g"));
    }

    #[test]
    fn parses_bare_up_down_logs_status() {
        assert!(matches!(parse(["up"]).unwrap(), Command::Up(_)));
        assert_eq!(parse(["down"]).unwrap(), Command::Down);
        assert_eq!(parse(["logs"]).unwrap(), Command::Logs);
        assert_eq!(parse(["status"]).unwrap(), Command::Status);
    }

    #[test]
    fn up_rejects_unknown_runtime() {
        let err = parse(["up", "--runtime", "lxc"]).unwrap_err().to_string();
        assert!(err.contains("runtime"), "{err}");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p kamaji cli::tests::parses_bare_up_down_logs_status 2>&1 | tail -20`
Expected: FAIL — `no variant Up`/`Down`/`Logs`/`Status` on `Command`.

- [ ] **Step 3: Extend the `Command` enum and parser**

In `cli.rs`, add the import near the top:

```rust
use crate::container::{plan::Runtime, UpArgs};
```

Add variants to `Command` (after `CreateTicket(CreateTicketArgs)`):

```rust
    Up(UpArgs),
    Down,
    Logs,
    Status,
```

> `UpArgs` derives `Default` and `Clone`; add `PartialEq, Eq` to its derive in `container/mod.rs` so `Command` keeps `PartialEq, Eq`. Update its derive line to:
> `#[derive(Debug, Clone, Default, PartialEq, Eq)]`

In `parse()`, add these match arms to the top-level `match args.as_slice()` block, before the `[other, ..]` catch-all:

```rust
        [cmd, rest @ ..] if cmd == "up" => parse_up(rest),
        [cmd, ..] if cmd == "down" => Ok(Command::Down),
        [cmd, ..] if cmd == "logs" => Ok(Command::Logs),
        [cmd, ..] if cmd == "status" => Ok(Command::Status),
```

Add the `parse_up` helper alongside `parse_ticket_create`:

```rust
fn parse_up(args: &[String]) -> Result<Command> {
    let mut up = UpArgs::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--build" => up.build = true,
            "--runtime" => {
                up.runtime = Some(match take_value(args, &mut i, "--runtime")?.as_str() {
                    "podman" => Runtime::Podman,
                    "docker" => Runtime::Docker,
                    other => bail!("unknown --runtime {other:?} (use podman or docker)"),
                });
            }
            "--memory" => up.memory = Some(take_value(args, &mut i, "--memory")?),
            "--cpus" => up.cpus = Some(take_value(args, &mut i, "--cpus")?),
            "--pids-limit" => {
                let v = take_value(args, &mut i, "--pids-limit")?;
                up.pids_limit = Some(v.parse().map_err(|_| anyhow!("--pids-limit must be a number"))?);
            }
            "--help" | "-h" => return Ok(Command::Help),
            other => bail!("unknown up option: {other}\n\n{USAGE}"),
        }
        i += 1;
    }
    Ok(Command::Up(up))
}
```

Extend the `USAGE` string with the new commands:

```rust
const USAGE: &str = "\
Usage:
  kamaji
  kamaji up [--build] [--runtime podman|docker] [--memory <m>] [--cpus <n>] [--pids-limit <n>]
  kamaji down
  kamaji logs
  kamaji status
  kamaji ticket create --prompt <prompt> [--title <title>] [--description <text>] [--agent <agent>] [--project <id-or-name>] [--background]
  kamaji ticket create <prompt> [--title <title>] [--description <text>] [--agent <agent>] [--project <id-or-name>] [--background]

Agents: claude, codex, copilot

  up                run the sandbox container (daemon + zellij + agents)
  down              stop the sandbox container (back to native)
  logs              follow the container's logs
  status            show the active mode (native vs container) + board URL
  --background, -b  also start the ticket's agent in a detached zellij session
";
```

- [ ] **Step 4: Dispatch in `main.rs`**

In `main.rs`, add arms to the `match cli::parse(...)` block (after the `CreateTicket` arm):

```rust
        cli::Command::Up(args) => container::up(&args),
        cli::Command::Down => container::down(),
        cli::Command::Logs => container::logs(),
        cli::Command::Status => container::status(),
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p kamaji cli::tests 2>&1 | tail -20`
Expected: PASS (existing + 3 new).

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add crates/kamaji/src/cli.rs crates/kamaji/src/main.rs crates/kamaji/src/container/mod.rs
git commit -m "feat(cli): kamaji up/down/logs/status commands

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 10: Container-aware `ensure_daemon`

**Files:**
- Modify: `crates/kamaji/src/daemon.rs`

The client must connect to the containerized daemon (and NOT auto-spawn a local one) when `kamaji up` has recorded a container. Make the decision a pure, tested function, then call it from `ensure_daemon`.

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `daemon.rs`:

```rust
    #[test]
    fn container_resolution_connects_when_healthy() {
        let st = crate::container::state::ContainerState {
            name: "kamaji".into(),
            board_addr: "127.0.0.1:8755".into(),
            runtime: "podman".into(),
        };
        let got = resolve_container(Some(st), |base| base == "http://127.0.0.1:8755");
        assert!(matches!(got, ContainerResolution::Connect(ref b) if b == "http://127.0.0.1:8755"));
    }

    #[test]
    fn container_resolution_errors_when_present_but_unhealthy() {
        let st = crate::container::state::ContainerState {
            name: "kamaji".into(),
            board_addr: "127.0.0.1:8755".into(),
            runtime: "podman".into(),
        };
        assert!(matches!(resolve_container(Some(st), |_| false), ContainerResolution::Down));
    }

    #[test]
    fn container_resolution_absent_is_no_container() {
        assert!(matches!(resolve_container(None, |_| true), ContainerResolution::None));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p kamaji daemon::tests::container_resolution_absent_is_no_container 2>&1 | tail -20`
Expected: FAIL — `cannot find ContainerResolution` / `resolve_container`.

- [ ] **Step 3: Add the decision function**

Add to `daemon.rs` (above the tests module):

```rust
use crate::container::state::ContainerState;

/// What to do given the container-state marker and a health check.
#[derive(Debug, PartialEq, Eq)]
pub enum ContainerResolution {
    /// Connect to this base URL (a healthy container daemon).
    Connect(String),
    /// A container is recorded but unreachable — the user should run `kamaji up`.
    Down,
    /// No container recorded; fall through to normal local discovery.
    None,
}

/// Pure decision: with a recorded container, probe its board; otherwise no-op.
pub fn resolve_container(
    state: Option<ContainerState>,
    healthy: impl Fn(&str) -> bool,
) -> ContainerResolution {
    match state {
        Some(st) => {
            let base = format!("http://{}", st.board_addr);
            if healthy(&base) {
                ContainerResolution::Connect(base)
            } else {
                ContainerResolution::Down
            }
        }
        None => ContainerResolution::None,
    }
}
```

- [ ] **Step 4: Call it from `ensure_daemon`**

In `ensure_daemon`, immediately after the `forced_addr` early-return block and before `let (pidfile, addrfile) = ...`, insert:

```rust
    // Container mode: if `kamaji up` recorded a containerized daemon, connect to
    // it (or report it down) instead of auto-spawning a local one.
    match resolve_container(crate::container::state::load(), |base| {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(300))
            .build()
            .ok()
            .and_then(|c| c.get(format!("{base}/healthz")).send().ok())
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }) {
        ContainerResolution::Connect(base) => {
            return DaemonClient::connect(base).map_err(|e| format!("{e:?}"));
        }
        ContainerResolution::Down => {
            return Err("kamaji container is not responding — run `kamaji up`".into());
        }
        ContainerResolution::None => {}
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p kamaji daemon::tests 2>&1 | tail -20`
Expected: PASS (existing + 3 new).

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
git add crates/kamaji/src/daemon.rs
git commit -m "feat(daemon): connect to a containerized daemon via the state marker

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Phase D — Escape hatches & docs

### Task 11: Podman Quadlet unit + Docker Compose file

**Files:**
- Create: `deploy/kamaji.container`
- Create: `deploy/docker-compose.yml`
- Create: `deploy/README.md`

These document the raw conventions for users who don't want the launcher. They use placeholders the user fills in (`<HOME>`, `<PROJECT_ROOT>`); call that out in `deploy/README.md`.

- [ ] **Step 1: Write the Quadlet unit**

`deploy/kamaji.container`:

```ini
# Podman Quadlet (rootless). Install to ~/.config/containers/systemd/kamaji.container
# then: systemctl --user daemon-reload && systemctl --user start kamaji
# Replace <HOME> and <PROJECT_ROOT> (repeat Volume= per project + its worktree dir).
[Unit]
Description=kamaji sandbox (daemon + zellij + agents)

[Container]
Image=ghcr.io/alveflo/kamaji:latest
ContainerName=kamaji
PublishPort=127.0.0.1:8755:8755
PublishPort=127.0.0.1:8756:8756
Environment=HOME=<HOME>
Volume=<HOME>/.local/share/kamaji:<HOME>/.local/share/kamaji
Volume=<HOME>/.config/kamaji:<HOME>/.config/kamaji
Volume=kamaji-zellij-cache:<HOME>/.cache/zellij
Volume=<HOME>/.claude:<HOME>/.claude
Volume=<PROJECT_ROOT>:<PROJECT_ROOT>
PodmanArgs=--memory 8g --cpus 4 --pids-limit 2048

[Install]
WantedBy=default.target
```

- [ ] **Step 2: Write the Compose file**

`deploy/docker-compose.yml`:

```yaml
# docker compose -f deploy/docker-compose.yml up -d   (then open http://127.0.0.1:8755)
# docker compose -f deploy/docker-compose.yml down
# Replace ${HOME} resolves from your shell; add one mapping per project root + its worktree dir.
services:
  kamaji:
    image: ghcr.io/alveflo/kamaji:latest
    container_name: kamaji
    # For plain Docker, container-root maps to host-root: prefer rootless Docker
    # or enable the daemon's userns-remap to reduce the gap. (Under rootless
    # Podman this is automatic — container-root maps to your unprivileged user.)
    ports:
      - "127.0.0.1:8755:8755"
      - "127.0.0.1:8756:8756"
    environment:
      HOME: ${HOME}
    deploy:
      resources:
        limits:
          memory: 8g
          cpus: "4"
    pids_limit: 2048
    volumes:
      - ${HOME}/.local/share/kamaji:${HOME}/.local/share/kamaji
      - ${HOME}/.config/kamaji:${HOME}/.config/kamaji
      - kamaji-zellij-cache:${HOME}/.cache/zellij
      - ${HOME}/.claude:${HOME}/.claude
      # - /path/to/project:/path/to/project
      # - /path/to/kamaji-worktrees:/path/to/kamaji-worktrees
volumes:
  kamaji-zellij-cache:
```

- [ ] **Step 3: Write `deploy/README.md`**

```markdown
# Running kamaji in a container by hand

`kamaji up` does all of this for you; these files are the raw equivalents.

**Required, or the board won't work:**
- The daemon must bind `0.0.0.0` (the image's CMD already does).
- Publish ports **8755** and **8756**; 8082 stays internal.
- Set `HOME` to your host home and mount everything at **identical paths**, so
  git worktrees and agent credentials resolve the same inside and out.
- For every registered project, mount **both** its root **and** its worktree
  base dir (default `<root>/../kamaji-worktrees`).
- Mount agent credentials (`~/.claude`, etc.) or pass API keys via env.

Podman (rootless) is recommended; under it, container-root maps to your
unprivileged user. Plain Docker maps container-root to host-root.
```

- [ ] **Step 4: Validate the compose file**

Run: `docker compose -f deploy/docker-compose.yml config >/dev/null && echo ok`
Expected: `ok` (compose syntax is valid).

- [ ] **Step 5: Commit**

```bash
git add deploy/
git commit -m "docs(deploy): Podman Quadlet + Docker Compose escape hatches

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 12: Document container mode in README + ARCHITECTURE

**Files:**
- Modify: `README.md`
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Add a "Container mode" section to `README.md`**

Insert after the `## Install` section (after line ~89):

````markdown
## Container mode (sandboxed agents)

There are two ways to run kamaji: **native** (the default — daemon and agents on
your host, exactly as before) and **sandboxed**, where the daemon, zellij, and
every agent run inside one container so an agent can have **root inside the box**
without risking your host. Container mode is opt-in via `kamaji up`. Rootless
**Podman** is recommended (container-root maps to your unprivileged user); Docker
also works.

```sh
kamaji            # native (default): daemon + agents on the host, as always
kamaji up         # opt in: run daemon + zellij + agents inside the sandbox
# open http://127.0.0.1:8755
kamaji status     # which mode am I in? + board URL
kamaji down       # stop the sandbox (back to native; board + sessions persist)
kamaji logs       # follow container logs
```

`kamaji up` reads your registered projects and bind-mounts each project root
(and its worktree dir) at identical paths, mounts your agent credentials, sets
resource limits, and binds the board to the published port. Add a new project in
the browser/TUI, then re-run `kamaji up` to mount it.

Only the host browser is needed; the TUI's terminal attach is proxied via the
runtime in container mode. v1 targets Linux hosts. Raw `podman`/`docker`
equivalents live in [`deploy/`](deploy/).
````

(Keep the triple-backtick fences balanced when inserting.)

- [ ] **Step 2: Add a "Container mode" note to `ARCHITECTURE.md`**

Insert a short subsection under "## Remote-future seams" (after line ~257):

```markdown
## Container mode

Native (daemon + agents on the host) remains the default and is unchanged. As an
opt-in alternative, a `kamaji up`/`down` launcher runs the whole daemon (board,
proxy, managed `zellij web`, all agent sessions) inside one container — the
sandbox boundary for "agents with root, host protected." It is a
transport/packaging layer over the same daemon: the board binds `0.0.0.0` (via
the image CMD, so the native config bind is untouched) and publishes 8755/8756
(8082 stays internal); project roots and worktree dirs are bind-mounted at
identical paths (so worktree links and XDG paths resolve on both sides); a state
marker in the runtime dir tells the client to connect to the container instead of
spawning a local daemon, and `kamaji status` shows which mode is active. See
`docs/superpowers/specs/2026-06-06-containerize-kamaji-design.md`.
```

- [ ] **Step 3: Verify the docs render (no broken fences)**

Open `README.md` in a markdown preview and confirm the new "Container mode"
section renders: the `kamaji up` block is fenced and the surrounding prose is
not swallowed into a code block.
Expected: the section renders cleanly with one shell code block.

- [ ] **Step 4: Commit**

```bash
git add README.md ARCHITECTURE.md
git commit -m "docs: document container mode (kamaji up/down) in README + ARCHITECTURE

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 13: End-to-end smoke (ignored) + final verification

**Files:**
- Modify: `crates/kamaji/src/container/mod.rs` (add an `#[ignore]` smoke test)

- [ ] **Step 1: Add an ignored end-to-end smoke test**

Append to `crates/kamaji/src/container/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    //! Real end-to-end check: needs a container runtime + the built image
    //! (`docker build -t ghcr.io/alveflo/kamaji:v<version> .`). Ignored in CI.
    #[test]
    #[ignore = "requires a container runtime and the built image; run with --ignored"]
    fn up_then_down_round_trip() {
        super::up(&super::UpArgs { build: true, ..Default::default() }).unwrap();
        assert!(super::state::load().is_some(), "state marker written");
        super::down().unwrap();
        assert!(super::state::load().is_none(), "state marker cleared");
    }
}
```

- [ ] **Step 2: Full workspace test + lint**

Run:
```bash
cargo test 2>&1 | tail -15
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings 2>&1 | tail -15
```
Expected: all tests pass; fmt clean; clippy clean.

- [ ] **Step 3: Manual end-to-end (once, on a machine with podman/docker)**

Run:
```bash
cargo run -p kamaji -- up --build    # builds the correctly-tagged image, then starts it
curl -fsS http://127.0.0.1:8755/healthz
cargo run -p kamaji -- down
```
Expected: build succeeds; `up` prints the board URL; `/healthz` returns `{"ok":true,...}`; `down` reports stopped.

- [ ] **Step 4: Commit**

```bash
git add crates/kamaji/src/container/mod.rs
git commit -m "test(container): ignored up/down end-to-end smoke

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-review (author's pass against the spec)

**Spec coverage:**
- Image with kamajid/zellij/git/agent CLIs → Task 1. ✓
- GHCR publish, version-pinned → Tasks 2, 8 (`image_ref` uses `CARGO_PKG_VERSION`). ✓
- Networking (bind 0.0.0.0, publish 8755/8756, 8082 internal) → Task 1 CMD, Task 6 argv. ✓
- Decision 1 (Podman-first) → Tasks 3, 6 (default rootless mapping), docs. ✓
- Decision 2 (root-in-container) → Task 1 (no `USER` switch). ✓
- Decision 3 (mount creds + env) → Task 8 (`cred_mounts`, `env_passthrough`). ✓
- Decision 4 (resource limits, overridable) → Tasks 6, 8, 9. ✓
- Decision 5 (mount registered roots, identical paths) → Tasks 4, 8; worktree refinement noted. ✓
- worktree-path consistency + `worktree_base` set headless → Tasks 4, 5. ✓
- Persistence (data/config bind, zellij cache volume) → Tasks 6, 8. ✓
- `kamaji up`/`down`/`logs` → Tasks 8, 9. ✓
- Client adapts (connect to container, no local spawn) → Task 10. ✓
- Modes: native default (unchanged) vs container (opt-in), choice visible via `kamaji status` → Tasks 5 (bind left untouched so native keeps loopback), 7/8/9 (status command), 10 (no-op without a marker), 12 (docs). ✓
- TUI attach via `<runtime> exec` → documented (Task 12); not yet code. **Gap:** the exec-attach code path is described in the spec but only documented here, not implemented. Acceptable for v1 (browser-primary); filed as a follow-up note below.
- Re-`up` to mount new project, macOS, remote limits → Task 12 docs. ✓
- Error handling (runtime missing, port in use via health timeout, pull failure) → Task 8. ✓
- Testing strategy (pure argv tests + ignored smoke + CI image build) → Tasks 2–10, 13. ✓
- Escape hatches (Quadlet, Compose) → Task 11. ✓

**Follow-up (out of this plan, file as an issue during execution):** implement TUI Enter/attach as `<runtime> exec -it kamaji zellij attach <name>` when a container is active (Effect path in `main.rs`). v1 ships browser-primary.

**Placeholder scan:** No "TBD"/"add error handling"/"similar to Task N" — every code step contains complete code; infra files contain complete content. The `<HOME>`/`<PROJECT_ROOT>` tokens in `deploy/` are deliberate user-fill placeholders, called out in `deploy/README.md`.

**Type consistency:** `Runtime`, `Mount` (`bind`, `arg`), `derive_project_mounts`, `render_container_config`, `DEFAULT_WORKTREE_BASE`, `RunSpec` (field names match Task 6 ↔ Task 8 construction), `build_run_argv`, `ContainerState` (`name`/`board_addr`/`runtime` match across Tasks 7, 8, 10), `UpArgs` (fields match Tasks 8 ↔ 9), `ContainerResolution`/`resolve_container` (Task 10) are used consistently. `wait_for_health` is reused from `daemon.rs` unchanged.
