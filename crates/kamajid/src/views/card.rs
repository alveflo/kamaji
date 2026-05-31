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

/// State-appropriate action buttons. Each fires the EXISTING JSON command API;
/// the authoritative UI update arrives over `/ui/events` (3c), so the response
/// body is ignored. Commands use a plain `fetch()` (like Attach) rather than a
/// Datastar `@post`/`@delete` action: in Datastar v1 RC.6 an action's second
/// argument is request *options*, not a body, so `{target}`/`{cleanup}` would
/// never reach the server. `@get` is kept only for modal-open, where Datastar's
/// morph-into-`#modal` is exactly what we want. Note the colon in `data-on:click`
/// — RC.6 parses parameterized attributes on `:`; the hyphen form is ignored.
fn card_actions(t: &Ticket) -> Markup {
    let id = t.id;
    // Single-quoted JS only (no `"`), so each expression is safe inside the
    // double-quoted, unescaped (PreEscaped) attribute value.
    // Attach morphs the inline terminal panel over `#modal`: the route ensures
    // the session + `zellij web`, pre-authenticates the proxy, and returns a
    // near-fullscreen iframe of the live session.
    let attach = PreEscaped(format!("@get('/ui/tickets/{id}/terminal')"));
    let edit = PreEscaped(format!("@get('/ui/tickets/{id}/edit')"));
    let delete = PreEscaped(format!(
        "confirm('Delete #{id}? This cannot be undone.') && fetch('/tickets/{id}', {{method:'DELETE'}})"
    ));
    let done = PreEscaped(format!(
        "confirm('Mark #{id} done and tear down its session?') && fetch('/tickets/{id}/done', {{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify({{cleanup:true}})}})"
    ));
    let move_to = |target: &str| {
        PreEscaped(format!(
            "fetch('/tickets/{id}/move', {{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify({{target:'{target}'}})}})"
        ))
    };
    html! {
        footer class="card-actions" {
            @match t.status {
                Status::Todo => {
                    button class="act" data-on:click=(PreEscaped(format!("fetch('/tickets/{id}/start', {{method:'POST'}})"))) { "▸ Start" }
                    button class="act" data-on:click=(&edit) { "Edit" }
                    button class="act danger" data-on:click=(&delete) { "Delete" }
                }
                Status::InProgress => {
                    button class="act" data-on:click=(&attach) { "⤢ Attach" }
                    button class="act" data-on:click=(move_to("review")) { "Move" }
                    button class="act" data-on:click=(&edit) { "Edit" }
                    button class="act" data-on:click=(&done) { "✓ Done" }
                }
                Status::Review => {
                    button class="act" data-on:click=(&attach) { "⤢ Attach" }
                    button class="act" data-on:click=(move_to("in_progress")) { "↩ In Progress" }
                    button class="act" data-on:click=(&done) { "✓ Done" }
                    button class="act" data-on:click=(&edit) { "Edit" }
                }
                Status::Done => {
                    button class="act danger" data-on:click=(&delete) { "Delete" }
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
    fn delete_action_is_confirm_guarded() {
        let html = card(&ticket(2, Status::Done)).into_string();
        assert!(
            html.contains("confirm("),
            "delete guarded by confirm:\n{html}"
        );
        assert!(
            html.contains("fetch('/tickets/2', {method:'DELETE'})"),
            "delete hits the JSON API via fetch:\n{html}"
        );
    }

    #[test]
    fn bindings_use_rc6_colon_syntax_not_hyphen() {
        // RC.6 parses parameterized attributes on `:`; `data-on-click` is silently
        // ignored. This guards the whole class of "buttons do nothing" regressions.
        for s in [
            Status::Todo,
            Status::InProgress,
            Status::Review,
            Status::Done,
        ] {
            let html = card(&ticket(7, s)).into_string();
            assert!(
                html.contains("data-on:click="),
                "colon event binding for {s:?}:\n{html}"
            );
            assert!(
                !html.contains("data-on-click="),
                "no hyphen event binding for {s:?}:\n{html}"
            );
        }
    }

    #[test]
    fn move_and_done_send_a_real_json_body() {
        // `@post(url, {target})` would drop the body (2nd arg is options in RC.6),
        // so commands carrying data go through an explicit fetch.
        let html = card(&ticket(5, Status::InProgress)).into_string();
        assert!(
            html.contains("body:JSON.stringify({target:'review'})"),
            "move sends target in the body:\n{html}"
        );
        assert!(
            html.contains("body:JSON.stringify({cleanup:true})"),
            "done sends cleanup in the body:\n{html}"
        );
    }

    #[test]
    fn in_progress_card_offers_attach() {
        let html = card(&ticket(5, Status::InProgress)).into_string();
        assert!(
            html.contains("@get('/ui/tickets/5/terminal')"),
            "Attach opens the inline terminal panel:\n{html}"
        );
        assert!(
            !html.contains("window.open"),
            "Attach no longer opens a new tab:\n{html}"
        );
    }
}
