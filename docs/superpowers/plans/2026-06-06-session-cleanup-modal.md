# Session Cleanup Modal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a "Sessions" button to the browser board topbar that opens a modal listing all active `kamaji-*` zellij sessions with per-session checkboxes, letting the user select sessions and, after an in-modal confirm, tear them down.

**Architecture:** A pure `classify_sessions` helper in `kamaji-core` turns `zellij list-sessions` output + the DB into a typed list (ticket-linked / main / orphan). The daemon gains a `GET /ui/sessions/manage` route that renders a maud modal fragment (morphed into `#modal` via Datastar, exactly like the new-ticket modal) and a `POST /sessions/delete` command route that re-classifies server-side and tears each session down with the smart rule: ticket-linked → `session::cleanup_ticket` (full teardown), orphan/main → `zellij` terminate. Ticket-linked deletes emit `SessionExited` so the board updates live. The session-list read is routed through the existing `SessionDriver` seam so the command route is testable without a real zellij.

**Tech Stack:** Rust, axum, maud, Datastar (RC.6 colon bindings), rust-embed assets, rusqlite (via `with_db` blocking pool), tokio.

---

## File Structure

| File | Responsibility | Action |
|------|----------------|--------|
| `crates/kamaji-core/src/session.rs` | Add `SessionKind`, `SessionEntry`, `classify_sessions` (pure parse + DB classify) | Modify |
| `crates/kamajid/src/session_driver.rs` | Add `list_sessions()` to the `SessionDriver` trait + `RealSessionDriver` impl + a configurable canned list on `FakeSessionDriver` | Modify |
| `crates/kamajid/src/views/sessions.rs` | The sessions-cleanup modal fragment (`sessions_modal`) | Create |
| `crates/kamajid/src/views/mod.rs` | Register `pub mod sessions;` | Modify |
| `crates/kamajid/src/routes/ui.rs` | `GET /ui/sessions/manage` handler (fetch list → classify → render) | Modify |
| `crates/kamajid/src/routes/sessions.rs` | `POST /sessions/delete` command handler | Create |
| `crates/kamajid/src/routes/mod.rs` | Register `pub mod sessions;` | Modify |
| `crates/kamajid/src/lib.rs` | Mount the two new routes | Modify |
| `crates/kamajid/src/views/page.rs` | "Sessions" topbar button + link `sessions.css` | Modify |
| `crates/kamajid/src/assets/sessions.css` | Modal row / tag / warning styling | Create |
| `crates/kamajid/tests/ui.rs` | Integration test for `GET /ui/sessions/manage` | Modify |
| `crates/kamajid/tests/api.rs` | Integration tests for `POST /sessions/delete` | Modify |

---

## Task 1: `classify_sessions` helper in kamaji-core

**Files:**
- Modify: `crates/kamaji-core/src/session.rs`
- Test: `crates/kamaji-core/src/session.rs` (`#[cfg(test)] mod tests`)

This is the logic-heavy, pure unit. It takes the raw `zellij list-sessions`
output and the DB and returns one typed entry per unique `kamaji-*` session.

- [ ] **Step 1: Write the failing test**

Add these tests inside the existing `mod tests` block at the bottom of `session.rs`:

```rust
#[test]
fn classify_sessions_labels_ticket_main_and_orphan() {
    let db = Db::open_in_memory().unwrap();
    let p = db
        .create_project("p", std::path::Path::new("/tmp/p"), None)
        .unwrap();
    // A ticket with a live session.
    let t = db.create_ticket(p.id, "Fix auth", "", None, Agent::Claude).unwrap();
    db.set_ticket_session(t.id, "kamaji-1-fix-auth", "/wt", "kamaji-1-fix-auth")
        .unwrap();

    // List: the ticket session, the project's main session, an orphan kamaji
    // session, a non-kamaji session (ignored), and a duplicate of the ticket
    // session as an EXITED/resurrectable entry (must dedupe to one).
    let main = crate::slug::main_session_name(p.id);
    let list = format!(
        "kamaji-1-fix-auth [Created 2h ago]\n\
         {main} [Created 1h ago]\n\
         kamaji-7-old [Created 3h ago]\n\
         other-session (current)\n\
         kamaji-1-fix-auth [Created 2h ago] (EXITED - attach to resurrect)\n"
    );

    let entries = classify_sessions(&db, &list).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    // Deduped, kamaji-only, in list order.
    assert_eq!(names, vec!["kamaji-1-fix-auth", main.as_str(), "kamaji-7-old"]);

    match &entries[0].kind {
        SessionKind::Ticket { id, title, status } => {
            assert_eq!(*id, t.id);
            assert_eq!(title, "Fix auth");
            assert_eq!(*status, Status::InProgress); // set_ticket_session moves to In Progress? no — assert Todo
        }
        other => panic!("expected ticket kind, got {other:?}"),
    }
    assert!(matches!(entries[1].kind, SessionKind::Main));
    assert!(matches!(entries[2].kind, SessionKind::Orphan));
}

#[test]
fn classify_sessions_empty_list_is_empty() {
    let db = Db::open_in_memory().unwrap();
    assert!(classify_sessions(&db, "").unwrap().is_empty());
}
```

NOTE on the status assertion: `set_ticket_session` does NOT change status (only
`commit_session` does). A freshly created ticket is `Status::Todo`. Fix the
assertion to `assert_eq!(*status, Status::Todo);` before running.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p kamaji-core classify_sessions`
Expected: FAIL — `cannot find function classify_sessions` / `SessionKind` / `SessionEntry` not found.

- [ ] **Step 3: Write minimal implementation**

At the top of `session.rs`, add to the imports:

```rust
use std::collections::{HashMap, HashSet};
```

Add the types + function (place them above the `#[cfg(test)]` block):

```rust
/// How a live `kamaji-*` zellij session relates to the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionKind {
    /// Bound to a ticket via its recorded `session_name`.
    Ticket { id: i64, title: String, status: Status },
    /// A project's bare "main" workspace (`kamaji-main-<project-id>`).
    Main,
    /// A `kamaji-*` session with no matching ticket and not a main session.
    Orphan,
}

/// One classified zellij session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    pub name: String,
    pub kind: SessionKind,
}

/// Classify the `kamaji-*` sessions in raw `zellij list-sessions` output against
/// the DB. The first whitespace-delimited token of each line is the session
/// name; non-`kamaji-` names are dropped, and duplicate names (zellij lists a
/// session and its EXITED/resurrectable stub separately) collapse to the first
/// occurrence, preserving list order. A name matching a ticket's recorded
/// `session_name` is `Ticket`; one matching `kamaji-main-<project-id>` is `Main`;
/// anything else is `Orphan`.
pub fn classify_sessions(db: &Db, list_output: &str) -> Result<Vec<SessionEntry>> {
    let projects = db.list_projects()?;
    let main_names: HashSet<String> = projects
        .iter()
        .map(|p| slug::main_session_name(p.id))
        .collect();
    // session_name -> (ticket id, title, status)
    let mut ticket_by_session: HashMap<String, (i64, String, Status)> = HashMap::new();
    for p in &projects {
        for t in db.list_tickets(p.id)? {
            if let Some(name) = t.session_name {
                ticket_by_session.insert(name, (t.id, t.title, t.status));
            }
        }
    }

    let mut seen: HashSet<&str> = HashSet::new();
    let mut entries = Vec::new();
    for line in list_output.lines() {
        let Some(name) = line.split_whitespace().next() else {
            continue;
        };
        if !name.starts_with("kamaji-") || !seen.insert(name) {
            continue;
        }
        let kind = if let Some((id, title, status)) = ticket_by_session.get(name) {
            SessionKind::Ticket {
                id: *id,
                title: title.clone(),
                status: *status,
            }
        } else if main_names.contains(name) {
            SessionKind::Main
        } else {
            SessionKind::Orphan
        };
        entries.push(SessionEntry {
            name: name.to_string(),
            kind,
        });
    }
    Ok(entries)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p kamaji-core classify_sessions`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/kamaji-core/src/session.rs
git commit -m "feat(core): classify_sessions helper for ticket/main/orphan zellij sessions"
```

---

## Task 2: `list_sessions()` on the SessionDriver seam

**Files:**
- Modify: `crates/kamajid/src/session_driver.rs`
- Test: `crates/kamajid/src/session_driver.rs` (`#[cfg(test)] mod tests`)

The command route must read the session list through a seam so tests can inject
a canned list (no real zellij in CI). Add `list_sessions` to the trait, delegate
to `kamaji_core::zellij::list_sessions` in the real driver, and give the fake a
configurable list.

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `session_driver.rs`:

```rust
#[test]
fn fake_returns_configured_session_list() {
    // Default: no list (models "couldn't ask zellij").
    let d = FakeSessionDriver::new(true);
    assert_eq!(d.list_sessions(), None);

    // Configured list is returned verbatim.
    let d = FakeSessionDriver::new(true).with_sessions("kamaji-1-x [Created 1h ago]\n");
    assert_eq!(
        d.list_sessions().as_deref(),
        Some("kamaji-1-x [Created 1h ago]\n")
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p kamajid --lib fake_returns_configured_session_list`
Expected: FAIL — no method `list_sessions` / no method `with_sessions`.

NOTE: `kamajid` is a binary+lib crate; its unit tests build with `--lib`. (The
integration tests in `tests/` build without `--lib`.)

- [ ] **Step 3: Write minimal implementation**

Add the method to the trait (after `create_background`):

```rust
    /// Raw `zellij list-sessions` output, or `None` if zellij couldn't be asked.
    /// Mirrors [`kamaji_core::zellij::list_sessions`]; behind the seam so the
    /// session-management routes are testable with a canned list.
    fn list_sessions(&self) -> Option<String>;
```

Implement it on `RealSessionDriver`:

```rust
    fn list_sessions(&self) -> Option<String> {
        kamaji_core::zellij::list_sessions()
    }
```

Give `FakeSessionDriver` a `sessions` field + builder. Change the struct:

```rust
pub struct FakeSessionDriver {
    live: bool,
    sessions: Option<String>,
    created: Mutex<Vec<CreatedSession>>,
    terminated: Mutex<Vec<String>>,
}
```

Update `new` to initialize it, and add a builder:

```rust
    pub fn new(live: bool) -> Self {
        FakeSessionDriver {
            live,
            sessions: None,
            created: Mutex::new(Vec::new()),
            terminated: Mutex::new(Vec::new()),
        }
    }

    /// Set the canned `list-sessions` output this fake reports.
    pub fn with_sessions(mut self, list: &str) -> Self {
        self.sessions = Some(list.to_string());
        self
    }
```

Implement the trait method on the fake:

```rust
    fn list_sessions(&self) -> Option<String> {
        self.sessions.clone()
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p kamajid --lib session_driver`
Expected: PASS (existing `fake_records_create_and_terminate`, `fake_live_reports_live`, and the new test).

- [ ] **Step 5: Commit**

```bash
git add crates/kamajid/src/session_driver.rs
git commit -m "feat(daemon): list_sessions() on the SessionDriver seam"
```

---

## Task 3: The sessions modal view

**Files:**
- Create: `crates/kamajid/src/views/sessions.rs`
- Modify: `crates/kamajid/src/views/mod.rs`
- Test: `crates/kamajid/src/views/sessions.rs` (`#[cfg(test)] mod tests`)

A pure maud fragment rooted at `#modal` (so Datastar's morph-by-id replaces the
mount), mirroring `views::modal`. Behavior is inline `data-on:*` handlers
(single-quoted JS), consistent with `modal.rs`.

- [ ] **Step 1: Write the failing test**

Create `crates/kamajid/src/views/sessions.rs` with ONLY the tests first (the
`use super::*;` will fail to compile until Step 3 adds the items — that is the
intended red state):

```rust
//! The session-cleanup modal fragment. Returned by `GET /ui/sessions/manage`.
//! Rooted at `#modal` so Datastar's `@get` morph-by-id replaces the page's empty
//! mount. Checkboxes select sessions; the footer swaps to an in-modal confirm
//! bar before POSTing `/sessions/delete`. Bindings use the RC.6 colon form and
//! single-quoted JS (no HTML-attribute escaping needed), like `views::modal`.

use kamaji_core::session::{SessionEntry, SessionKind};
use maud::{html, Markup, PreEscaped};

#[cfg(test)]
mod tests {
    use super::*;
    use kamaji_core::models::Status;

    fn entries() -> Vec<SessionEntry> {
        vec![
            SessionEntry {
                name: "kamaji-12-fix-auth".into(),
                kind: SessionKind::Ticket {
                    id: 12,
                    title: "Fix auth".into(),
                    status: Status::Review,
                },
            },
            SessionEntry {
                name: "kamaji-main-3".into(),
                kind: SessionKind::Main,
            },
            SessionEntry {
                name: "kamaji-7-old".into(),
                kind: SessionKind::Orphan,
            },
        ]
    }

    #[test]
    fn fragment_is_rooted_at_modal_with_dialog() {
        let html = sessions_modal(&entries(), true).into_string();
        assert!(html.starts_with(r#"<div id="modal">"#), "{html}");
        assert!(html.contains("<dialog"), "{html}");
        assert!(html.contains(r#"class="modal-title">Sessions"#), "{html}");
    }

    #[test]
    fn renders_a_checkbox_row_per_session_with_kind_labels() {
        let html = sessions_modal(&entries(), true).into_string();
        // One checkbox per session, value = session name.
        for name in ["kamaji-12-fix-auth", "kamaji-main-3", "kamaji-7-old"] {
            assert!(
                html.contains(&format!(r#"name="session" value="{name}""#)),
                "checkbox for {name}:\n{html}"
            );
        }
        // Kind labels.
        assert!(html.contains("Fix auth"), "ticket title:\n{html}");
        assert!(html.contains("#12"), "ticket id:\n{html}");
        assert!(html.contains("project workspace"), "main label:\n{html}");
        assert!(html.contains("orphan"), "orphan label:\n{html}");
        // Main rows carry the warning class.
        assert!(html.contains("sess-warn"), "main row flagged:\n{html}");
    }

    #[test]
    fn confirm_bar_posts_selected_to_sessions_delete() {
        let html = sessions_modal(&entries(), true).into_string();
        assert!(html.contains("sess-foot-confirm"), "confirm bar present:\n{html}");
        assert!(
            html.contains("fetch('/sessions/delete',{method:'POST'"),
            "confirm posts to delete:\n{html}"
        );
        assert!(
            html.contains("input[name=session]:checked"),
            "gathers checked names:\n{html}"
        );
        // Colon bindings only.
        assert!(html.contains("data-on:click="), "colon bindings:\n{html}");
        assert!(!html.contains("data-on-click"), "no inert hyphen:\n{html}");
    }

    #[test]
    fn empty_list_shows_empty_state() {
        let html = sessions_modal(&[], true).into_string();
        assert!(html.contains("No active kamaji sessions"), "{html}");
        // Nothing to delete: no checkboxes.
        assert!(!html.contains(r#"name="session""#), "{html}");
    }

    #[test]
    fn unreachable_zellij_shows_a_note() {
        let html = sessions_modal(&[], false).into_string();
        assert!(html.contains("Could not reach zellij"), "{html}");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p kamajid --lib views::sessions`
Expected: FAIL — `cannot find function sessions_modal` (and `views::sessions` not declared yet, so also a module error until mod.rs is updated; declare it now to get a clean compile error).

Add to `crates/kamajid/src/views/mod.rs` (keep the list alphabetical):

```rust
pub mod sessions;
```

Re-run; expected: FAIL on `sessions_modal` not found.

- [ ] **Step 3: Write minimal implementation**

Add the implementation to `crates/kamajid/src/views/sessions.rs` (above the
`#[cfg(test)]` block):

```rust
/// JS that clears the `#modal` mount. Reused by Cancel, the close button, the
/// Escape handler, and the confirm-success `.then`.
const CLEAR_MODAL_JS: &str = "document.getElementById('modal').replaceChildren()";

/// The session-cleanup modal. `entries` is the classified session list (snapshot
/// at open time); `zellij_reachable` is false when `list-sessions` couldn't be
/// queried, which shows an explanatory note instead of an empty list.
pub fn sessions_modal(entries: &[SessionEntry], zellij_reachable: bool) -> Markup {
    let escape_handler = format!("if(evt.key==='Escape'){{{CLEAR_MODAL_JS}}}");

    // Recompute the checked count → enable/relabel the Delete button.
    let recount = "const d=el.closest('dialog');const n=d.querySelectorAll('input[name=session]:checked').length;const b=d.querySelector('#sess-del-btn');b.disabled=n===0;b.textContent='Delete selected ('+n+')'";
    // Delete → swap the footer to the confirm bar (no-op when nothing checked).
    let show_confirm = "const d=el.closest('dialog');const n=d.querySelectorAll('input[name=session]:checked').length;if(n===0)return;d.querySelector('#sess-confirm-text').textContent='Delete '+n+' session(s)? Ticket sessions also remove their git worktree and branch.';d.querySelector('#sess-foot-main').hidden=true;d.querySelector('#sess-foot-confirm').hidden=false";
    // Back → restore the main footer.
    let hide_confirm = "const d=el.closest('dialog');d.querySelector('#sess-foot-confirm').hidden=true;d.querySelector('#sess-foot-main').hidden=false";
    // Confirm → POST the checked names; clear the modal on a 2xx.
    let do_delete = format!(
        "const d=el.closest('dialog');const names=[...d.querySelectorAll('input[name=session]:checked')].map(c=>c.value);fetch('/sessions/delete',{{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify({{names}})}}).then(r=>{{if(r.ok){{{CLEAR_MODAL_JS}}}}})"
    );

    html! {
        div id="modal" {
            dialog open class="modal" id="sessions-dialog"
                   data-on:keydown__window=(PreEscaped(escape_handler)) {
                div class="modal-head" {
                    span class="modal-title" { "Sessions" }
                    button type="button" class="modal-close"
                           data-on:click=(PreEscaped(CLEAR_MODAL_JS)) { "✕" }
                }
                div class="modal-body" {
                    @if !zellij_reachable {
                        p class="sess-empty" { "Could not reach zellij." }
                    } @else if entries.is_empty() {
                        p class="sess-empty" { "No active kamaji sessions." }
                    } @else {
                        div id="sess-list" class="sess-list"
                            data-on:change=(PreEscaped(recount)) {
                            @for e in entries {
                                (session_row(e))
                            }
                        }
                    }
                }
                div class="modal-foot" {
                    div id="sess-foot-main" class="sess-foot-row" {
                        button type="button" class="btn"
                               data-on:click=(PreEscaped(CLEAR_MODAL_JS)) { "Cancel" }
                        button type="button" class="btn btn-danger" id="sess-del-btn" disabled
                               data-on:click=(PreEscaped(show_confirm)) { "Delete selected (0)" }
                    }
                    div id="sess-foot-confirm" class="sess-foot-row" hidden {
                        span id="sess-confirm-text" class="sess-confirm-text" {}
                        span class="foot-spacer" {}
                        button type="button" class="btn"
                               data-on:click=(PreEscaped(hide_confirm)) { "Back" }
                        button type="button" class="btn btn-danger"
                               data-on:click=(PreEscaped(do_delete)) { "Confirm" }
                    }
                }
            }
        }
    }
}

/// One checkbox row. Main sessions carry `sess-warn` (heavier to kill: a project
/// workspace) and a ⚠ marker. The checkbox value is the raw session name.
fn session_row(e: &SessionEntry) -> Markup {
    let warn = matches!(e.kind, SessionKind::Main);
    html! {
        label class=(if warn { "check sess-row sess-warn" } else { "check sess-row" }) {
            input type="checkbox" name="session" value=(e.name);
            span class="sess-name" { (e.name) }
            (kind_tag(&e.kind))
        }
    }
}

/// The right-aligned descriptor for a session's kind.
fn kind_tag(kind: &SessionKind) -> Markup {
    match kind {
        SessionKind::Ticket { id, title, status } => html! {
            span class="sess-tag sess-tag-ticket" {
                "ticket #" (id) " · \"" (title) "\" · " (status.title())
            }
        },
        SessionKind::Main => html! {
            span class="sess-tag sess-tag-main" { "project workspace ⚠" }
        },
        SessionKind::Orphan => html! {
            span class="sess-tag sess-tag-orphan" { "orphan" }
        },
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p kamajid --lib views::sessions`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/kamajid/src/views/sessions.rs crates/kamajid/src/views/mod.rs
git commit -m "feat(daemon): sessions-cleanup modal view"
```

---

## Task 4: `GET /ui/sessions/manage` route

**Files:**
- Modify: `crates/kamajid/src/routes/ui.rs`
- Modify: `crates/kamajid/src/lib.rs`
- Test: `crates/kamajid/tests/ui.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/kamajid/tests/ui.rs` (it already has `mod support;` and the
SSE helpers; this test only needs a spawn + a GET). Add a local spawn helper
that injects a fake driver with a canned session list, then asserts the rendered
fragment. Put this at the end of the file:

```rust
/// Boot a daemon whose session driver reports a fixed `list-sessions` output,
/// and seed one ticket whose session is in that list.
async fn spawn_with_sessions(list: &str) -> (String, AppState) {
    let mut state = AppState::new(Db::open_in_memory().unwrap(), Config::default());
    state.set_session_driver(std::sync::Arc::new(
        kamajid::session_driver::FakeSessionDriver::new(true).with_sessions(list),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = kamajid::router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), state)
}

#[tokio::test]
async fn manage_sessions_modal_lists_classified_sessions() {
    let list = "kamaji-1-fix-auth [Created 2h ago]\nkamaji-9-orphan [Created 1h ago]\n";
    let (base, state) = spawn_with_sessions(list).await;
    state
        .with_db(|db| {
            let p = db.create_project("p", std::path::Path::new("/tmp/p"), None)?;
            let t = db.create_ticket(p.id, "Fix auth", "", None, kamaji_core::models::Agent::Claude)?;
            db.set_ticket_session(t.id, "kamaji-1-fix-auth", "/wt", "kamaji-1-fix-auth")?;
            Ok(())
        })
        .await
        .unwrap();

    let html = reqwest::get(format!("{base}/ui/sessions/manage"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.starts_with(r#"<div id="modal">"#), "{html}");
    assert!(html.contains(r#"value="kamaji-1-fix-auth""#), "ticket session row:\n{html}");
    assert!(html.contains("Fix auth"), "ticket title:\n{html}");
    assert!(html.contains(r#"value="kamaji-9-orphan""#), "orphan row:\n{html}");
    assert!(html.contains("orphan"), "orphan label:\n{html}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p kamajid --test ui manage_sessions_modal_lists_classified_sessions`
Expected: FAIL — 404 (route not mounted), so the `starts_with` assert fails.

- [ ] **Step 3: Write minimal implementation**

In `crates/kamajid/src/routes/ui.rs`, add the import near the top:

```rust
use kamaji_core::session;
```

Add the handler at the end of the file:

```rust
/// `GET /ui/sessions/manage` → the session-cleanup modal fragment. Snapshots
/// `zellij list-sessions` (through the session-driver seam, off the async
/// runtime since it shells out), classifies each `kamaji-*` session against the
/// DB, and renders the modal. When zellij can't be queried the modal shows an
/// explanatory note rather than erroring.
pub async fn manage_sessions(State(state): State<AppState>) -> Result<Markup, ApiError> {
    let st = state.clone();
    let list = tokio::task::spawn_blocking(move || st.sessions().list_sessions())
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("list-sessions task panicked: {e}")))?;
    let reachable = list.is_some();
    let entries = match list {
        Some(list) => {
            state
                .with_db(move |db| session::classify_sessions(db, &list))
                .await?
        }
        None => Vec::new(),
    };
    Ok(views::sessions::sessions_modal(&entries, reachable))
}
```

NOTE: `anyhow` is already a dependency of the crate (used across routes). If the
import is missing in this file, fully-qualify as `anyhow::anyhow!` (shown above)
— no `use` needed.

Mount it in `crates/kamajid/src/lib.rs`, next to the other `/ui/...` GET routes
(after the `"/ui/tickets/:id/terminal"` line):

```rust
        .route("/ui/sessions/manage", get(routes::ui::manage_sessions))
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p kamajid --test ui manage_sessions_modal_lists_classified_sessions`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/kamajid/src/routes/ui.rs crates/kamajid/src/lib.rs crates/kamajid/tests/ui.rs
git commit -m "feat(daemon): GET /ui/sessions/manage modal route"
```

---

## Task 5: `POST /sessions/delete` command route

**Files:**
- Create: `crates/kamajid/src/routes/sessions.rs`
- Modify: `crates/kamajid/src/routes/mod.rs`
- Modify: `crates/kamajid/src/lib.rs`
- Test: `crates/kamajid/tests/api.rs`

Smart per-session teardown. Re-classifies server-side (never trusts the client),
then: ticket-linked → `session::cleanup_ticket` (full teardown) + emit
`SessionExited`; orphan/main → driver `terminate`; a `kamaji-*` name no longer in
the list → idempotent success (already gone); a non-`kamaji-` name → reported as
failed. Returns `{ "deleted": N, "failed": [{ "name", "reason" }, ...] }`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/kamajid/tests/api.rs`. The first helper boots a daemon with a
fake driver + a canned list; the ticket-linked test needs a real committed git
repo (via `support::committed_repo`) so `cleanup_ticket`'s worktree removal runs.

```rust
/// Boot a daemon with a fake session driver reporting `list`, a temp state dir,
/// and return the base URL + state (to seed + inspect) + the driver handle (to
/// assert terminate calls).
async fn spawn_with_session_list(
    list: &str,
) -> (
    String,
    AppState,
    std::sync::Arc<kamajid::session_driver::FakeSessionDriver>,
) {
    let driver = std::sync::Arc::new(
        kamajid::session_driver::FakeSessionDriver::new(true).with_sessions(list),
    );
    let mut state = AppState::new(Db::open_in_memory().unwrap(), Config::default());
    state.set_session_driver(driver.clone());
    let sd = tempfile::tempdir().unwrap();
    state.set_state_dir(sd.path().to_path_buf());
    // Leak the tempdir so it outlives the test body (state holds only the path).
    std::mem::forget(sd);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = kamajid::router(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), state, driver)
}

#[tokio::test]
async fn delete_sessions_rejects_empty_names() {
    let (base, _state, _d) = spawn_with_session_list("").await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/sessions/delete"))
        .json(&serde_json::json!({ "names": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn delete_orphan_session_terminates_via_driver() {
    let list = "kamaji-9-orphan [Created 1h ago]\n";
    let (base, _state, driver) = spawn_with_session_list(list).await;
    let body: serde_json::Value = reqwest::Client::new()
        .post(format!("{base}/sessions/delete"))
        .json(&serde_json::json!({ "names": ["kamaji-9-orphan"] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["deleted"], 1);
    assert!(body["failed"].as_array().unwrap().is_empty());
    assert_eq!(driver.terminated(), vec!["kamaji-9-orphan".to_string()]);
}

#[tokio::test]
async fn delete_non_kamaji_name_is_reported_failed() {
    let (base, _state, driver) = spawn_with_session_list("").await;
    let body: serde_json::Value = reqwest::Client::new()
        .post(format!("{base}/sessions/delete"))
        .json(&serde_json::json!({ "names": ["some-random-session"] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["deleted"], 0);
    assert_eq!(body["failed"][0]["name"], "some-random-session");
    // A non-kamaji name is never terminated.
    assert!(driver.terminated().is_empty());
}

#[tokio::test]
async fn delete_already_gone_kamaji_session_is_idempotent_success() {
    // Empty list → the name isn't present; treated as already-gone success.
    let (base, _state, driver) = spawn_with_session_list("").await;
    let body: serde_json::Value = reqwest::Client::new()
        .post(format!("{base}/sessions/delete"))
        .json(&serde_json::json!({ "names": ["kamaji-5-vanished"] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["deleted"], 1);
    assert!(driver.terminated().is_empty(), "nothing to terminate");
}

#[tokio::test]
async fn delete_ticket_session_full_teardown_and_event() {
    // A real committed repo so cleanup_ticket's worktree removal runs.
    let repo = support::committed_repo();
    let root = repo.path().to_path_buf();
    let worktree = root.join("..").join("kamaji-1-x-wt");
    let _ = kamaji_core::git::remove_worktree(&root, &worktree);
    kamaji_core::git::add_worktree(&root, &worktree, "kamaji-1-x", "main").unwrap();
    assert!(worktree.exists());

    let list = "kamaji-1-x [Created 1h ago]\n";
    let (base, state, _driver) = spawn_with_session_list(list).await;
    let tid = state
        .with_db({
            let root = root.clone();
            let wt = worktree.to_string_lossy().to_string();
            move |db| {
                let p = db.create_project("p", &root, None)?;
                let t = db.create_ticket(p.id, "x", "", None, kamaji_core::models::Agent::Claude)?;
                db.set_ticket_session(t.id, "kamaji-1-x", &wt, "kamaji-1-x")?;
                db.set_ticket_status(t.id, kamaji_core::models::Status::InProgress)?;
                Ok(t.id)
            }
        })
        .await
        .unwrap();

    // Subscribe so we can assert the SessionExited broadcast.
    let mut rx = state.tx.subscribe();

    let body: serde_json::Value = reqwest::Client::new()
        .post(format!("{base}/sessions/delete"))
        .json(&serde_json::json!({ "names": ["kamaji-1-x"] }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["deleted"], 1);
    assert!(body["failed"].as_array().unwrap().is_empty());

    // Worktree removed and the ticket's session columns cleared.
    assert!(!worktree.exists(), "worktree torn down");
    let cleared = state
        .with_db(move |db| Ok(db.get_ticket(tid)?.unwrap().session_name))
        .await
        .unwrap();
    assert_eq!(cleared, None);

    // SessionExited emitted for the ticket.
    let ev = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("event within 2s")
        .unwrap();
    match ev {
        kamaji_core::events::Event::SessionExited { ticket_id, session_name } => {
            assert_eq!(ticket_id, tid);
            assert_eq!(session_name, "kamaji-1-x");
        }
        other => panic!("expected SessionExited, got {other:?}"),
    }
}
```

NOTE: this assumes `crates/kamaji-core/src/git.rs` exposes `add_worktree` and
`remove_worktree` as `pub` (they are — `session.rs` and its tests call
`crate::git::add_worktree` / `remove_worktree`). If `kamaji_core::git` is not
`pub`, use the same access path the core tests use.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kamajid --test api delete_`
Expected: FAIL — 404 (route not mounted) on every `delete_*` test.

- [ ] **Step 3: Write minimal implementation**

Create `crates/kamajid/src/routes/sessions.rs`:

```rust
//! Session-management command routes. `POST /sessions/delete` tears down the
//! selected zellij sessions with a smart per-session rule: a ticket-linked
//! session gets the full `session::cleanup_ticket` (kill + remove worktree +
//! delete branch + clear DB columns) and a `session.exited` broadcast; an orphan
//! or main/project session is just killed via the session-driver seam. The
//! request's session kinds are re-derived server-side from the live DB + zellij
//! list — the client's checkbox values are only a selection set, never trusted.

use axum::extract::State;
use axum::Json;
use kamaji_core::events::Event;
use kamaji_core::session::{self, SessionEntry, SessionKind};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct DeleteSessions {
    pub names: Vec<String>,
}

/// `POST /sessions/delete` → tear down the named sessions. Body
/// `{ "names": ["kamaji-…", …] }`. Returns `{ "deleted": N, "failed": [...] }`.
/// Per-session failures are collected, not fatal: one bad name never aborts the
/// batch.
pub async fn delete(
    State(state): State<AppState>,
    Json(body): Json<DeleteSessions>,
) -> Result<Json<Value>, ApiError> {
    if body.names.is_empty() {
        return Err(ApiError::BadRequest("no sessions selected".into()));
    }

    // Snapshot + classify the live sessions once (off the runtime: list shells out).
    let st = state.clone();
    let list = tokio::task::spawn_blocking(move || st.sessions().list_sessions())
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("list-sessions task panicked: {e}")))?;
    let entries: Vec<SessionEntry> = match list {
        Some(list) => {
            state
                .with_db(move |db| session::classify_sessions(db, &list))
                .await?
        }
        None => Vec::new(),
    };

    let mut deleted = 0u64;
    let mut failed: Vec<Value> = Vec::new();

    for name in &body.names {
        if !name.starts_with("kamaji-") {
            failed.push(json!({ "name": name, "reason": "not a kamaji session" }));
            continue;
        }
        match entries.iter().find(|e| &e.name == name).map(|e| &e.kind) {
            // Ticket-linked: full teardown + SessionExited.
            Some(SessionKind::Ticket { id, .. }) => {
                let id = *id;
                let state_dir = state.state_dir().to_path_buf();
                let res = state
                    .with_db(move |db| {
                        let Some(t) = db.get_ticket(id)? else {
                            return Ok(false);
                        };
                        let Some(p) = db.get_project(t.project_id)? else {
                            return Ok(false);
                        };
                        session::cleanup_ticket(db, &p.root_dir, &state_dir, id)?;
                        Ok(true)
                    })
                    .await;
                match res {
                    Ok(true) => {
                        deleted += 1;
                        state.emit(Event::SessionExited {
                            ticket_id: id,
                            session_name: name.clone(),
                        });
                    }
                    // Ticket vanished between classify and teardown → already gone.
                    Ok(false) => deleted += 1,
                    Err(ApiError::Internal(e)) => {
                        failed.push(json!({ "name": name, "reason": e.to_string() }));
                    }
                    Err(e) => {
                        failed.push(json!({ "name": name, "reason": format!("{e:?}") }));
                    }
                }
            }
            // Orphan / main: kill via the driver (best-effort, off the runtime).
            Some(SessionKind::Main) | Some(SessionKind::Orphan) => {
                let st = state.clone();
                let n = name.clone();
                let _ = tokio::task::spawn_blocking(move || st.sessions().terminate(&n)).await;
                deleted += 1;
            }
            // A kamaji-* name not in the live list: already gone → idempotent success.
            None => deleted += 1,
        }
    }

    Ok(Json(json!({ "deleted": deleted, "failed": failed })))
}
```

Register the module in `crates/kamajid/src/routes/mod.rs` (alphabetical, after
`projects`):

```rust
pub mod sessions;
```

Mount the route in `crates/kamajid/src/lib.rs` (near the other command routes):

```rust
        .route(
            "/sessions/delete",
            axum::routing::post(routes::sessions::delete),
        )
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kamajid --test api delete_`
Expected: PASS (5 tests). The ticket-linked test needs `git` on PATH (CI has it;
`support::committed_repo` already relies on it).

- [ ] **Step 5: Commit**

```bash
git add crates/kamajid/src/routes/sessions.rs crates/kamajid/src/routes/mod.rs crates/kamajid/src/lib.rs crates/kamajid/tests/api.rs
git commit -m "feat(daemon): POST /sessions/delete with smart per-session teardown"
```

---

## Task 6: Topbar "Sessions" button

**Files:**
- Modify: `crates/kamajid/src/views/page.rs`
- Test: `crates/kamajid/src/views/page.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `page.rs`:

```rust
#[test]
fn topbar_has_sessions_button_opening_the_manage_modal() {
    let p = project(1, "acme");
    let html = page(&p, std::slice::from_ref(&p), &empty_board()).into_string();
    assert!(
        html.contains(r#"class="sessions-btn""#),
        "sessions button present:\n{html}"
    );
    assert!(
        html.contains("@get('/ui/sessions/manage')"),
        "sessions button opens the manage modal:\n{html}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p kamajid --lib topbar_has_sessions_button_opening_the_manage_modal`
Expected: FAIL — `sessions-btn` not in the rendered page.

- [ ] **Step 3: Write minimal implementation**

In `crates/kamajid/src/views/page.rs`, in the topbar, add a button just BEFORE
the existing "+ New" button (after `span class="spacer" {}`):

```rust
                        button class="sessions-btn"
                               data-on:click=(PreEscaped("@get('/ui/sessions/manage')")) {
                            "Sessions"
                        }
```

(`PreEscaped` is already imported in `page.rs`.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p kamajid --lib topbar_has_sessions_button_opening_the_manage_modal`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/kamajid/src/views/page.rs
git commit -m "feat(daemon): Sessions topbar button"
```

---

## Task 7: Modal styling asset

**Files:**
- Create: `crates/kamajid/src/assets/sessions.css`
- Modify: `crates/kamajid/src/views/page.rs`
- Test: `crates/kamajid/src/views/page.rs` (`#[cfg(test)] mod tests`)

The fragment reuses shared modal chrome (`modal`, `modal-head/body/foot`, `btn`,
`check`) from `modal.css`; this adds only the session-list-specific rules. Assets
are embedded via `rust-embed` (`src/assets/`), so a new file is picked up with no
route change.

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `page.rs`:

```rust
#[test]
fn page_links_sessions_css() {
    let p = project(1, "acme");
    let html = page(&p, std::slice::from_ref(&p), &empty_board()).into_string();
    assert!(
        html.contains(r#"href="/assets/sessions.css""#),
        "sessions css link:\n{html}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p kamajid --lib page_links_sessions_css`
Expected: FAIL — link not present.

- [ ] **Step 3: Write minimal implementation**

Create `crates/kamajid/src/assets/sessions.css`:

```css
/* Session-cleanup modal: the checkbox list and per-session kind tags.
   Chrome (modal, modal-head/body/foot, btn, check) comes from modal.css. */

.sess-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-height: 50vh;
  overflow-y: auto;
}

.sess-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 10px;
  border: 1px solid var(--border, #2a2a33);
  border-radius: 8px;
}

.sess-row:hover {
  background: var(--surface-2, #1c1c22);
}

.sess-name {
  font-family: var(--mono, ui-monospace, monospace);
  font-size: 13px;
}

.sess-tag {
  margin-left: auto;
  font-size: 12px;
  color: var(--muted, #9aa0aa);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 55%;
}

.sess-tag-main {
  color: var(--warn, #e6a23c);
}

/* Main/project workspaces are heavier to kill — tint the whole row. */
.sess-warn {
  border-color: var(--warn, #e6a23c);
}

.sess-confirm-text {
  font-size: 13px;
  color: var(--text, #e6e6ea);
}

.sess-foot-row {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
}

.sess-empty {
  color: var(--muted, #9aa0aa);
  padding: 8px 2px;
}
```

Link it in `crates/kamajid/src/views/page.rs` head, after the `modal.css` link:

```rust
                link rel="stylesheet" href="/assets/sessions.css";
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p kamajid --lib page_links_sessions_css`
Expected: PASS.

Also verify the asset is served:

Run: `cargo test -p kamajid --lib assets`
Expected: PASS (the existing asset tests still pass; the new file is embedded).

- [ ] **Step 5: Commit**

```bash
git add crates/kamajid/src/assets/sessions.css crates/kamajid/src/views/page.rs
git commit -m "feat(daemon): sessions modal styling"
```

---

## Task 8: Whole-branch verification

**Files:** none (verification only)

- [ ] **Step 1: Format**

Run: `cargo fmt --all`
Expected: no diff (or commit the formatting).

- [ ] **Step 2: Lint**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 3: Full test suite**

Run: `cargo test --workspace`
Expected: PASS (all existing + new tests).

- [ ] **Step 4: Commit any fmt/clippy fixes**

```bash
git add -A
git commit -m "chore: fmt + clippy for session cleanup modal"
```

(Skip if Steps 1–2 produced no changes.)

---

## Self-Review

**Spec coverage:**
- "All kamaji sessions listed (ticket/main/orphan)" → Task 1 `classify_sessions` + Task 3 rows.
- "Main flagged as risky, selectable" → Task 3 `sess-warn` + ⚠; Task 7 styling.
- "Smart per-session teardown" → Task 5 (ticket → `cleanup_ticket`; orphan/main → driver `terminate`).
- "In-modal confirm" → Task 3 confirm-bar footer swap.
- "Trigger button in topbar" → Task 6.
- "zellij unreachable → note" → Task 3 `unreachable_zellij_shows_a_note` + Task 4 `reachable` flag.
- "Empty/missing names → 400" → Task 5 `delete_sessions_rejects_empty_names`.
- "Per-session failure non-fatal; `{deleted, failed}`" → Task 5 handler + tests.
- "Race: already gone → success" → Task 5 `delete_already_gone_…` + `None` arm.
- "Server re-classifies; non-kamaji rejected" → Task 5 handler + `delete_non_kamaji_name_is_reported_failed`.
- "Emit SessionExited per ticket-linked success" → Task 5 + `delete_ticket_session_full_teardown_and_event`.
- "Unit test classify; integration test delete; view render test" → Tasks 1, 3, 4, 5.

**Placeholder scan:** none — every code/test step shows full content.

**Type consistency:** `classify_sessions(&Db, &str) -> Result<Vec<SessionEntry>>`,
`SessionKind::{Ticket{id,title,status}, Main, Orphan}`, `SessionEntry{name,kind}`,
`sessions_modal(&[SessionEntry], bool) -> Markup`, `FakeSessionDriver::with_sessions(&str)`,
`SessionDriver::list_sessions(&self) -> Option<String>`, `DeleteSessions{names: Vec<String>}`,
response `{deleted, failed}` — all used consistently across Tasks 1–7.

**Note on SessionExited for orphan/main:** deliberately NOT emitted — those
sessions have no board card to update; emitting requires a `ticket_id` they don't
have. Only ticket-linked deletes emit `SessionExited`. This is intentional, not a
gap.
