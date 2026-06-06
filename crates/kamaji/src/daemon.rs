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

fn read_addr(addrfile: &Path) -> Option<String> {
    let addr = std::fs::read_to_string(addrfile).ok()?.trim().to_string();
    (!addr.is_empty()).then_some(addr)
}

fn base_url(addr: &str) -> String {
    if addr.starts_with("http://") || addr.starts_with("https://") {
        addr.to_string()
    } else {
        format!("http://{addr}")
    }
}

/// If a live daemon is described by the pidfile+addrfile, connect and return it.
/// "Live" = the named PID exists AND `/healthz` answers. On any failure the
/// stale files are removed and `None` is returned so the caller lock-acquires.
pub fn probe_existing(pidfile: &Path, addrfile: &Path) -> Option<DaemonClient> {
    let pid = read_pid(pidfile)?;
    let addr = read_addr(addrfile)?;
    if pid_alive(pid) {
        if let Ok(client) = DaemonClient::connect(base_url(&addr)) {
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
    // Startup lock placeholder, intentionally not a parseable daemon PID. The
    // daemon replaces it with its real PID before writing the addrfile.
    writeln!(f, "starting:{}", std::process::id())
}

fn health_client() -> std::result::Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(200))
        .build()
        .map_err(|e| e.to_string())
}

fn health_responds(http: &reqwest::blocking::Client, base: &str) -> bool {
    http.get(format!("{base}/healthz"))
        .send()
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Poll `<base>/healthz` every ~50ms until 200 or the deadline. Bounded.
#[cfg(test)]
fn wait_for_health(base: &str, timeout: Duration) -> std::result::Result<DaemonClient, String> {
    let deadline = Instant::now() + timeout;
    let http = health_client()?;
    while Instant::now() < deadline {
        if health_responds(&http, base) {
            return DaemonClient::connect(base.to_string()).map_err(|e| format!("{e:?}"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(format!(
        "daemon did not become healthy at {base} within {timeout:?}"
    ))
}

/// Poll the addrfile for the daemon's actual bound address, then health-check
/// that address. This is the spawn path source of truth for `--bind :0` and any
/// daemon-side bind fallback.
pub fn wait_for_addrfile_health(
    addrfile: &Path,
    timeout: Duration,
) -> std::result::Result<DaemonClient, String> {
    let deadline = Instant::now() + timeout;
    let http = health_client()?;
    let mut last_base = None;
    while Instant::now() < deadline {
        if let Some(addr) = read_addr(addrfile) {
            let base = base_url(&addr);
            if health_responds(&http, &base) {
                return DaemonClient::connect(base).map_err(|e| format!("{e:?}"));
            }
            last_base = Some(base);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    match last_base {
        Some(base) => Err(format!(
            "daemon wrote {base} but did not become healthy within {timeout:?}"
        )),
        None => Err(format!(
            "daemon did not write {} within {timeout:?}",
            addrfile.display()
        )),
    }
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
/// loser health-waits on the addrfile). Bounded retry on a lost race whose
/// winner crashed. `forced_addr` (from `--daemon`) skips spawning entirely.
pub fn ensure_daemon(
    config: &Config,
    forced_addr: Option<&str>,
    allow_spawn: bool,
) -> std::result::Result<DaemonClient, String> {
    if let Some(addr) = forced_addr {
        let base = base_url(addr);
        return DaemonClient::connect(base).map_err(|e| format!("--daemon {addr}: {e:?}"));
    }
    let (pidfile, addrfile) = runtime_files().ok_or("cannot determine runtime dir")?;
    let bind = config.daemon.bind.clone();
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
                // The daemon writes its actual bound addr on bind; use that,
                // not the configured bind, as the health-check target.
                return wait_for_addrfile_health(&addrfile, Duration::from_secs(5));
            }
            Err(_already_exists) => {
                // Someone else is starting it: wait for the winner's addrfile.
                if let Ok(client) = wait_for_addrfile_health(&addrfile, Duration::from_secs(5)) {
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
    use std::io::Read;

    fn spawn_healthz_server(requests: usize) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let handle = std::thread::spawn(move || {
            for _ in 0..requests {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 1024];
                let _ = stream.read(&mut request);
                let body = br#"{"ok":true,"version":"test"}"#;
                let header = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                std::io::Write::write_all(&mut stream, header.as_bytes()).unwrap();
                std::io::Write::write_all(&mut stream, body).unwrap();
            }
        });
        (addr, handle)
    }

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
    fn acquire_lock_placeholder_is_not_a_daemon_pid() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("kamajid.pid");
        acquire_lock(&pidfile).unwrap();
        let contents = std::fs::read_to_string(&pidfile).unwrap();
        assert!(contents.starts_with("starting:"));
        assert_eq!(read_pid(&pidfile), None);
    }

    #[test]
    fn health_wait_times_out_on_dead_port() {
        // Nothing listens on this port; bounded wait returns an error, not a hang.
        let started = std::time::Instant::now();
        let res = wait_for_health("http://127.0.0.1:1", std::time::Duration::from_millis(300));
        assert!(res.is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn addrfile_health_wait_uses_written_addr() {
        let dir = tempfile::tempdir().unwrap();
        let addrfile = dir.path().join("kamajid.addr");
        let (addr, handle) = spawn_healthz_server(2);
        std::fs::write(&addrfile, format!("{addr}\n")).unwrap();

        let client = wait_for_addrfile_health(&addrfile, std::time::Duration::from_secs(2))
            .expect("addrfile health wait should connect to written addr");

        let expected = format!("http://{addr}");
        assert_eq!(client.base(), expected.as_str());
        drop(client);
        handle.join().unwrap();
    }

    /// End-to-end: actually spawns the built `kamajid` detached and connects.
    /// Gated behind `--ignored` because it forks a real daemon and binds a port.
    #[cfg(unix)]
    #[test]
    #[ignore = "actually spawns the built kamajid detached; run with --ignored"]
    fn ensure_daemon_spawns_and_connects() {
        use kamaji_core::config::Config;

        // Serialize against other env-mutating tests (XDG_* are process-global);
        // held across the mutations + daemon spawn + asserts.
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        // Isolate every runtime/data/config base into the tempdir so the spawned
        // daemon and this test agree on the pidfile/addrfile location and the
        // daemon's own state lives nowhere durable.
        std::env::set_var("XDG_RUNTIME_DIR", dir.path());
        std::env::set_var("XDG_DATA_HOME", dir.path());
        std::env::set_var("XDG_CONFIG_HOME", dir.path());

        let mut cfg = Config::default();
        cfg.daemon.bind = "127.0.0.1:0".to_string();
        let client = ensure_daemon(&cfg, None, true)
            .expect("ensure_daemon should spawn and connect to a healthy daemon");
        // The returned client already pinged /healthz on connect; sanity-check it
        // again directly to prove the daemon is green.
        assert_ne!(
            client.base(),
            "http://127.0.0.1:0",
            "ensure_daemon must use the daemon's actual bound address"
        );
        let base = client.base().to_string();
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
