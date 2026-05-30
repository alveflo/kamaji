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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Agent;
    use std::path::PathBuf;

    /// An in-progress ticket with a recorded session, in a fresh in-memory DB.
    fn setup() -> (Db, Vec<Ticket>, i64) {
        let db = Db::open_in_memory().unwrap();
        let p = db
            .create_project("p", &PathBuf::from("/tmp/p"), None)
            .unwrap();
        let t = db
            .create_ticket(p.id, "t", "", None, Agent::Claude)
            .unwrap();
        db.set_ticket_session(t.id, "kamaji-1-t", "/wt", "kamaji-1-t")
            .unwrap();
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
        p.apply(&tickets, &levels(id, SignalLevel::Active), &db)
            .unwrap();
        let events = p
            .apply(&tickets, &levels(id, SignalLevel::Idle), &db)
            .unwrap();
        assert_eq!(db.get_ticket(id).unwrap().unwrap().status, Status::Review);
        assert!(p.is_auto_reviewed(id));
        // A move to Review emits TicketMoved + SessionIdle.
        assert!(events.iter().any(|e| matches!(
            e,
            Event::TicketMoved {
                to: Status::Review,
                ..
            }
        )));
        assert!(events
            .iter()
            .any(|e| matches!(e, Event::SessionIdle { ticket_id } if *ticket_id == id)));
    }

    #[test]
    fn resumed_auto_reviewed_card_returns_to_in_progress() {
        let (db, tickets, id) = setup();
        let mut p = PollLoop::new();
        p.apply(&tickets, &levels(id, SignalLevel::Active), &db)
            .unwrap();
        p.apply(&tickets, &levels(id, SignalLevel::Idle), &db)
            .unwrap();
        // Reload tickets so status reflects the Review move before resuming.
        let tickets = db.list_tickets(tickets[0].project_id).unwrap();
        let events = p
            .apply(&tickets, &levels(id, SignalLevel::Active), &db)
            .unwrap();
        assert_eq!(
            db.get_ticket(id).unwrap().unwrap().status,
            Status::InProgress
        );
        assert!(!p.is_auto_reviewed(id));
        assert!(events.iter().any(|e| matches!(
            e,
            Event::TicketMoved {
                to: Status::InProgress,
                ..
            }
        )));
    }

    #[test]
    fn move_back_survives_lost_in_memory_provenance() {
        let (db, tickets, id) = setup();
        let mut p = PollLoop::new();
        p.apply(&tickets, &levels(id, SignalLevel::Active), &db)
            .unwrap();
        p.apply(&tickets, &levels(id, SignalLevel::Idle), &db)
            .unwrap();
        assert_eq!(db.get_ticket(id).unwrap().unwrap().status, Status::Review);
        assert!(db.get_ticket(id).unwrap().unwrap().auto_reviewed);

        // Simulate a restart: fresh PollLoop, rehydrate provenance from the DB.
        let tickets = db.list_tickets(tickets[0].project_id).unwrap();
        let mut p = PollLoop::new();
        p.rehydrate(&tickets);

        // Resume the agent: re-baseline as Idle, then Active => back to In Progress.
        p.apply(&tickets, &levels(id, SignalLevel::Idle), &db)
            .unwrap();
        p.apply(&tickets, &levels(id, SignalLevel::Active), &db)
            .unwrap();
        assert_eq!(
            db.get_ticket(id).unwrap().unwrap().status,
            Status::InProgress
        );
    }

    #[test]
    fn unknown_level_never_moves_or_baselines() {
        let (db, tickets, id) = setup();
        let mut p = PollLoop::new();
        p.apply(&tickets, &levels(id, SignalLevel::Active), &db)
            .unwrap();
        let events = p
            .apply(&tickets, &levels(id, SignalLevel::Unknown), &db)
            .unwrap();
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
        p.apply(&tickets, &levels(id, SignalLevel::Active), &db)
            .unwrap();
        assert!(p.levels().contains_key(&id));
        p.forget_ticket(id);
        assert!(!p.levels().contains_key(&id));
        assert!(!p.is_auto_reviewed(id));
    }

    #[test]
    fn clear_auto_review_drops_provenance() {
        let (db, tickets, id) = setup();
        let mut p = PollLoop::new();
        // Drive an idle move so the ticket is recorded as auto-reviewed.
        p.apply(&tickets, &levels(id, SignalLevel::Active), &db)
            .unwrap();
        p.apply(&tickets, &levels(id, SignalLevel::Idle), &db)
            .unwrap();
        assert!(p.is_auto_reviewed(id));
        // A manual move clears the provenance so the card is not dragged back.
        p.clear_auto_review(id);
        assert!(!p.is_auto_reviewed(id));
    }
}
