//! Background SSE listener: streams `GET /events`, decodes each record into a
//! `kamaji_core::events::Event` via `Event::from_sse`, and forwards `SseMsg`s
//! over an mpsc channel the sync UI loop drains. Reconnects with backoff.

use std::io::Read;
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::Duration;

use kamaji_core::events::Event;

#[allow(dead_code)]
pub enum SseMsg {
    Event(Box<Event>),
    Connected,
    Disconnected,
}

/// Pull complete SSE records out of an accumulating buffer. Returns the decoded
/// events and leaves any partial trailing record in `buf`. A record is the text
/// between blank-line separators; we read its `event:` and `data:` lines.
pub(crate) fn drain_records(buf: &mut String) -> Vec<Event> {
    let mut out = Vec::new();
    while let Some(idx) = buf.find("\n\n") {
        let record: String = buf.drain(..idx + 2).collect();
        let mut name = None;
        let mut data = None;
        for line in record.lines() {
            if let Some(v) = line.strip_prefix("event:") {
                name = Some(v.trim().to_string());
            } else if let Some(v) = line.strip_prefix("data:") {
                data = Some(v.trim().to_string());
            }
        }
        if let (Some(name), Some(data)) = (name, data) {
            if let Some(ev) = Event::from_sse(&name, &data) {
                out.push(ev);
            }
        }
    }
    out
}

/// Spawn the SSE listener thread. It connects to `<base>/events`, emits
/// `Connected` (→ UI re-fetch), streams `Event`s, and on stream end/error emits
/// `Disconnected` and retries with capped backoff (250ms → 2s). Ends when the
/// receiver is dropped (send fails).
#[allow(dead_code)]
pub fn spawn(base: String, tx: Sender<SseMsg>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let http = reqwest::blocking::Client::builder()
            .timeout(None) // a streaming response must not time out mid-stream
            .build()
            .expect("build sse client");
        let mut backoff = Duration::from_millis(250);
        loop {
            match http.get(format!("{base}/events")).send() {
                Ok(mut resp) if resp.status().is_success() => {
                    backoff = Duration::from_millis(250);
                    if tx.send(SseMsg::Connected).is_err() {
                        return;
                    }
                    let mut buf = String::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        match resp.read(&mut chunk) {
                            Ok(0) => break, // stream ended
                            Ok(n) => {
                                buf.push_str(&String::from_utf8_lossy(&chunk[..n]));
                                for ev in drain_records(&mut buf) {
                                    if tx.send(SseMsg::Event(Box::new(ev))).is_err() {
                                        return;
                                    }
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    if tx.send(SseMsg::Disconnected).is_err() {
                        return;
                    }
                }
                _ => {
                    if tx.send(SseMsg::Disconnected).is_err() {
                        return;
                    }
                }
            }
            std::thread::sleep(backoff);
            backoff = (backoff * 2).min(Duration::from_secs(2));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_parses_complete_records_and_keeps_partial() {
        let mut buf = String::from(
            "event: ticket.deleted\ndata: {\"id\":7}\n\nevent: session.idle\ndata: {\"ticket_id\":3}\n\nevent: ticket.del",
        );
        let events = drain_records(&mut buf);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sse_name(), "ticket.deleted");
        assert_eq!(events[1].sse_name(), "session.idle");
        // The incomplete trailing record stays buffered for the next chunk.
        assert!(buf.starts_with("event: ticket.del"));
    }

    #[test]
    fn drain_ignores_keepalive_comments() {
        // axum keep-alive sends `:` comment lines; they carry no event:/data:.
        let mut buf = String::from(": keep-alive\n\n");
        assert!(drain_records(&mut buf).is_empty());
    }

    /// Boot a real kamajid on 127.0.0.1:0, returning its base URL. The tokio
    /// runtime is kept alive in the spawned thread for the test's lifetime so
    /// the server keeps serving.
    fn spawn_daemon() -> String {
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
                kamajid::serve(listener, state).await.unwrap();
            });
        });
        rx.recv().unwrap()
    }

    #[test]
    #[ignore = "boots a daemon and exercises live streaming; run with --ignored"]
    fn live_listener_reports_connected_then_event() {
        let base = spawn_daemon();
        let (tx, rx) = std::sync::mpsc::channel::<SseMsg>();

        // Spawn the SSE listener pointing at the live daemon.
        let _handle = spawn(base.clone(), tx);

        // 1. The first message must be Connected (stream was established).
        match rx
            .recv_timeout(Duration::from_secs(5))
            .expect("expected Connected within 5s")
        {
            SseMsg::Connected => {}
            SseMsg::Disconnected => panic!("got Disconnected instead of Connected"),
            SseMsg::Event(_) => panic!("got Event before Connected"),
        }

        // 2. POST a project + ticket to trigger a ticket.created SSE event.
        let http = reqwest::blocking::Client::new();
        let p: serde_json::Value = http
            .post(format!("{base}/projects"))
            .json(&serde_json::json!({ "name": "sse-test", "root_dir": "/tmp/sse-test" }))
            .send()
            .unwrap()
            .json()
            .unwrap();
        let pid = p["id"].as_i64().unwrap();
        http.post(format!("{base}/tickets"))
            .json(&serde_json::json!({
                "project_id": pid,
                "title": "SSE live test ticket",
                "agent": "claude",
            }))
            .send()
            .unwrap();

        // 3. Expect a ticket.created event to arrive within 2 seconds.
        let mut got_ticket_created = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(SseMsg::Event(ev)) => {
                    if ev.sse_name() == "ticket.created" {
                        got_ticket_created = true;
                        break;
                    }
                    // Skip other events (e.g. keep-alive or unrelated events).
                    let _ = ev;
                }
                Ok(SseMsg::Connected) => {}
                Ok(SseMsg::Disconnected) => panic!("SSE listener disconnected during test"),
                Err(_) => break, // timeout
            }
        }
        assert!(
            got_ticket_created,
            "expected ticket.created SSE event within 2s"
        );
    }
}
