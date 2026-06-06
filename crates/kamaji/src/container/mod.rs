//! Container-mode orchestration: start (`up`), stop (`down`), and follow logs
//! for the single sandbox container that holds the daemon + zellij + all agents.
//! Pure planning lives in [`plan`]; the host-side marker in [`state`].

pub mod plan;
pub mod state;

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use kamaji_core::config;
use kamaji_core::db::Db;
use kamaji_core::paths;

use self::plan::{
    build_run_argv, derive_project_mounts, render_container_config, Mount, RunSpec, Runtime,
};
use self::state::ContainerState;

const CONTAINER_NAME: &str = "kamaji";
const ZELLIJ_VOLUME: &str = "kamaji-zellij-cache";
const BOARD_ADDR: &str = "127.0.0.1:8755";

/// Options parsed from `kamaji up`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GITHUB_TOKEN",
        "GH_TOKEN",
    ]
    .iter()
    .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
    .collect()
}

/// Read the registered projects from the shared DB (empty if it doesn't exist
/// yet — the very first `up` runs with the board only).
fn registered_projects() -> Result<Vec<kamaji_core::models::Project>> {
    let db_path = paths::data_dir()
        .context("cannot determine data dir")?
        .join("kamaji.db");
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    Db::open(&db_path)?.list_projects()
}

/// Start (or restart) the sandbox container.
pub fn up(args: &UpArgs) -> Result<()> {
    let runtime = plan::detect_runtime(binary_exists, args.runtime).ok_or_else(|| {
        anyhow!("no container runtime found — install podman (recommended) or docker")
    })?;
    let bin = runtime.binary();
    let image = image_ref();

    // 1. Ensure the image is available.
    if args.build {
        run_checked(bin, &["build", "-t", &image, "."], "building image")?;
    } else if !run_ok(bin, &["image", "inspect", &image]) && !run_ok(bin, &["pull", &image]) {
        bail!("could not pull {image}; re-run with --build to build it locally");
    }

    // 2. Ensure worktree_base in the shared config (bind comes from the image CMD).
    let base = config::load_or_init()?;
    let cfg = render_container_config(&base);
    let cfg_path = config::config_path()?;
    config::save_to(&cfg_path, &cfg).with_context(|| format!("writing {}", cfg_path.display()))?;

    // 3. Pre-create worktree-base dirs on the host so their bind mounts exist.
    let projects = registered_projects()?;
    let wt_template = cfg
        .worktree_base
        .as_deref()
        .unwrap_or(plan::DEFAULT_WORKTREE_BASE);
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
    run_checked(
        bin,
        &argv.iter().map(String::as_str).collect::<Vec<_>>(),
        "starting container",
    )?;

    // 5. Wait for health on the published port, then record state.
    crate::daemon::wait_for_health(&format!("http://{BOARD_ADDR}"), Duration::from_secs(30))
        .map_err(|e| {
            anyhow!(
                "container started but board never became healthy: {e}\n\
                 Check `kamaji logs`, then run `kamaji down` before retrying."
            )
        })?;
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
        plan::detect_runtime(binary_exists, None)
            .map(|r| r.binary().to_string())
            .unwrap_or_default()
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
    let status = Command::new(&st.runtime)
        .args(["logs", "-f", &st.name])
        .status()?;
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
    Command::new(bin)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_checked(bin: &str, args: &[&str], what: &str) -> Result<()> {
    let status = Command::new(bin)
        .args(args)
        .status()
        .with_context(|| format!("{what}: spawning {bin}"))?;
    if !status.success() {
        bail!("{what}: {bin} exited with {status}");
    }
    Ok(())
}
