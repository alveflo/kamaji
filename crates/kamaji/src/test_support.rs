//! Shared test-only helpers. Both `client.rs` and `engine.rs` need a real
//! in-process kamajid to exercise the blocking HTTP client against, so the
//! daemon-spawn helper lives here once.

/// Serializes tests that mutate process-global env vars (e.g.
/// `XDG_CONFIG_HOME`) for isolation. `std::env::set_var` is process-wide, so
/// two such tests running in parallel can clobber each other's tempdir. Every
/// test that calls `set_var` for isolation must lock this for its whole
/// duration (env mutation + daemon spawn + assertions) as its first line.
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Boot a real kamajid on 127.0.0.1:0, returning its base URL. The tokio
/// runtime is kept alive in the spawned thread for the test's lifetime so the
/// server keeps serving. Uses an in-memory DB so it never touches the
/// developer's real database.
pub(crate) fn spawn_test_daemon() -> String {
    use kamaji_core::config::Config;
    use kamaji_core::db::Db;
    use kamajid::state::AppState;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        rt.block_on(async move {
            let state = AppState::new(Db::open_in_memory().unwrap(), Config::default());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tx.send(format!("http://{addr}")).unwrap();
            // kamajid::serve returns anyhow::Result; unwrap to propagate panics.
            kamajid::serve(listener, state).await.unwrap();
        });
    });
    rx.recv().unwrap()
}
