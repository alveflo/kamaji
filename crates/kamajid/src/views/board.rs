//! Column and board partials. `column()` is reused verbatim by the SSE
//! serializer (3c) so an initial render and a live patch are byte-identical.

use kamaji_core::models::{Status, Ticket};
use maud::{html, Markup};

use super::card::card;

/// One Kanban column. Stable id `col-<status.as_str()>` is the SSE patch
/// target. Header shows the title (`Status::title()` → "Needs attention" for
/// Review) and the live count. Empty columns show a quiet placeholder.
pub fn column(status: Status, tickets: &[Ticket]) -> Markup {
    html! {
        section class="column"
                id=(format!("col-{}", status.as_str()))
                data-status=(status.as_str()) {
            header class="col-head" {
                span class="col-title" { (status.title()) }
                span class="col-count" { (tickets.len()) }
            }
            div class="col-body" {
                @if tickets.is_empty() {
                    p class="col-empty" { "Nothing here" }
                } @else {
                    @for t in tickets {
                        (card(t))
                    }
                }
            }
        }
    }
}

/// The full four-column board. `by_status` is indexed by `Status::all()` order.
pub fn board(by_status: &[(Status, Vec<Ticket>)]) -> Markup {
    html! {
        main class="board" id="board" {
            @for (status, tickets) in by_status {
                (column(*status, tickets))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kamaji_core::models::Agent;

    fn ticket(id: i64, status: Status) -> Ticket {
        Ticket {
            id, project_id: 1, title: format!("title{id}"), description: String::new(),
            initial_prompt: None, agent: Agent::Claude, status, position: 0,
            session_name: None, worktree_path: None, branch: None,
            auto_reviewed: false, instrumented: false,
            created_at: String::new(), updated_at: String::new(),
        }
    }

    #[test]
    fn column_has_stable_id_keyed_off_status() {
        let html = column(Status::Review, &[]).into_string();
        assert!(html.contains(r#"id="col-review""#), "stable review id:\n{html}");
    }

    #[test]
    fn review_column_titled_needs_attention() {
        let html = column(Status::Review, &[]).into_string();
        assert!(html.contains("Needs attention"), "review header label:\n{html}");
    }

    #[test]
    fn empty_column_shows_placeholder() {
        let html = column(Status::Todo, &[]).into_string();
        assert!(html.contains("Nothing here"), "empty placeholder:\n{html}");
    }

    #[test]
    fn column_shows_count_and_cards() {
        let ts = vec![ticket(1, Status::Todo), ticket(2, Status::Todo)];
        let html = column(Status::Todo, &ts).into_string();
        assert!(html.contains(r#"class="col-count">2"#), "count 2:\n{html}");
        assert!(html.contains("card-1") && html.contains("card-2"), "both cards:\n{html}");
    }

    #[test]
    fn board_renders_all_four_columns() {
        let by = vec![
            (Status::Todo, vec![ticket(1, Status::Todo)]),
            (Status::InProgress, vec![]),
            (Status::Review, vec![]),
            (Status::Done, vec![]),
        ];
        let html = board(&by).into_string();
        for id in ["col-todo", "col-in_progress", "col-review", "col-done"] {
            assert!(html.contains(id), "missing {id} in:\n{html}");
        }
    }
}
