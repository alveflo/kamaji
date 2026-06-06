//! Daemon-side seam over zellij *session lifecycle* (liveness, terminate,
//! background-create). The resume-on-attach path needs to ask "is this session
//! still live?" and, if not, recreate it — operations that shell out to the
//! `zellij` binary. Hiding them behind a trait lets tests inject a recording
//! fake (mirroring [`crate::zellij_web::ZellijWeb::fake`]), so the resurrect
//! logic is exercised in CI with no real zellij present.
//!
//! This is purely a daemon concern: `kamaji-core::zellij` owns the real
//! subprocess calls; [`RealSessionDriver`] is a thin adapter over them.

use std::path::Path;
use std::sync::Mutex;

/// The session-lifecycle operations the attach route needs.
pub trait SessionDriver: Send + Sync {
    /// True if `name` is a *live* (running, non-exited) zellij session. When
    /// zellij can't be queried at all, returns `true` so attach falls back to a
    /// plain attach rather than spuriously recreating a session it can't see.
    fn is_session_live(&self, name: &str) -> bool;

    /// Kill + delete a (possibly exited / "resurrectable") session so its name
    /// is free to recreate. Best-effort — a name that's already gone is fine.
    fn terminate(&self, name: &str);

    /// Create a detached session named `name` running `layout_path` from `cwd`.
    fn create_background(&self, name: &str, layout_path: &Path, cwd: &Path) -> anyhow::Result<()>;
}

/// The production driver: delegates to `kamaji_core::zellij`.
pub struct RealSessionDriver;

impl SessionDriver for RealSessionDriver {
    fn is_session_live(&self, name: &str) -> bool {
        match kamaji_core::zellij::list_sessions() {
            // Present AND not exited == live. Absent or exited == resurrect.
            Some(list) => {
                kamaji_core::zellij::session_in_list(&list, name)
                    && !kamaji_core::zellij::session_exited(&list, name)
            }
            // Couldn't ask zellij: assume live so we never spuriously recreate.
            None => true,
        }
    }

    fn terminate(&self, name: &str) {
        kamaji_core::zellij::terminate_session(name);
    }

    fn create_background(&self, name: &str, layout_path: &Path, cwd: &Path) -> anyhow::Result<()> {
        kamaji_core::zellij::create_session_background(name, layout_path, cwd)
    }
}

/// One recorded `create_background` call, capturing the rendered layout's
/// contents so a test can assert the resume argv made it into the session.
#[derive(Debug, Clone)]
pub struct CreatedSession {
    pub name: String,
    pub layout: String,
    pub cwd: std::path::PathBuf,
}

/// A recording test double. `is_session_live` returns the fixed `live` value;
/// `terminate`/`create_background` record their calls for later assertions.
pub struct FakeSessionDriver {
    live: bool,
    created: Mutex<Vec<CreatedSession>>,
    terminated: Mutex<Vec<String>>,
}

impl FakeSessionDriver {
    /// `live` is what every `is_session_live` query returns: `true` to model an
    /// already-running session (plain attach), `false` to model a dead one that
    /// must be resurrected.
    pub fn new(live: bool) -> Self {
        FakeSessionDriver {
            live,
            created: Mutex::new(Vec::new()),
            terminated: Mutex::new(Vec::new()),
        }
    }

    /// The recorded `create_background` calls, in order.
    pub fn created(&self) -> Vec<CreatedSession> {
        self.created.lock().expect("created mutex poisoned").clone()
    }

    /// The names passed to `terminate`, in order.
    pub fn terminated(&self) -> Vec<String> {
        self.terminated
            .lock()
            .expect("terminated mutex poisoned")
            .clone()
    }
}

impl SessionDriver for FakeSessionDriver {
    fn is_session_live(&self, _name: &str) -> bool {
        self.live
    }

    fn terminate(&self, name: &str) {
        self.terminated
            .lock()
            .expect("terminated mutex poisoned")
            .push(name.to_string());
    }

    fn create_background(&self, name: &str, layout_path: &Path, cwd: &Path) -> anyhow::Result<()> {
        // Read the rendered layout so assertions can inspect the launched argv,
        // then delete it — mirroring the real driver, which relies on zellij
        // having consumed the single-use layout file by the time it returns.
        let layout = std::fs::read_to_string(layout_path)?;
        let _ = std::fs::remove_file(layout_path);
        self.created
            .lock()
            .expect("created mutex poisoned")
            .push(CreatedSession {
                name: name.to_string(),
                layout,
                cwd: cwd.to_path_buf(),
            });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_records_create_and_terminate() {
        let dir = tempfile::tempdir().unwrap();
        let layout = dir.path().join("l.kdl");
        std::fs::write(&layout, "pane command=\"codex\"").unwrap();

        let d = FakeSessionDriver::new(false);
        assert!(!d.is_session_live("any"));
        d.terminate("kamaji-1-x");
        d.create_background("kamaji-1-x", &layout, dir.path())
            .unwrap();

        assert_eq!(d.terminated(), vec!["kamaji-1-x".to_string()]);
        let created = d.created();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].name, "kamaji-1-x");
        assert!(created[0].layout.contains("codex"));
        assert_eq!(created[0].cwd, dir.path());
        // The single-use layout file is deleted once consumed, so it never
        // accumulates in temp_dir()/kamaji-layouts across session starts.
        assert!(!layout.exists());
    }

    #[test]
    fn fake_live_reports_live() {
        assert!(FakeSessionDriver::new(true).is_session_live("x"));
    }
}
