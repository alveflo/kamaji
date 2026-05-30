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
}

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
