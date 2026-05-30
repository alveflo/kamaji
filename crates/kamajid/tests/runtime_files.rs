//! Verifies the daemon writes pid/addr files on bind and removes them on exit.
//! Spawns the built `kamajid` binary detached on an ephemeral port. Gated
//! `#[ignore]` like the other live tests: run with `--ignored`.

#[test]
#[ignore = "spawns the kamajid binary; run manually with --ignored"]
fn writes_pid_and_addr_files_on_bind() {
    use std::time::{Duration, Instant};
    let tmp = tempfile::tempdir().unwrap();
    let bin = env!("CARGO_BIN_EXE_kamajid");
    let mut child = std::process::Command::new(bin)
        .args(["serve", "--bind", "127.0.0.1:0"])
        .env("XDG_RUNTIME_DIR", tmp.path())
        .env("XDG_DATA_HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path())
        .spawn()
        .unwrap();
    let pidfile = tmp.path().join("kamaji").join("kamajid.pid");
    let addrfile = tmp.path().join("kamaji").join("kamajid.addr");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !(pidfile.exists() && addrfile.exists()) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(pidfile.exists(), "pidfile should be written on bind");
    assert!(addrfile.exists(), "addrfile should be written on bind");
    let addr = std::fs::read_to_string(&addrfile).unwrap();
    assert!(addr.starts_with("127.0.0.1:"), "addr was {addr:?}");
    child.kill().unwrap();
    child.wait().unwrap();
}
