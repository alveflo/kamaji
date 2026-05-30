use anyhow::{bail, Result};
use std::path::Path;
use std::process::Command;

fn git(root: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(root);
    c
}

pub fn is_git_repo(root: &Path) -> bool {
    git(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn default_branch(root: &Path) -> Result<String> {
    let out = git(root)
        .args([
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ])
        .output()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        return Ok(s.strip_prefix("origin/").unwrap_or(&s).to_string());
    }
    let out = git(root)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    bail!("could not determine base branch for {}", root.display())
}

pub fn add_worktree(root: &Path, worktree: &Path, branch: &str, base: &str) -> Result<()> {
    let out = git(root)
        .args(["worktree", "add"])
        .arg(worktree)
        .args(["-b", branch, base])
        .output()?;
    if !out.status.success() {
        bail!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

pub fn remove_worktree(root: &Path, worktree: &Path) -> Result<()> {
    let out = git(root)
        .args(["worktree", "remove", "--force"])
        .arg(worktree)
        .output()?;
    if !out.status.success() {
        bail!(
            "git worktree remove failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

pub fn delete_branch(root: &Path, branch: &str) -> Result<()> {
    git(root).args(["branch", "-D", branch]).output()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {:?} failed", args);
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.email", "t@t.t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("README.md"), "hi").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "init"]);
        dir
    }

    #[test]
    fn detects_repo_and_non_repo() {
        let repo = init_repo();
        assert!(is_git_repo(repo.path()));
        let plain = tempfile::tempdir().unwrap();
        assert!(!is_git_repo(plain.path()));
    }

    #[test]
    fn default_branch_is_main() {
        let repo = init_repo();
        assert_eq!(default_branch(repo.path()).unwrap(), "main");
    }

    #[test]
    fn worktree_add_then_remove() {
        let repo = init_repo();
        let wt = repo.path().join("..").join("kamaji-wt-test");
        let _ = remove_worktree(repo.path(), &wt); // ignore if absent
        add_worktree(repo.path(), &wt, "feature-x", "main").unwrap();
        assert!(wt.join("README.md").exists());
        remove_worktree(repo.path(), &wt).unwrap();
        delete_branch(repo.path(), "feature-x").unwrap();
    }
}
