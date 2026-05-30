# Phase 1a — Core Extraction for the Daemon (events + poll) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prepare `kamaji-core` to back the upcoming `kamajid` daemon by (1) making the domain models JSON-serializable, (2) adding a canonical `Event` type, and (3) extracting the auto-review poll loop out of the TUI's `Engine` into a reusable `kamaji-core::poll::PollLoop` — all with **zero TUI behavior change**.

**Architecture:** This is the first of two plans for Phase 1 (spec: `docs/superpowers/specs/2026-05-30-phase-1-kamajid-design.md`). Plan 1a is the behavior-critical core refactor; Plan 1b (separate) builds the daemon crate on top. The poll loop's three in-memory state maps (`last_level`, `auto_review_ids`, `scrape_hash`) move from `Engine` into a `PollLoop` struct in `kamaji-core`; `Engine` holds a `PollLoop` and delegates to it, preserving identical behavior. The TUI keeps polling exactly as today.

**Tech Stack:** Rust 2021, `serde`/`serde_json` (already core deps for serde; `chrono` added for event timestamps), the existing `kamaji-core` modules (`detect`, `db`, `config`, `models`, `zellij`).

**Baseline:** On `main` at the start of this plan, `cargo test --all-targets --all-features` reports **152 (kamaji) + 83 (kamaji-core) = 235** passing. Every task must keep the total at 235-or-more (new tests only add).

**Repo conventions (from `CLAUDE.md`):** all work on a branch in a worktree (the executing skill sets this up), never on `main`. Commit style mirrors history (`feat(core): …`, `refactor(core): …`). Ship at the end with `gh pr create --fill --base main` → `gh pr merge --squash --auto --delete-branch` (the `--delete-branch` step errors from inside a worktree but the merge still lands — verify with `gh pr view` and clean up manually).

---

## Verification commands (run at every checkpoint)

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

These mirror `.github/workflows/ci.yml`. To see per-binary test counts:

```bash
cargo test --all-targets --all-features 2>&1 | grep -E '(test result:|running [0-9]+ tests)'
```

---

## File Structure (after this plan)

```
crates/kamaji-core/src/
├── lib.rs            +pub mod events;  +pub mod poll;
├── models.rs         MODIFIED — serde derives on Agent, Status, Project, Ticket
├── events.rs         NEW — Event enum + sse_name()
├── poll.rs           NEW — PollLoop (extracted from Engine), + the detection unit tests
└── …unchanged…
crates/kamaji-core/Cargo.toml   +chrono
crates/kamaji/src/
├── engine.rs         MODIFIED — holds a PollLoop; detect_tick/apply_move/reconcile/forget delegate
└── main.rs           MODIFIED — ui::render reads engine.poll.levels()
```

---

## Task 1: Make domain models JSON-serializable

The daemon's HTTP API and the `Event` type serialize `Ticket`, `Project`, `Agent`, and `Status`. Derive `serde` on them. The enums must serialize to their existing string forms (`"claude"`, `"in_progress"`) so the wire format matches the DB/`as_str()` representation — `rename_all = "snake_case"` produces exactly that.

**Files:**
- Modify: `crates/kamaji-core/src/models.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/kamaji-core/src/models.rs`:

```rust
#[test]
fn agent_serializes_to_snake_case_string() {
    assert_eq!(serde_json::to_string(&Agent::Claude).unwrap(), "\"claude\"");
    assert_eq!(serde_json::to_string(&Agent::Codex).unwrap(), "\"codex\"");
    assert_eq!(
        serde_json::from_str::<Agent>("\"copilot\"").unwrap(),
        Agent::Copilot
    );
}

#[test]
fn status_serializes_to_db_string_form() {
    assert_eq!(
        serde_json::to_string(&Status::InProgress).unwrap(),
        "\"in_progress\""
    );
    assert_eq!(
        serde_json::from_str::<Status>("\"review\"").unwrap(),
        Status::Review
    );
    // The serde form must equal the existing DB/as_str() form for every variant.
    for s in Status::all() {
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            format!("\"{}\"", s.as_str())
        );
    }
}

#[test]
fn ticket_serializes_with_expected_field_names() {
    let t = Ticket {
        id: 7,
        project_id: 1,
        title: "Add login".into(),
        description: "desc".into(),
        initial_prompt: Some("do it".into()),
        agent: Agent::Claude,
        status: Status::InProgress,
        position: 0,
        session_name: Some("kamaji-7-add-login".into()),
        worktree_path: Some(std::path::PathBuf::from("/wt")),
        branch: Some("kamaji-7-add-login".into()),
        auto_reviewed: false,
        instrumented: true,
        created_at: "2026-05-30T00:00:00Z".into(),
        updated_at: "2026-05-30T00:00:00Z".into(),
    };
    let v: serde_json::Value = serde_json::to_value(&t).unwrap();
    assert_eq!(v["id"], 7);
    assert_eq!(v["agent"], "claude");
    assert_eq!(v["status"], "in_progress");
    assert_eq!(v["session_name"], "kamaji-7-add-login");
    assert_eq!(v["worktree_path"], "/wt");
    assert_eq!(v["instrumented"], true);
}
```

This needs `serde_json`, which is **not yet** a `kamaji-core` dependency. Add it as a dev-dependency for now (the daemon will add it as a runtime dep in Plan 1b; core only needs it for these tests):

In `crates/kamaji-core/Cargo.toml`, under `[dev-dependencies]`:

```toml
serde_json = "1"
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p kamaji-core models::tests::agent_serializes_to_snake_case_string`
Expected: FAIL — `Agent` doesn't implement `Serialize`/`Deserialize` (compile error).

- [ ] **Step 3: Add the derives**

In `crates/kamaji-core/src/models.rs`, change the four type definitions. Add `use serde::{Deserialize, Serialize};` near the top (after the existing `use std::...` lines).

For `Agent` (currently `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
```

For `Status` (same change):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
```

For `Project` (currently `#[derive(Debug, Clone)]`):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
```

For `Ticket` (currently `#[derive(Debug, Clone)]`):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticket {
```

Also remove the now-obsolete `#[allow(dead_code)]` attributes on `Project::created_at` and on `Ticket`'s `project_id`/`position`/`created_at`/`updated_at` fields — serialization reads them, so they are no longer dead. (If clippy still flags any as dead after this, restore that specific `#[allow(dead_code)]` — but serialization should make them all live.)

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p kamaji-core models::tests`
Expected: PASS — all model tests including the three new ones.

- [ ] **Step 5: Full verification**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E 'test result:'
```

Expected: all green; total still ≥ 235 (now 235 + 3 new core tests = 238).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(core): derive serde on domain models

Agent/Status serialize to their existing snake_case string form
(matching as_str()/the DB); Project/Ticket gain Serialize+Deserialize.
Needed by the upcoming kamajid HTTP API and the Event type. Adds
serde_json as a core dev-dependency for the serialization tests."
```

---

## Task 2: Add the `kamaji-core::events` module

The canonical state-change vocabulary shared by the daemon (emits) and clients (consume). Phase 1b's SSE handler turns these into `event:`/`data:` frames; this task defines the type and its SSE event-name mapping.

**Files:**
- Create: `crates/kamaji-core/src/events.rs`
- Modify: `crates/kamaji-core/src/lib.rs` (declare `pub mod events;`)

- [ ] **Step 1: Write the failing test**

Create `crates/kamaji-core/src/events.rs` with ONLY the test module first (so the test fails to compile against the not-yet-written type — TDD):

```rust
//! Canonical state-change events shared by the daemon (which emits them) and
//! clients (which consume them over SSE). The enum is the in-process source of
//! truth; the daemon frames each event as an SSE record using `sse_name()` for
//! the `event:` line and the variant's payload for the `data:` line.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Status;

    #[test]
    fn ticket_moved_serializes_with_tag_and_data() {
        let ev = Event::TicketMoved {
            id: 5,
            from: Status::InProgress,
            to: Status::Review,
            at: "2026-05-30T10:23:45Z".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "ticket_moved");
        assert_eq!(v["data"]["id"], 5);
        assert_eq!(v["data"]["from"], "in_progress");
        assert_eq!(v["data"]["to"], "review");
    }

    #[test]
    fn sse_names_are_dotted_lowercase() {
        assert_eq!(
            Event::TicketDeleted { id: 1 }.sse_name(),
            "ticket.deleted"
        );
        assert_eq!(
            Event::SessionIdle { ticket_id: 2 }.sse_name(),
            "session.idle"
        );
        assert_eq!(
            Event::SessionExited {
                ticket_id: 3,
                session_name: "kamaji-3-x".into()
            }
            .sse_name(),
            "session.exited"
        );
    }

    #[test]
    fn round_trips_through_json() {
        let ev = Event::SessionStarted {
            ticket_id: 9,
            session_name: "kamaji-9-y".into(),
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&s).unwrap();
        assert_eq!(back.sse_name(), "session.started");
    }
}
```

- [ ] **Step 2: Declare the module so the test compiles**

In `crates/kamaji-core/src/lib.rs`, add `pub mod events;` in alphabetical position (between `pub mod detect;` and `pub mod git;`):

```rust
pub mod detect;
pub mod events;
pub mod git;
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p kamaji-core events::tests::sse_names_are_dotted_lowercase`
Expected: FAIL — `Event` is undefined (compile error).

- [ ] **Step 4: Write the `Event` type and `sse_name`**

Prepend to `crates/kamaji-core/src/events.rs` (above the `#[cfg(test)]` module, keeping the module doc comment at the very top):

```rust
use crate::models::{Status, Ticket};
use serde::{Deserialize, Serialize};

/// A state change worth broadcasting to connected clients. Payloads are
/// minimal — identifiers plus the fields that changed; a client that needs the
/// full object re-fetches it. `at` fields are RFC 3339 timestamps.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Event {
    TicketCreated(Ticket),
    TicketUpdated(Ticket),
    TicketMoved {
        id: i64,
        from: Status,
        to: Status,
        at: String,
    },
    TicketDeleted {
        id: i64,
    },
    SessionStarted {
        ticket_id: i64,
        session_name: String,
    },
    SessionIdle {
        ticket_id: i64,
    },
    SessionExited {
        ticket_id: i64,
        session_name: String,
    },
}

impl Event {
    /// The SSE `event:` name — dotted lowercase, e.g. `ticket.moved`. Browser
    /// clients filter on these via `addEventListener("ticket.moved", …)`.
    pub fn sse_name(&self) -> &'static str {
        match self {
            Event::TicketCreated(_) => "ticket.created",
            Event::TicketUpdated(_) => "ticket.updated",
            Event::TicketMoved { .. } => "ticket.moved",
            Event::TicketDeleted { .. } => "ticket.deleted",
            Event::SessionStarted { .. } => "session.started",
            Event::SessionIdle { .. } => "session.idle",
            Event::SessionExited { .. } => "session.exited",
        }
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p kamaji-core events::tests`
Expected: PASS — all three event tests.

- [ ] **Step 6: Full verification**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E 'test result:'
```

Expected: all green; total now 238 + 3 = 241.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(core): add events module with the Event enum

Canonical state-change vocabulary (ticket.created/updated/moved/
deleted, session.started/idle/exited) shared by the upcoming daemon
and its clients. Tagged-JSON serialization plus an sse_name() helper
for the SSE event: line. Phase 1a step 2."
```

---

## Task 3: Extract `PollLoop` into `kamaji-core::poll`

Move the auto-review detection out of the TUI's `Engine` into a self-contained, reusable runner. `PollLoop` owns the three in-memory state maps and exposes `tick` (gather levels + decide moves) and `apply` (decide moves from pre-supplied levels — the seam today's `detect_tick_with` provides). It returns `Event`s instead of mutating UI state. The pure detection unit tests move here (they belong with the detection logic); Engine-integration tests stay in `engine.rs` (Task 4).

**Files:**
- Create: `crates/kamaji-core/src/poll.rs`
- Modify: `crates/kamaji-core/src/lib.rs` (declare `pub mod poll;`)
- Modify: `crates/kamaji-core/Cargo.toml` (add `chrono`)

- [ ] **Step 1: Add `chrono` to `kamaji-core`**

In `crates/kamaji-core/Cargo.toml`, under `[dependencies]`:

```toml
chrono = { version = "0.4", default-features = false, features = ["clock"] }
```

(`clock` enables `Utc::now()`. `default-features = false` keeps it lean; we don't need `serde` on chrono since `at` is a `String`.)

- [ ] **Step 2: Declare the module**

In `crates/kamaji-core/src/lib.rs`, add `pub mod poll;` in alphabetical position (between `pub mod paths;` and `pub mod session;`):

```rust
pub mod paths;
pub mod poll;
pub mod session;
```

- [ ] **Step 3: Write `poll.rs` with the `PollLoop` implementation**

Create `crates/kamaji-core/src/poll.rs`. This is a faithful port of `Engine`'s `gather_levels` + `detect_tick_with` (currently `crates/kamaji/src/engine.rs` lines ~289–391), with the UI side effects replaced by returned `Event`s:

```rust
//! The auto-review poll loop: detect when an agent session goes idle (or
//! resumes) and move its ticket between In Progress and Needs attention
//! (Review). Extracted from the TUI's `Engine` so the daemon and the TUI share
//! one canonical implementation. Pure of UI concerns — it reads tickets, writes
//! status to the DB, and returns the [`Event`]s that fired; the caller decides
//! how to surface them.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;

use crate::config::Config;
use crate::db::Db;
use crate::detect::{self, SignalLevel};
use crate::events::Event;
use crate::models::{Agent, Status, Ticket};
use crate::zellij;

/// Per-session detection state, held across ticks. Re-baselined on restart;
/// auto-review provenance is rehydrated from the persisted `auto_reviewed`
/// column via [`PollLoop::rehydrate`].
#[derive(Default)]
pub struct PollLoop {
    /// Last observed signal level per ticket id.
    last_level: HashMap<i64, SignalLevel>,
    /// Tickets kamaji auto-moved to Review (provenance gate for the move back).
    auto_review_ids: HashSet<i64>,
    /// Per-ticket scrape screen hash for the scrape detector's stability guard.
    scrape_hash: HashMap<i64, Option<u64>>,
}

impl PollLoop {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild auto-review provenance from the persisted column (call after a
    /// ticket reload / on startup) so the move back from Review survives a
    /// restart that wiped in-memory state.
    pub fn rehydrate(&mut self, tickets: &[Ticket]) {
        self.auto_review_ids = tickets
            .iter()
            .filter(|t| t.auto_reviewed)
            .map(|t| t.id)
            .collect();
    }

    /// Last observed signal level per ticket — read by the TUI to colour the
    /// activity bullet.
    pub fn levels(&self) -> &HashMap<i64, SignalLevel> {
        &self.last_level
    }

    /// Whether kamaji currently considers this ticket auto-moved to Review.
    pub fn is_auto_reviewed(&self, id: i64) -> bool {
        self.auto_review_ids.contains(&id)
    }

    /// Drop a ticket's auto-review provenance (a manual move overrides it, so a
    /// human-placed Review card is not dragged back when its agent resumes).
    pub fn clear_auto_review(&mut self, id: i64) {
        self.auto_review_ids.remove(&id);
    }

    /// Forget all in-memory detection state for a ticket (on teardown/vanish).
    pub fn forget_ticket(&mut self, id: i64) {
        self.last_level.remove(&id);
        self.auto_review_ids.remove(&id);
        self.scrape_hash.remove(&id);
    }

    /// One full detection pass: gather the current signal level for every live,
    /// in-progress/review ticket, then apply move decisions. Returns the events
    /// that fired (zero or more `TicketMoved`, each Review move paired with a
    /// `SessionIdle`).
    pub fn tick(
        &mut self,
        tickets: &[Ticket],
        db: &Db,
        config: &Config,
        state_dir: &Path,
    ) -> Result<Vec<Event>> {
        let levels = self.gather_levels(tickets, config, state_dir);
        self.apply(tickets, &levels, db)
    }

    /// Apply move decisions given already-gathered levels. Split from the IO so
    /// it can be unit-tested with crafted levels (the seam the TUI's tests use).
    pub fn apply(
        &mut self,
        tickets: &[Ticket],
        levels: &HashMap<i64, SignalLevel>,
        db: &Db,
    ) -> Result<Vec<Event>> {
        let mut events = Vec::new();
        for (&id, &level) in levels {
            let Some(status) = tickets.iter().find(|t| t.id == id).map(|t| t.status) else {
                continue;
            };
            let last = self.last_level.get(&id).copied();
            let was_auto = self.auto_review_ids.contains(&id);
            if let Some(target) = detect::decide(last, level, status, was_auto) {
                db.set_ticket_status(id, target)?;
                match target {
                    Status::Review => {
                        db.set_ticket_auto_reviewed(id, true)?;
                        self.auto_review_ids.insert(id);
                    }
                    Status::InProgress => {
                        db.set_ticket_auto_reviewed(id, false)?;
                        self.auto_review_ids.remove(&id);
                    }
                    _ => {}
                }
                events.push(Event::TicketMoved {
                    id,
                    from: status,
                    to: target,
                    at: chrono::Utc::now().to_rfc3339(),
                });
                if target == Status::Review {
                    events.push(Event::SessionIdle { ticket_id: id });
                }
            }
            if level != SignalLevel::Unknown {
                self.last_level.insert(id, level);
            }
        }
        Ok(events)
    }

    /// Read the current signal level for every live, in-progress/review ticket.
    fn gather_levels(
        &mut self,
        tickets: &[Ticket],
        config: &Config,
        state_dir: &Path,
    ) -> HashMap<i64, SignalLevel> {
        // Snapshot the live (in-progress/review + session) tickets first.
        let live: Vec<(i64, Agent, String, bool)> = tickets
            .iter()
            .filter(|t| matches!(t.status, Status::InProgress | Status::Review))
            .filter_map(|t| {
                t.session_name
                    .clone()
                    .map(|s| (t.id, t.agent, s, t.instrumented))
            })
            .collect();

        // One session listing per tick, used to drop signals from exited
        // (resurrectable) sessions whose agent is no longer running. `None`
        // (couldn't ask) leaves detection untouched.
        let sessions = zellij::list_sessions();

        let mut out = HashMap::new();
        for (id, agent, session, instrumented) in live {
            if let Some(list) = &sessions {
                if zellij::session_exited(list, &session) {
                    out.insert(id, SignalLevel::Unknown);
                    continue;
                }
            }
            let level = match agent {
                Agent::Claude => {
                    if instrumented {
                        detect::marker_level(&detect::marker_path(state_dir, &session))
                    } else {
                        SignalLevel::Unknown
                    }
                }
                Agent::Codex | Agent::Copilot => {
                    let patterns: Vec<String> = config.auto_review_patterns(agent).to_vec();
                    if patterns.is_empty() {
                        continue; // detector disabled for this agent
                    }
                    let screen = zellij::dump_screen(&session);
                    let hash = self.scrape_hash.entry(id).or_insert(None);
                    detect::scrape_level(screen.as_deref(), &patterns, hash)
                }
            };
            out.insert(id, level);
        }
        out
    }
}
```

- [ ] **Step 4: Add the ported detection unit tests**

Append this `#[cfg(test)]` module to `crates/kamaji-core/src/poll.rs`. These are the pure detection-decision tests, moved from `engine.rs` and rewritten against `PollLoop::apply` directly (no `Engine`, no `App`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Agent;
    use std::path::PathBuf;

    /// An in-progress ticket with a recorded session, in a fresh in-memory DB.
    fn setup() -> (Db, Vec<Ticket>, i64) {
        let db = Db::open_in_memory().unwrap();
        let p = db.create_project("p", &PathBuf::from("/tmp/p"), None).unwrap();
        let t = db.create_ticket(p.id, "t", "", None, Agent::Claude).unwrap();
        db.set_ticket_session(t.id, "kamaji-1-t", "/wt", "kamaji-1-t").unwrap();
        db.set_ticket_status(t.id, Status::InProgress).unwrap();
        let tickets = db.list_tickets(p.id).unwrap();
        (db, tickets, t.id)
    }

    fn levels(id: i64, level: SignalLevel) -> HashMap<i64, SignalLevel> {
        let mut m = HashMap::new();
        m.insert(id, level);
        m
    }

    #[test]
    fn idle_after_active_moves_in_progress_to_review() {
        let (db, tickets, id) = setup();
        let mut p = PollLoop::new();
        p.apply(&tickets, &levels(id, SignalLevel::Active), &db).unwrap();
        let events = p.apply(&tickets, &levels(id, SignalLevel::Idle), &db).unwrap();
        assert_eq!(db.get_ticket(id).unwrap().unwrap().status, Status::Review);
        assert!(p.is_auto_reviewed(id));
        // A move to Review emits TicketMoved + SessionIdle.
        assert!(events
            .iter()
            .any(|e| matches!(e, Event::TicketMoved { to: Status::Review, .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, Event::SessionIdle { ticket_id } if *ticket_id == id)));
    }

    #[test]
    fn resumed_auto_reviewed_card_returns_to_in_progress() {
        let (db, tickets, id) = setup();
        let mut p = PollLoop::new();
        p.apply(&tickets, &levels(id, SignalLevel::Active), &db).unwrap();
        p.apply(&tickets, &levels(id, SignalLevel::Idle), &db).unwrap();
        // Reload tickets so status reflects the Review move before resuming.
        let tickets = db.list_tickets(tickets[0].project_id).unwrap();
        let events = p.apply(&tickets, &levels(id, SignalLevel::Active), &db).unwrap();
        assert_eq!(
            db.get_ticket(id).unwrap().unwrap().status,
            Status::InProgress
        );
        assert!(!p.is_auto_reviewed(id));
        assert!(events
            .iter()
            .any(|e| matches!(e, Event::TicketMoved { to: Status::InProgress, .. })));
    }

    #[test]
    fn move_back_survives_lost_in_memory_provenance() {
        let (db, tickets, id) = setup();
        let mut p = PollLoop::new();
        p.apply(&tickets, &levels(id, SignalLevel::Active), &db).unwrap();
        p.apply(&tickets, &levels(id, SignalLevel::Idle), &db).unwrap();
        assert_eq!(db.get_ticket(id).unwrap().unwrap().status, Status::Review);
        assert!(db.get_ticket(id).unwrap().unwrap().auto_reviewed);

        // Simulate a restart: fresh PollLoop, rehydrate provenance from the DB.
        let tickets = db.list_tickets(tickets[0].project_id).unwrap();
        let mut p = PollLoop::new();
        p.rehydrate(&tickets);

        // Resume the agent: re-baseline as Idle, then Active => back to In Progress.
        p.apply(&tickets, &levels(id, SignalLevel::Idle), &db).unwrap();
        p.apply(&tickets, &levels(id, SignalLevel::Active), &db).unwrap();
        assert_eq!(
            db.get_ticket(id).unwrap().unwrap().status,
            Status::InProgress
        );
    }

    #[test]
    fn unknown_level_never_moves_or_baselines() {
        let (db, tickets, id) = setup();
        let mut p = PollLoop::new();
        p.apply(&tickets, &levels(id, SignalLevel::Active), &db).unwrap();
        let events = p.apply(&tickets, &levels(id, SignalLevel::Unknown), &db).unwrap();
        assert!(events.is_empty());
        // Unknown must not overwrite the Active baseline.
        assert_eq!(p.levels().get(&id), Some(&SignalLevel::Active));
        assert_eq!(
            db.get_ticket(id).unwrap().unwrap().status,
            Status::InProgress
        );
    }

    #[test]
    fn forget_ticket_clears_all_state() {
        let (db, tickets, id) = setup();
        let mut p = PollLoop::new();
        p.apply(&tickets, &levels(id, SignalLevel::Active), &db).unwrap();
        assert!(p.levels().contains_key(&id));
        p.forget_ticket(id);
        assert!(!p.levels().contains_key(&id));
        assert!(!p.is_auto_reviewed(id));
    }
}
```

- [ ] **Step 5: Run the new tests**

Run: `cargo test -p kamaji-core poll::tests`
Expected: PASS — all five `PollLoop` tests. (`Db::open_in_memory` is already `pub` from Phase 0.)

- [ ] **Step 6: Full verification**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E 'test result:'
```

Expected: all green. NOTE: `engine.rs` still has its own `gather_levels`/`detect_tick_with` and the three maps — that's fine, this task only ADDS `poll.rs`. The duplication is removed in Task 4. Total now 241 + 5 = 246.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(core): extract PollLoop auto-review runner

Faithful port of Engine::gather_levels + detect_tick_with into a
reusable kamaji-core::poll::PollLoop that owns the detection state
maps and returns Events instead of mutating UI state. Engine still
has its own copy; Task 4 rewires it to delegate. Adds chrono for
event timestamps. Phase 1a step 3."
```

---

## Task 4: Rewire `Engine` to delegate to `PollLoop` (zero behavior change)

Replace `Engine`'s three detection-state fields and its `gather_levels`/`detect_tick_with` methods with a single `poll: PollLoop`. Every site that touched the maps now goes through `PollLoop`. The TUI's observable behavior — auto-moves, toasts, bullet colours, cleanup — must be **byte-identical**. This is the riskiest task; the gate is "all pre-existing tests still pass."

**Files:**
- Modify: `crates/kamaji/src/engine.rs`
- Modify: `crates/kamaji/src/main.rs`

- [ ] **Step 1: Swap the struct fields**

In `crates/kamaji/src/engine.rs`, change the `Engine` struct. Remove the three map fields and add `poll`:

```rust
pub struct Engine {
    pub db: Db,
    pub config: Config,
    pub app: App,
    /// Auto-review detection runner (owns the per-session detection state).
    pub poll: kamaji_core::poll::PollLoop,
    /// Where per-session idle markers live.
    pub state_dir: std::path::PathBuf,
    /// Where the theme picker persists the chosen theme. Defaults to the real
    /// config path; tests override it.
    pub config_path: std::path::PathBuf,
}
```

Update `Engine::new` accordingly — replace the three `*: HashMap::new()/HashSet::new()` initializers with `poll: kamaji_core::poll::PollLoop::new(),`:

```rust
impl Engine {
    pub fn new(db: Db, config: Config, app: App) -> Self {
        Engine {
            db,
            config,
            app,
            poll: kamaji_core::poll::PollLoop::new(),
            state_dir: detect::default_state_dir(),
            config_path: kamaji_core::config::config_path().unwrap_or_default(),
        }
    }
```

Remove the now-unused imports at the top of `engine.rs`: the `HashMap`/`HashSet` import line (`use std::collections::{HashMap, HashSet};`) — but ONLY if nothing else in the file uses them after this task. (After deleting `gather_levels`/`detect_tick_with` they're likely unused; the compiler will tell you. If something else uses `HashMap`, keep that part.) Also the `SignalLevel` import may become unused in non-test code — leave it if the test module still references it (it does).

- [ ] **Step 2: Replace `reload`'s provenance rebuild**

In `Engine::reload` (around line 79–93), the block that rebuilds `auto_review_ids`:

```rust
        // Rehydrate the auto-review provenance cache from the persisted column so
        // it survives restarts (the move back from Needs attention depends on it).
        self.auto_review_ids = self
            .app
            .tickets
            .iter()
            .filter(|t| t.auto_reviewed)
            .map(|t| t.id)
            .collect();
```

becomes:

```rust
        // Rehydrate the auto-review provenance cache from the persisted column so
        // it survives restarts (the move back from Needs attention depends on it).
        self.poll.rehydrate(&self.app.tickets);
```

- [ ] **Step 3: Delete `gather_levels` and `detect_tick_with`; rewrite `detect_tick`**

Delete the entire `gather_levels` method (lines ~333–385) and the entire `detect_tick_with` method (lines ~287–331) from `engine.rs` — that logic now lives in `PollLoop`. Replace `detect_tick` (lines ~387–391) and add a private event-handler:

```rust
    /// One detection pass: delegate to the poll runner, then surface any moves
    /// as the same informational toasts the TUI showed before, and reload.
    pub fn detect_tick(&mut self) -> Result<()> {
        let events = self
            .poll
            .tick(&self.app.tickets, &self.db, &self.config, &self.state_dir)?;
        self.handle_poll_events(&events)
    }

    /// Translate poll events into UI toasts and reload if anything moved. Shared
    /// by `detect_tick` and the test-only `detect_tick_with` seam.
    fn handle_poll_events(&mut self, events: &[kamaji_core::events::Event]) -> Result<()> {
        use kamaji_core::events::Event;
        let mut changed = false;
        for ev in events {
            if let Event::TicketMoved { id, to, .. } = ev {
                match to {
                    Status::Review => self
                        .app
                        .set_info(format!("#{id} → Needs attention (agent idle)")),
                    Status::InProgress => self
                        .app
                        .set_info(format!("#{id} → In Progress (agent active)")),
                    _ => {}
                }
                changed = true;
            }
        }
        if changed {
            self.reload()?;
        }
        Ok(())
    }

    /// Test-only seam: apply move decisions from crafted levels (mirrors the old
    /// `detect_tick_with`) so detection-integration tests need no real zellij.
    #[cfg(test)]
    fn detect_tick_with(
        &mut self,
        levels: &std::collections::HashMap<i64, kamaji_core::detect::SignalLevel>,
    ) -> Result<()> {
        let events = self.poll.apply(&self.app.tickets, levels, &self.db)?;
        self.handle_poll_events(&events)
    }
```

- [ ] **Step 4: Replace map access in `apply_move`, `cleanup_ticket`, `reconcile`, `forget_ticket_state`**

In `apply_move` (line ~209), replace:

```rust
        self.auto_review_ids.remove(&ticket.id);
```

with:

```rust
        self.poll.clear_auto_review(ticket.id);
```

In `forget_ticket_state` (lines ~281–285), the whole method body becomes a delegation. Replace:

```rust
    fn forget_ticket_state(&mut self, id: i64) {
        self.last_level.remove(&id);
        self.auto_review_ids.remove(&id);
        self.scrape_hash.remove(&id);
    }
```

with:

```rust
    fn forget_ticket_state(&mut self, id: i64) {
        self.poll.forget_ticket(id);
    }
```

(`cleanup_ticket` and `reconcile` call `self.forget_ticket_state(id)`, so they need no further change.)

- [ ] **Step 5: Update `main.rs`'s render call**

In `crates/kamaji/src/main.rs` (line ~147), the UI reads the signal levels. Replace:

```rust
        terminal.draw(|frame| ui::render(frame, &engine.app, &engine.last_level))?;
```

with:

```rust
        terminal.draw(|frame| ui::render(frame, &engine.app, engine.poll.levels()))?;
```

- [ ] **Step 6: Migrate the engine tests that poked the maps**

Several tests in `engine.rs` directly touched `e.auto_review_ids` / `e.last_level`. Update each:

- `idle_after_active_moves_in_progress_to_review` (line ~996): replace `assert!(e.auto_review_ids.contains(&id));` with `assert!(e.poll.is_auto_reviewed(id));`.
- `resumed_auto_reviewed_card_returns_to_in_progress` (line ~1017): replace `assert!(!e.auto_review_ids.contains(&id));` with `assert!(!e.poll.is_auto_reviewed(id));`.
- `move_back_survives_lost_in_memory_provenance` (lines ~1037–1038): replace the two lines

  ```rust
          e.auto_review_ids.clear();
          e.last_level.clear();
  ```

  with a fresh poll runner (simulating the restart that wipes in-memory state):

  ```rust
          e.poll = kamaji_core::poll::PollLoop::new();
  ```

- `cleanup_removes_marker_and_state` (lines ~1092–1099): replace the seeding

  ```rust
          e.auto_review_ids.insert(t.id);
          e.last_level.insert(t.id, SignalLevel::Idle);
  ```

  with seeding through the real detection path (an Active observation populates `last_level`, and we mark provenance via the DB + rehydrate):

  ```rust
          e.db.set_ticket_auto_reviewed(t.id, true).unwrap();
          e.reload().unwrap();
          e.detect_tick_with(&levels(t.id, SignalLevel::Active)).unwrap();
  ```

  and replace the two assertions

  ```rust
          assert!(!e.auto_review_ids.contains(&t.id));
          assert!(!e.last_level.contains_key(&t.id));
  ```

  with:

  ```rust
          assert!(!e.poll.is_auto_reviewed(t.id));
          assert!(!e.poll.levels().contains_key(&t.id));
  ```

- `non_instrumented_claude_signal_is_ignored` (lines ~1174–1177): replace

  ```rust
          assert!(!matches!(
              e.last_level.get(&t.id),
              Some(SignalLevel::Active) | Some(SignalLevel::Idle)
          ));
  ```

  with:

  ```rust
          assert!(!matches!(
              e.poll.levels().get(&t.id),
              Some(SignalLevel::Active) | Some(SignalLevel::Idle)
          ));
  ```

The pure-decision tests that previously lived here (`idle_after_active…` minus its toast assertion, `resumed…`, `move_back…`) now ALSO exist in `poll.rs`. Keep the `engine.rs` versions too — here they additionally verify the Engine integration (toast in `idle_after_active…`, the `move_ticket`-interleaved cases). They are not redundant: `poll.rs` tests the runner, `engine.rs` tests the Engine wiring. Do NOT delete them.

- [ ] **Step 7: Build and run the full suite**

```bash
cargo build --workspace
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E '(test result:|running [0-9]+ tests)'
```

Expected: all green. The `kamaji` binary test count is unchanged from the start of the plan (152) — no engine tests were deleted, only rewired. The `kamaji-core` count is 83 + 3 (Task 1) + 3 (Task 2) + 5 (Task 3) = 94. Total ≥ 246.

- [ ] **Step 8: Manual smoke (behavior unchanged)**

```bash
cargo build --release
XDG_DATA_HOME=$(mktemp -d) XDG_CONFIG_HOME=$(mktemp -d) timeout 3s ./target/release/kamaji || true
```

Expected: the project picker launches and the process exits on the timeout (or a non-TTY ratatui error in a non-interactive shell) — NOT a panic or a missing-symbol error. This confirms the rewired binary still starts and wires the poll runner.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "refactor(tui): delegate auto-review detection to PollLoop

Engine drops its three detection-state maps and its gather_levels/
detect_tick_with methods in favour of a kamaji_core::poll::PollLoop.
detect_tick delegates and translates the returned events into the
same toasts; the UI reads engine.poll.levels() for bullet colours.
Behaviour is identical — all pre-existing engine tests pass, rewired
to the PollLoop accessors. Completes the Phase 1a core extraction."
```

---

## Task 5: Ship

- [ ] **Step 1: Final full verification**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features 2>&1 | grep -E 'test result:'
```

Expected: all green; `kamaji` 152, `kamaji-core` 94, total 246.

- [ ] **Step 2: Push and open the PR**

```bash
git push -u origin "$(git branch --show-current)"
gh pr create --fill --base main
```

- [ ] **Step 3: Enable squash auto-merge with branch delete**

```bash
gh pr merge --squash --auto --delete-branch
```

Per the known worktree gotcha, the post-merge local cleanup may error from inside the worktree; the merge still lands. Verify:

```bash
gh pr view --json state,mergeStateStatus -q '{state:.state,merge:.mergeStateStatus}'
```

If a merge conflict appears (something landed on `main` meanwhile), rebase or merge `origin/main`, resolve, re-verify green, and re-push. Once `MERGED`, clean up from the primary worktree at `/home/victor/dev/kamaji`:

```bash
cd /home/victor/dev/kamaji
git checkout main && git pull --ff-only
git worktree remove ../kamaji-worktrees/<branch>
git branch -d <branch>
git push origin --delete <branch> 2>/dev/null || true
git fetch --prune origin
```

---

## Self-review checklist (run before marking the plan done)

- **Spec coverage:** This plan covers spec §3's `events` module and `poll::PollLoop` extraction, and the serde prerequisite the API/SSE need. The daemon crate, HTTP routes, SSE handler, `zellij web` management, logging, and the `reconcile`/`cleanup_ticket` extractions are **Plan 1b** (explicitly out of scope here).
- **Type consistency:** `PollLoop` API used identically across Tasks 3–4: `new()`, `rehydrate(&[Ticket])`, `tick(&[Ticket], &Db, &Config, &Path)`, `apply(&[Ticket], &HashMap<i64,SignalLevel>, &Db)`, `levels() -> &HashMap<i64,SignalLevel>`, `is_auto_reviewed(i64) -> bool`, `clear_auto_review(i64)`, `forget_ticket(i64)`. `Event` variants and `sse_name()` consistent between Tasks 2 and 3.
- **No placeholders:** every code step shows complete code.

## What this plan deliberately does NOT do (→ Plan 1b)

- No `kamajid` crate, axum server, HTTP routes, or SSE handler.
- No `zellij web` management, `/attach`, logging/tracing, or `/healthz`.
- No extraction of `reconcile` or `cleanup_ticket` into core (Plan 1b extracts these when the daemon's poll task and `/done` route need them; the TUI keeps its in-place `Engine::reconcile`/`cleanup_ticket` for now).
- No `[daemon]` config section (Plan 1b).
- No new TUI behavior.
