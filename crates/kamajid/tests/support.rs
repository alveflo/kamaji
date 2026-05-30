//! Shared test support: build a committed temp git repo so the daemon's
//! worktree-creating routes have a real base branch to branch from.

#![allow(dead_code)]

use std::process::Command;

/// Create a committed git repo at a fresh temp dir and return it. The repo has
/// one commit on `main` so `git worktree add -b … main` works.
pub fn committed_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let run = |args: &[&str]| {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap()
                .status
                .success(),
            "git {args:?} failed"
        );
    };
    run(&["init", "-b", "main"]);
    run(&["config", "user.email", "t@t.t"]);
    run(&["config", "user.name", "t"]);
    std::fs::write(root.join("README.md"), "hi").unwrap();
    run(&["add", "."]);
    run(&["commit", "-m", "init"]);
    dir
}

/// True if a `zellij` binary is on PATH (the live session-spawn path needs it).
pub fn zellij_available() -> bool {
    Command::new("zellij")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
