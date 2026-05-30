//! Shared test-only helpers. Both `client.rs` and `engine.rs` need a real
//! in-process kamajid to exercise the blocking HTTP client against, so the
//! daemon-spawn helper lives here once.

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
