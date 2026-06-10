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
        out.push_str(&format!(
            "  {} {} — {}\n",
            marker(c.verdict),
            c.name,
            c.detail
        ));
        if let Some(hint) = &c.hint {
            out.push_str(&format!("         ↳ {hint}\n"));
        }
    }
}

/// Count of non-Ok checks across local + daemon-local sections.
fn problem_count(report: &DoctorReport) -> usize {
    let local = report
        .local
        .checks
        .iter()
        .filter(|c| c.verdict != Verdict::Ok)
        .count();
    let daemon = report
        .daemon
        .as_ref()
        .map(|d| {
            d.local
                .checks
                .iter()
                .filter(|c| c.verdict != Verdict::Ok)
                .count()
        })
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
                marker(if d.zellij_web_reachable {
                    Verdict::Ok
                } else {
                    Verdict::Warn
                }),
                if d.zellij_web_reachable {
                    "reachable"
                } else {
                    "not reachable (no browser attach yet, or zellij web down)"
                }
            ));
            out.push_str(&format!(
                "  {} reverse proxy — {}\n",
                marker(if d.proxy_reachable {
                    Verdict::Ok
                } else {
                    Verdict::Fail
                }),
                if d.proxy_reachable {
                    "listening"
                } else {
                    "not listening (iframe will show a crashed page)"
                }
            ));
            out.push_str(&format!(
                "  ·      projects {}, tickets {}\n",
                d.project_count, d.ticket_count
            ));
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
        if problems == 0 {
            "no problems found".to_string()
        } else {
            format!("{problems} problem(s) found")
        }
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
            env: vec![EnvVar {
                key: "PATH".into(),
                value: "/usr/bin".into(),
            }],
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
            local: local(true),
            daemon: Some(daemon_report(false, true)),
            daemon_error: None,
            recent_logs: vec!["line one".into()],
        };
        let text = render(&report);
        assert!(text.contains("MISMATCH: zellij is on YOUR PATH"), "{text}");
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
