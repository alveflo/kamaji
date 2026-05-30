//! kamajid — the kamaji daemon. Parses minimal CLI args, initializes logging,
//! opens the shared SQLite DB, and serves the HTTP API on the configured bind
//! address.

use std::path::PathBuf;

use anyhow::{Context, Result};
use kamaji_core::config::{self, Config};
use kamaji_core::db::Db;
use kamaji_core::paths;
use tracing_subscriber::EnvFilter;

use kamajid::state::AppState;

fn db_path() -> Result<PathBuf> {
    Ok(paths::data_dir()
        .context("cannot determine data dir")?
        .join("kamaji.db"))
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

fn init_tracing(config: &Config) {
    let filter = EnvFilter::try_from_env("KAMAJID_LOG")
        .or_else(|_| EnvFilter::try_new(&config.daemon.log_level))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if config.daemon.log_format == "json" {
        builder.json().init();
    } else {
        builder.init();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = config::load_or_init()?;
    let args = parse_args(&config)?;
    init_tracing(&config);

    let bind = args.bind.unwrap_or_else(|| config.daemon.bind.clone());
    let db = Db::open(&db_path()?)?;
    let state = AppState::new(db, config);

    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    tracing::info!(%bind, "kamajid listening");
    kamajid::serve(listener, state).await
}
