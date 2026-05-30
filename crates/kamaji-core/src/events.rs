//! Canonical state-change events shared by the daemon (which emits them) and
//! clients (which consume them over SSE). The enum is the in-process source of
//! truth; the daemon frames each event as an SSE record using `sse_name()` for
//! the `event:` line and the variant's payload for the `data:` line.

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

    /// Reconstruct an `Event` from the daemon's SSE framing: the dotted `event:`
    /// name plus the bare `data:` payload (the inner `data` of the tagged enum,
    /// with no `type` envelope). The inverse of [`Self::sse_name`] + the daemon's
    /// `payload_json`. Returns `None` for an unknown name or a payload that does
    /// not match the named variant.
    pub fn from_sse(name: &str, data: &str) -> Option<Event> {
        let inner: serde_json::Value = serde_json::from_str(data).ok()?;
        // Rebuild the tagged `{ "type": <snake>, "data": <inner> }` shape and
        // deserialize through the canonical enum so framing stays defined once.
        let tag = match name {
            "ticket.created" => "ticket_created",
            "ticket.updated" => "ticket_updated",
            "ticket.moved" => "ticket_moved",
            "ticket.deleted" => "ticket_deleted",
            "session.started" => "session_started",
            "session.idle" => "session_idle",
            "session.exited" => "session_exited",
            _ => return None,
        };
        let tagged = serde_json::json!({ "type": tag, "data": inner });
        serde_json::from_value(tagged).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Agent, Status};

    /// Mirror of kamajid's routes/events.rs::payload_json — the SSE `data:` payload.
    fn payload_json(event: &Event) -> String {
        let full = serde_json::to_value(event).unwrap();
        let data = full.get("data").cloned().unwrap_or(serde_json::Value::Null);
        serde_json::to_string(&data).unwrap()
    }

    fn sample_events() -> Vec<Event> {
        let t = crate::models::Ticket {
            id: 1,
            project_id: 1,
            title: "t".into(),
            description: String::new(),
            initial_prompt: None,
            agent: Agent::Claude,
            status: Status::Todo,
            position: 0,
            session_name: None,
            worktree_path: None,
            branch: None,
            auto_reviewed: false,
            instrumented: false,
            created_at: String::new(),
            updated_at: String::new(),
        };
        vec![
            Event::TicketCreated(t.clone()),
            Event::TicketUpdated(t),
            Event::TicketMoved {
                id: 5,
                from: Status::InProgress,
                to: Status::Review,
                at: "2026-05-30T10:23:45Z".into(),
            },
            Event::TicketDeleted { id: 7 },
            Event::SessionStarted {
                ticket_id: 3,
                session_name: "kamaji-3-x".into(),
            },
            Event::SessionIdle { ticket_id: 3 },
            Event::SessionExited {
                ticket_id: 3,
                session_name: "kamaji-3-x".into(),
            },
        ]
    }

    #[test]
    fn from_sse_round_trips_daemon_framing_for_every_variant() {
        for ev in sample_events() {
            let name = ev.sse_name();
            let data = payload_json(&ev);
            let back =
                Event::from_sse(name, &data).expect("from_sse should decode the daemon frame");
            assert_eq!(back.sse_name(), name, "variant changed for {name}");
            // The payload must also round-trip identically.
            assert_eq!(payload_json(&back), data, "payload differs for {name}");
        }
    }

    #[test]
    fn from_sse_rejects_unknown_event_name() {
        assert!(Event::from_sse("nope.unknown", "{}").is_none());
    }

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
        assert_eq!(Event::TicketDeleted { id: 1 }.sse_name(), "ticket.deleted");
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
