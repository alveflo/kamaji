//! Daemon auto-spawn: ensure a healthy kamajid (pidfile lock + health probe),
//! spawning one detached if absent; race-safe via atomic pidfile create.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kamaji_core::config::Config;
use kamaji_core::paths;

use crate::client::DaemonClient;

/// Paths to the pidfile + addrfile under the runtime dir.
pub fn runtime_files() -> Option<(PathBuf, PathBuf)> {
    let dir = paths::runtime_dir()?;
    Some((dir.join("kamajid.pid"), dir.join("kamajid.addr")))
}

/// True if `pid` names a live process. Unix: `kill(pid, 0)` semantics via
/// checking `/proc` is avoided; we use a 0-signal. Windows: best-effort true
/// (we rely on the health probe to catch a dead daemon).
#[cfg(unix)]
pub fn pid_alive(pid: i32) -> bool {
    // signal 0 only checks existence/permission, never delivers a signal.
    unsafe { libc::kill(pid, 0) == 0 }
}
#[cfg(not(unix))]
pub fn pid_alive(_pid: i32) -> bool {
    true
}

/// Parse the PID written in the pidfile, if any.
pub fn read_pid(pidfile: &Path) -> Option<i32> {
    std::fs::read_to_string(pidfile).ok()?.trim().parse().ok()
}

/// If a live daemon is described by the pidfile+addrfile, connect and return it.
/// "Live" = the named PID exists AND `/healthz` answers. On any failure the
/// stale files are removed and `None` is returned so the caller lock-acquires.
pub fn probe_existing(pidfile: &Path, addrfile: &Path) -> Option<DaemonClient> {
    let pid = read_pid(pidfile)?;
    let addr = std::fs::read_to_string(addrfile).ok()?.trim().to_string();
    if pid_alive(pid) {
        if let Ok(client) = DaemonClient::connect(format!("http://{addr}")) {
            return Some(client);
        }
    }
    let _ = std::fs::remove_file(pidfile);
    let _ = std::fs::remove_file(addrfile);
    None
}

/// Atomically create the pidfile as a lock (O_CREAT|O_EXCL). Exactly one racer
/// wins; losers get an `AlreadyExists` error.
pub fn acquire_lock(pidfile: &Path) -> std::io::Result<()> {
    if let Some(parent) = pidfile.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(pidfile)?;
    // Placeholder; the daemon overwrites this with its real PID on bind.
    write!(f, "{}", std::process::id())
}

/// Poll `<base>/healthz` every ~50ms until 200 or the deadline. Bounded.
pub fn wait_for_health(base: &str, timeout: Duration) -> std::result::Result<DaemonClient, String> {
    let deadline = Instant::now() + timeout;
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(200))
        .build()
        .map_err(|e| e.to_string())?;
    while Instant::now() < deadline {
        if http
            .get(format!("{base}/healthz"))
            .send()
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return DaemonClient::connect(base.to_string()).map_err(|e| format!("{e:?}"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(format!(
        "daemon did not become healthy at {base} within {timeout:?}"
    ))
}

/// Locate the kamajid binary: a sibling next to the running kamaji, else PATH.
fn kamajid_path() -> std::result::Result<PathBuf, String> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(if cfg!(windows) {
                "kamajid.exe"
            } else {
                "kamajid"
            });
            if sibling.exists() {
                return Ok(sibling);
            }
        }
    }
    Ok(PathBuf::from("kamajid")) // fall back to PATH resolution
}

/// Spawn `kamajid serve --bind <addr>` detached so it outlives the TUI.
#[cfg(unix)]
fn spawn_detached(bin: &Path, addr: &str) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(bin);
    cmd.args(["serve", "--bind", addr])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // New session so it isn't killed when the terminal closes.
    // SAFETY: the pre_exec closure runs in the forked child before exec and
    // only calls `setsid`, which is async-signal-safe and allocates nothing.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn()?;
    Ok(())
}
#[cfg(not(unix))]
fn spawn_detached(bin: &Path, addr: &str) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    Command::new(bin)
        .args(["serve", "--bind", addr])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()?;
    Ok(())
}

/// Ensure a healthy daemon and return a connected client. Tries an existing
/// daemon; else lock-acquires (winner spawns + health-waits + writes addr;
/// loser health-waits on the expected addr). Bounded retry on a lost race whose
/// winner crashed. `forced_addr` (from `--daemon`) skips spawning entirely.
pub fn ensure_daemon(
    config: &Config,
    forced_addr: Option<&str>,
    allow_spawn: bool,
) -> std::result::Result<DaemonClient, String> {
    if let Some(addr) = forced_addr {
        let base = if addr.starts_with("http") {
            addr.to_string()
        } else {
            format!("http://{addr}")
        };
        return DaemonClient::connect(base).map_err(|e| format!("--daemon {addr}: {e:?}"));
    }
    let (pidfile, addrfile) = runtime_files().ok_or("cannot determine runtime dir")?;
    let bind = config.daemon.bind.clone();
    let base = format!("http://{bind}");
    for _attempt in 0..2 {
        if let Some(client) = probe_existing(&pidfile, &addrfile) {
            return Ok(client);
        }
        match acquire_lock(&pidfile) {
            Ok(()) => {
                if !allow_spawn {
                    let _ = std::fs::remove_file(&pidfile);
                    return Err("no daemon running and --no-spawn was given".into());
                }
                let bin = kamajid_path()?;
                spawn_detached(&bin, &bind)
                    .map_err(|e| format!("spawning kamajid ({}): {e}", bin.display()))?;
                // The daemon writes its own pid/addr on bind; we just wait for health.
                return wait_for_health(&base, Duration::from_secs(5));
            }
            Err(_already_exists) => {
                // Someone else is starting it: wait for the winner's health.
                if let Ok(client) = wait_for_health(&base, Duration::from_secs(5)) {
                    return Ok(client);
                }
                // Winner may have crashed between lock and bind: clear + retry once.
                let _ = std::fs::remove_file(&pidfile);
                let _ = std::fs::remove_file(&addrfile);
            }
        }
    }
    Err(format!("could not reach or start a daemon at {bind}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_pid_parses_written_value() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("kamajid.pid");
        std::fs::write(&f, "4321\n").unwrap();
        assert_eq!(read_pid(&f), Some(4321));
    }

    #[test]
    fn read_pid_none_when_absent_or_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("kamajid.pid");
        assert_eq!(read_pid(&f), None);
        std::fs::write(&f, "not-a-pid").unwrap();
        assert_eq!(read_pid(&f), None);
    }

    #[cfg(unix)]
    #[test]
    fn pid_alive_true_for_self_false_for_unused() {
        assert!(pid_alive(std::process::id() as i32));
        // PID 0x7fffffff is astronomically unlikely to be live.
        assert!(!pid_alive(0x7fff_ffff));
    }

    #[test]
    fn stale_pidfile_is_reclaimed() {
        // A pidfile naming a dead PID + no live daemon => probe_existing returns
        // None and the stale files are removed.
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("kamajid.pid");
        let addrfile = dir.path().join("kamajid.addr");
        std::fs::write(&pidfile, "2147483647").unwrap(); // dead PID
        std::fs::write(&addrfile, "127.0.0.1:8755").unwrap();
        let got = probe_existing(&pidfile, &addrfile);
        assert!(got.is_none(), "a stale pidfile must not yield a client");
        assert!(!pidfile.exists(), "stale pidfile is removed");
        assert!(!addrfile.exists(), "stale addrfile is removed");
    }

    #[test]
    fn acquire_lock_is_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("kamajid.pid");
        assert!(acquire_lock(&pidfile).is_ok(), "first writer wins the lock");
        assert!(
            acquire_lock(&pidfile).is_err(),
            "second writer loses (AlreadyExists)"
        );
    }

    #[test]
    fn health_wait_times_out_on_dead_port() {
        // Nothing listens on this port; bounded wait returns an error, not a hang.
        let started = std::time::Instant::now();
        let res = wait_for_health("http://127.0.0.1:1", std::time::Duration::from_millis(300));
        assert!(res.is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    /// End-to-end: actually spawns the built `kamajid` detached and connects.
    /// Gated behind `--ignored` because it forks a real daemon and binds a port.
    #[cfg(unix)]
    #[test]
    #[ignore = "actually spawns the built kamajid detached; run with --ignored"]
    fn ensure_daemon_spawns_and_connects() {
        use kamaji_core::config::Config;

        let dir = tempfile::tempdir().unwrap();
        // Isolate every runtime/data/config base into the tempdir so the spawned
        // daemon and this test agree on the pidfile/addrfile location and the
        // daemon's own state lives nowhere durable.
        std::env::set_var("XDG_RUNTIME_DIR", dir.path());
        std::env::set_var("XDG_DATA_HOME", dir.path());
        std::env::set_var("XDG_CONFIG_HOME", dir.path());

        let cfg = Config::default();
        let client = ensure_daemon(&cfg, None, true)
            .expect("ensure_daemon should spawn and connect to a healthy daemon");
        // The returned client already pinged /healthz on connect; sanity-check it
        // again directly to prove the daemon is green.
        let base = format!("http://{}", cfg.daemon.bind);
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .unwrap();
        let healthy = http
            .get(format!("{base}/healthz"))
            .send()
            .map(|r| r.status().is_success())
            .unwrap_or(false);
        assert!(healthy, "/healthz must be green after ensure_daemon");
        drop(client);

        // Tear down the daemon we spawned, via the PID it wrote to the pidfile.
        let (pidfile, _addrfile) = runtime_files().expect("runtime dir under XDG_RUNTIME_DIR");
        if let Some(pid) = read_pid(&pidfile) {
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
    }
}
