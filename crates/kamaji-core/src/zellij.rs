use anyhow::Result;
use std::path::Path;
use std::process::{Command, ExitStatus};

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
    match Command::new("zellij")
        .args(["list-sessions", "--no-formatting"])
        .output()
    {
        Ok(o) if o.status.success() => Some(String::from_utf8_lossy(&o.stdout).into_owned()),
        _ => None,
    }
}

/// Create AND attach a new session running the given layout. Returns when the
/// user detaches.
pub fn create_session(name: &str, layout_path: &Path) -> Result<ExitStatus> {
    Ok(Command::new("zellij")
        .args(["--session", name, "-n"])
        .arg(layout_path)
        .status()?)
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
    let out = Command::new("zellij")
        .current_dir(cwd)
        .arg("--layout")
        .arg(layout_path)
        .args(["attach", "--create-background", name])
        .output()?;
    if !out.status.success() {
        anyhow::bail!(
            "zellij --layout … attach --create-background failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// Attach to an existing session. Returns when the user detaches.
pub fn attach_session(name: &str) -> Result<ExitStatus> {
    Ok(Command::new("zellij").args(["attach", name]).status()?)
}

/// Capture the focused pane of a (possibly background) session. Returns `None`
/// if zellij isn't reachable or the dump fails, so callers treat it as "no
/// information". `dump-screen` writes to a file, which we read then delete.
pub fn dump_screen(session: &str) -> Option<String> {
    let tmp = std::env::temp_dir().join(format!("kamaji-dump-{session}.txt"));
    // Use output() (not status()) so zellij's stdout/stderr are captured rather
    // than inherited — otherwise its noise would paint onto the live TUI.
    let output = Command::new("zellij")
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
    let _ = Command::new("zellij").args(["kill-session", name]).output();
    let _ = Command::new("zellij")
        .args(["delete-session", "--force", name])
        .output();
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
