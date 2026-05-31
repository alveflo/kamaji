//! The per-ticket card partial. Stable id `card-<id>`; session bullet `●`/`○`;
//! `#<id>` + title; agent label; state-appropriate action buttons firing the
//! existing JSON API via Datastar. Pure: `card(&Ticket) -> Markup`.

use kamaji_core::models::{Status, Ticket};
use maud::{html, Markup, PreEscaped};

/// Render one ticket as a card element. The id is `card-<id>` so SSE patches
/// can target it; the per-column accent comes from `data-status`.
pub fn card(t: &Ticket) -> Markup {
    let bullet = if t.session_name.is_some() {
        "●"
    } else {
        "○"
    };
    html! {
        article id=(format!("card-{}", t.id))
                class="card"
                data-status=(t.status.as_str()) {
            header class="card-head" {
                span class="bullet" { (bullet) }
                span class="card-id" { "#" (t.id) }
                span class="card-title" { (t.title) }
            }
            div class="card-meta" {
                span class="agent" { (t.agent.label()) }
                @if matches!(t.status, Status::InProgress | Status::Review) {
                    span class="chip" {
                        @if t.status == Status::Review { "idle" } @else { "active" }
                    }
                }
            }
            (card_actions(t))
        }
    }
}

/// State-appropriate action buttons. Each fires the EXISTING JSON command API
/// via a Datastar action attribute; the authoritative UI update arrives over
/// `/ui/events` (3c), so the response body is ignored.
fn card_actions(t: &Ticket) -> Markup {
    let id = t.id;
    html! {
        footer class="card-actions" {
            @match t.status {
                Status::Todo => {
                    button class="act" data-on-click=(PreEscaped(format!("@post('/tickets/{id}/start')"))) { "▸ Start" }
                    button class="act" data-on-click=(PreEscaped(format!("@get('/ui/tickets/{id}/edit')"))) { "Edit" }
                    button class="act danger" data-on-click=(PreEscaped(format!("@delete('/tickets/{id}')"))) { "Delete" }
                }
                Status::InProgress => {
                    button class="act" data-on-click=(PreEscaped(format!("@post('/tickets/{id}/attach')"))) { "⤢ Attach" }
                    button class="act" data-on-click=(PreEscaped(format!("@post('/tickets/{id}/move', {{target:'review'}})"))) { "Move" }
                    button class="act" data-on-click=(PreEscaped(format!("@get('/ui/tickets/{id}/edit')"))) { "Edit" }
                    button class="act" data-on-click=(PreEscaped(format!("@post('/tickets/{id}/done', {{cleanup:false}})"))) { "✓ Done" }
                }
                Status::Review => {
                    button class="act" data-on-click=(PreEscaped(format!("@post('/tickets/{id}/attach')"))) { "⤢ Attach" }
                    button class="act" data-on-click=(PreEscaped(format!("@post('/tickets/{id}/move', {{target:'in_progress'}})"))) { "↩ In Progress" }
                    button class="act" data-on-click=(PreEscaped(format!("@post('/tickets/{id}/done', {{cleanup:false}})"))) { "✓ Done" }
                    button class="act" data-on-click=(PreEscaped(format!("@get('/ui/tickets/{id}/edit')"))) { "Edit" }
                }
                Status::Done => {
                    button class="act danger" data-on-click=(PreEscaped(format!("@delete('/tickets/{id}')"))) { "Delete" }
                }
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
            id,
            project_id: 1,
            title: format!("title{id}"),
            description: String::new(),
            initial_prompt: None,
            agent: Agent::Claude,
            status,
            position: 0,
            session_name: None,
            worktree_path: None,
            branch: None,
            auto_reviewed: false,
            instrumented: false,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn card_has_stable_id_title_and_agent_label() {
        let html = card(&ticket(3, Status::Todo)).into_string();
        assert!(html.contains(r#"id="card-3""#), "stable card id:\n{html}");
        assert!(html.contains("#3"), "ticket id shown:\n{html}");
        assert!(html.contains("title3"), "title shown:\n{html}");
        assert!(html.contains("Claude Code"), "agent label:\n{html}");
    }

    #[test]
    fn no_session_renders_hollow_bullet() {
        let html = card(&ticket(1, Status::Todo)).into_string();
        assert!(html.contains("○"), "hollow bullet when no session:\n{html}");
        assert!(!html.contains("●"), "no filled bullet:\n{html}");
    }

    #[test]
    fn live_session_renders_filled_bullet() {
        let mut t = ticket(1, Status::InProgress);
        t.session_name = Some("sess1".into());
        let html = card(&t).into_string();
        assert!(
            html.contains("●"),
            "filled bullet when session present:\n{html}"
        );
    }

    #[test]
    fn todo_card_offers_start_not_attach() {
        let html = card(&ticket(1, Status::Todo)).into_string();
        assert!(html.contains("/tickets/1/start"), "Start action:\n{html}");
        assert!(
            !html.contains("/tickets/1/attach"),
            "no attach in todo:\n{html}"
        );
    }

    #[test]
    fn in_progress_card_offers_attach() {
        let html = card(&ticket(5, Status::InProgress)).into_string();
        assert!(html.contains("/tickets/5/attach"), "Attach action:\n{html}");
    }
}
