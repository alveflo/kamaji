//! kamajid — the kamaji daemon. Parses minimal CLI args, initializes logging,
//! opens the shared SQLite DB, and serves the HTTP API on the configured bind
//! address.

use std::path::PathBuf;

use anyhow::{Context, Result};
use kamaji_core::config::{self, Config};
use kamaji_core::db::Db;
use kamaji_core::paths;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;

use kamajid::state::AppState;

fn db_path() -> Result<PathBuf> {
    Ok(paths::data_dir()
        .context("cannot determine data dir")?
        .join("kamaji.db"))
}

fn runtime_paths() -> Result<(PathBuf, PathBuf)> {
    let dir = paths::runtime_dir().context("cannot determine runtime dir")?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok((dir.join("kamajid.pid"), dir.join("kamajid.addr")))
}

/// Minimal arg parse: `kamajid serve [--bind ADDR]`, plus `--help`/`--version`.
/// Other daemon settings come from the `[daemon]` config section.
struct Args {
    bind: Option<String>,
}

fn parse_args(config: &Config) -> Result<Args> {
    let mut bind = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "serve" => {}
            "--bind" => {
                bind = Some(it.next().context("--bind needs an address")?);
            }
            "--help" | "-h" => {
                println!(
                    "usage: kamajid serve [--bind ADDR]\n  default bind: {}",
                    config.daemon.bind
                );
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("kamajid {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    Ok(Args { bind })
}

/// Derive the reverse-proxy `(bind_addr, public_base_url)` from the board bind.
/// The proxy listens on the board port + 1. The public base URL the browser
/// loads the iframe from uses `127.0.0.1` when the board binds a wildcard host.
fn derive_proxy_addr(bind: &str) -> Option<(String, String)> {
    let (host, port) = bind.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let proxy_port = port.checked_add(1)?;
    let proxy_bind = format!("{host}:{proxy_port}");
    let public_host = if host == "0.0.0.0" || host == "::" || host.is_empty() {
        "127.0.0.1"
    } else {
        host
    };
    let public_base = format!("http://{public_host}:{proxy_port}");
    Some((proxy_bind, public_base))
}

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

#[tokio::main]
async fn main() -> Result<()> {
    let config = config::load_or_init()?;
    let args = parse_args(&config)?;
    // Hold the file-log writer guard for the whole process so buffered log
    // lines are flushed; dropping it would stop the background writer.
    let _log_guard = init_tracing(&config);

    let bind = args.bind.unwrap_or_else(|| config.daemon.bind.clone());
    let db = Db::open(&db_path()?)?;
    let mut state = AppState::new(db, config);

    // Reverse proxy for `zellij web` (board port + 1): lets the browser embed a
    // session in a same-origin, pre-authenticated iframe. Best-effort — if the
    // port can't be bound, the board still serves; only the inline terminal
    // panel is unavailable.
    let proxy = derive_proxy_addr(&bind);
    if let Some((_, base)) = &proxy {
        state.set_proxy_base(base.clone());
    }

    let poll_interval = state.config_async().await.poll_interval();
    kamajid::poll_task::spawn_poll_task(state.clone(), poll_interval);

    if let Some((proxy_bind, _)) = &proxy {
        match tokio::net::TcpListener::bind(proxy_bind).await {
            Ok(pl) => {
                tracing::info!(bind = %proxy_bind, "zellij proxy listening");
                let st = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = kamajid::serve_proxy(pl, st).await {
                        tracing::error!(error = %e, "zellij proxy stopped");
                    }
                });
            }
            Err(e) => tracing::warn!(bind = %proxy_bind, error = %e, "zellij proxy not started"),
        }
    }

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    tracing::info!(%bind, "kamajid listening");

    let local = listener
        .local_addr()
        .with_context(|| "reading bound address")?;
    state.set_bound_addr(local);
    let (pidfile, addrfile) = runtime_paths()?;
    // Startup clients treat the addrfile as the point where health can be
    // probed, so replace the non-PID lock placeholder before publishing it.
    std::fs::write(&pidfile, std::process::id().to_string())
        .with_context(|| format!("writing {}", pidfile.display()))?;
    std::fs::write(&addrfile, local.to_string())
        .with_context(|| format!("writing {}", addrfile.display()))?;
    tracing::info!(%local, pid = std::process::id(), "wrote pid/addr files");

    let cleanup = (pidfile.clone(), addrfile.clone());
    let result = kamajid::serve(listener, state).await;
    let _ = std::fs::remove_file(&cleanup.0);
    let _ = std::fs::remove_file(&cleanup.1);
    result
}
