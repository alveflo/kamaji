//! Thin wrappers over the `zellij` CLI: listing sessions, distinguishing
//! running from exited/resurrectable ones, and creating/attaching sessions that
//! run a generated layout.

use anyhow::Result;
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicU64, Ordering};

/// Build a `zellij` [`Command`] with the ambient session's environment stripped.
///
/// kamaji orchestrates zellij sessions from the outside. If the daemon (or TUI)
/// was launched *inside* a zellij session, every child process inherits
/// `ZELLIJ`, `ZELLIJ_SESSION_NAME`, and `ZELLIJ_PANE_ID`. With those set, zellij
/// treats CLI invocations as *in-session* commands — most damagingly,
/// `zellij --layout … attach --create-background <name>` silently injects the
/// layout's tab into the ambient session and exits 0 *without* creating the
/// named background session. Stripping the vars makes every invocation behave as
/// a standalone client regardless of where the daemon happens to run.
pub fn command() -> Command {
    let mut c = Command::new("zellij");
    c.env_remove("ZELLIJ")
        .env_remove("ZELLIJ_SESSION_NAME")
        .env_remove("ZELLIJ_PANE_ID");
    c
}

/// Add `-c <config>` to a session-*creating* command so the new session opts
/// into web sharing (zellij sessions default to `web_sharing "off"`, which makes
/// `zellij web` refuse a browser attach with "Web clients are not allowed to
/// attach to this session"). The config mirrors the user's so theme/keybindings
/// survive; see [`crate::zellij_config::web_sharing_config_file`]. A no-op if the
/// config can't be written — the session is still created, just not shareable.
/// Only creation needs this; `list`/`dump`/`attach`/`terminate` must not carry it.
fn with_web_sharing(cmd: &mut Command) {
    if let Some(cfg) = crate::zellij_config::web_sharing_config_file() {
        cmd.arg("-c").arg(cfg);
    }
}

/// True if a session named `name` appears in `zellij list-sessions` output.
/// Compares the first whitespace-delimited token of each line. Note that
/// zellij keeps exited-but-resurrectable sessions in this list, so presence
/// here means "exists or is resurrectable", not "currently running".
pub fn session_in_list(list_output: &str, name: &str) -> bool {
    list_output
        .lines()
        .any(|l| l.split_whitespace().next() == Some(name))
}

/// True if `name` appears in the list AND is marked as exited (zellij keeps
/// exited sessions as "resurrectable"). An exited session's agent is gone, so
/// its detection signal can't be trusted. A session that is absent entirely
/// returns `false` here (handled by reconcile instead).
pub fn session_exited(list_output: &str, name: &str) -> bool {
    list_output
        .lines()
        .any(|l| l.split_whitespace().next() == Some(name) && l.contains("EXITED"))
}

/// Raw `zellij list-sessions` output, or `None` if the command failed (so
/// callers can distinguish "no sessions" from "couldn't ask").
pub fn list_sessions() -> Option<String> {
    match command()
        .args(["list-sessions", "--no-formatting"])
        .output()
    {
        Ok(o) if o.status.success() => Some(String::from_utf8_lossy(&o.stdout).into_owned()),
        _ => None,
    }
}

/// Create AND attach a new session running the given layout. Returns when the
/// user detaches. The layout file is a single-use throwaway: zellij reads it
/// when the session starts, so we delete it once that's done (best-effort).
pub fn create_session(name: &str, layout_path: &Path) -> Result<ExitStatus> {
    let mut cmd = command();
    with_web_sharing(&mut cmd);
    let status = cmd
        .args(["--session", name, "-n"])
        .arg(layout_path)
        .status();
    let _ = std::fs::remove_file(layout_path);
    Ok(status?)
}

/// Create a DETACHED session running `layout_path`, without attaching the
/// caller. The top-level `--layout` makes the layout the session's *initial*
/// tab, and `attach --create-background` creates it detached — so the session
/// has exactly ONE tab (the agent) and a later `attach` lands directly on it.
/// (Doing this in two steps — create then `action new-tab --layout` — leaves a
/// stray empty default tab in front of the agent, which is what we avoid here.)
/// Runs from `cwd` and uses `output()` so zellij's stdout/stderr are captured
/// rather than painted onto the live TUI (same rationale as `dump_screen`).
pub fn create_session_background(name: &str, layout_path: &Path, cwd: &Path) -> Result<()> {
    let mut cmd = command();
    cmd.current_dir(cwd);
    with_web_sharing(&mut cmd);
    let out = cmd
        .arg("--layout")
        .arg(layout_path)
        .args(["attach", "--create-background", name])
        .output();
    // The layout file is a single-use throwaway: by the time `output()` returns
    // zellij has read it into the session's initial tab, so it's safe to delete
    // whether the spawn succeeded or not (best-effort). Without this, every
    // session start/resume leaks one file into temp_dir()/kamaji-layouts.
    let _ = std::fs::remove_file(layout_path);
    let out = out?;
    if !out.status.success() {
        anyhow::bail!(
            "zellij --layout … attach --create-background failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // A 0 exit code is not proof the session exists. `command()` scrubs the
    // ambient ZELLIJ env that would otherwise make zellij no-op this into a
    // tab-injection, but verify the session actually materialized so any future
    // silent-success mode fails loudly here instead of surfacing later as a
    // reconcile-cleared session and a 400 on attach. (`list_sessions` reflects a
    // new background session synchronously once this command returns.) If we
    // can't reach zellij to check, assume success — same as `is_session_live`.
    if !list_sessions()
        .map(|l| session_in_list(&l, name))
        .unwrap_or(true)
    {
        anyhow::bail!(
            "zellij reported success but session {name:?} is absent from list-sessions \
             (is the daemon running inside a zellij session?)"
        );
    }
    Ok(())
}

/// Attach to an existing session. Returns when the user detaches.
pub fn attach_session(name: &str) -> Result<ExitStatus> {
    Ok(command().args(["attach", name]).status()?)
}

/// Capture the focused pane of a (possibly background) session. Returns `None`
/// if zellij isn't reachable or the dump fails, so callers treat it as "no
/// information". `dump-screen` writes to a file, which we read then delete.
/// The temp filename is made unique (pid + counter) so that the TUI and daemon
/// polling the same session concurrently never read/delete each other's dump.
pub fn dump_screen(session: &str) -> Option<String> {
    static DUMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = DUMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!(
        "kamaji-dump-{session}-{}-{counter}.txt",
        std::process::id()
    ));
    // Use output() (not status()) so zellij's stdout/stderr are captured rather
    // than inherited — otherwise its noise would paint onto the live TUI.
    let output = command()
        .args(["--session", session, "action", "dump-screen"])
        .arg(&tmp)
        .output()
        .ok()?;
    let result = if output.status.success() {
        std::fs::read_to_string(&tmp).ok()
    } else {
        None
    };
    let _ = std::fs::remove_file(&tmp);
    result
}

/// Kill and delete a session; errors are ignored (it may already be gone).
pub fn terminate_session(name: &str) {
    let _ = command().args(["kill-session", name]).output();
    let _ = command().args(["delete-session", "--force", name]).output();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_session_by_first_token() {
        let out = "kamaji-1-foo [Created 2h ago]\nother-session (current)\n";
        assert!(session_in_list(out, "kamaji-1-foo"));
        assert!(session_in_list(out, "other-session"));
        assert!(!session_in_list(out, "kamaji-2-bar"));
    }

    #[test]
    fn command_strips_ambient_zellij_env() {
        // kamaji orchestrates zellij from the outside. If the daemon/TUI was
        // launched inside a zellij session it inherits these vars, which make a
        // child `zellij` behave as an in-session command (e.g. create-background
        // silently injects a tab and exits 0). The builder must scrub them.
        let cmd = command();
        assert_eq!(cmd.get_program(), "zellij");
        let removed: Vec<String> = cmd
            .get_envs()
            .filter(|(_, v)| v.is_none())
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        for var in ["ZELLIJ", "ZELLIJ_SESSION_NAME", "ZELLIJ_PANE_ID"] {
            assert!(removed.contains(&var.to_string()), "{var} not scrubbed");
        }
    }

    #[test]
    fn session_exited_detects_resurrectable_sessions() {
        let out = "kamaji-1-foo [Created 2h ago]\n\
                   kamaji-2-bar [Created 1h ago] (EXITED - attach to resurrect)\n";
        // Live session: not exited.
        assert!(!session_exited(out, "kamaji-1-foo"));
        // Resurrectable session: exited.
        assert!(session_exited(out, "kamaji-2-bar"));
        // Absent session: not reported as exited (reconcile handles removal).
        assert!(!session_exited(out, "kamaji-3-baz"));
    }
}
