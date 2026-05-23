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

/// Attach to an existing session. Returns when the user detaches.
pub fn attach_session(name: &str) -> Result<ExitStatus> {
    Ok(Command::new("zellij").args(["attach", name]).status()?)
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
}
