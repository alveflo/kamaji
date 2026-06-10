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
            // 8082 is ZellijWeb's fixed base port (zellij_web.rs DEFAULT_BASE_URL).
            // TODO: derive from state.zellij_web() once it exposes a base_url() accessor.
            let web = diagnostics::tcp_reachable("127.0.0.1", 8082, PROBE_TIMEOUT);
            let proxy = probe_proxy(&proxy_for_probe);
            (local, sessions, web, proxy)
        })
        .await
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "diagnostics gather task panicked");
            (
                diagnostics::LocalReport {
                    checks: vec![],
                    env: vec![],
                },
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
        assert!(report
            .local
            .checks
            .iter()
            .any(|c| c.name.contains("zellij")));
        assert_eq!(report.project_count, 0);
        assert_eq!(report.ticket_count, 0);
    }

    #[tokio::test]
    async fn probe_proxy_parses_host_and_port() {
        assert!(!probe_proxy("http://127.0.0.1:1"));
        assert!(!probe_proxy("garbage"));
    }

    #[tokio::test]
    async fn diagnostics_counts_seeded_projects_and_tickets() {
        use kamaji_core::models::Agent;
        use std::path::PathBuf;

        let db = Db::open_in_memory().unwrap();
        let project = db
            .create_project("test-proj", &PathBuf::from("/tmp/test-proj"), None)
            .unwrap();
        db.create_ticket(project.id, "ticket-one", "", None, Agent::Claude)
            .unwrap();
        let mut state = AppState::new(db, Config::default());
        state.set_zellij_web(crate::zellij_web::ZellijWeb::fake("t"));
        let Json(report) = diagnostics(State(state)).await;
        assert_eq!(report.project_count, 1);
        assert_eq!(report.ticket_count, 1);
    }
}
