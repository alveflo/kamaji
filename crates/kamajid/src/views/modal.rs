//! The create/edit ticket modal fragment. Returned by `GET /ui/tickets/new` and
//! `GET /ui/tickets/:id/edit`, it targets `#modal`. Fields map 1:1 to
//! `CreateTicket`/`UpdateTicket`; submit fires the existing JSON API.

use kamaji_core::models::{Agent, Ticket};
use maud::{html, Markup, PreEscaped};

/// Render the modal. `editing` carries an existing ticket (edit mode) or is
/// `None` (create mode, scoped to `project_id`). `default_agent` pre-selects the
/// agent in create mode. `error` is shown inline when re-rendered after a 400.
pub fn ticket_form(
    project_id: i64,
    editing: Option<&Ticket>,
    default_agent: Agent,
    error: Option<&str>,
) -> Markup {
    let (title, desc, prompt, agent, submit_action, heading) = match editing {
        Some(t) => (
            t.title.clone(),
            t.description.clone(),
            t.initial_prompt.clone().unwrap_or_default(),
            t.agent,
            format!("@patch('/tickets/{}')", t.id),
            "Edit ticket",
        ),
        None => (
            String::new(),
            String::new(),
            String::new(),
            default_agent,
            "@post('/tickets')".to_string(),
            "New ticket",
        ),
    };
    html! {
        dialog open class="modal" id="ticket-dialog" {
            form data-on-submit=(PreEscaped(submit_action)) {
                @if editing.is_none() {
                    input type="hidden" name="project_id" data-bind="project_id" value=(project_id);
                }
                h2 { (heading) }
                label for="f-title" { "Title" }
                input id="f-title" name="title" data-bind="title" value=(title) required;
                label for="f-desc" { "Description" }
                textarea id="f-desc" name="description" data-bind="description" rows="3" { (desc) }
                label for="f-prompt" { "Initial prompt" }
                textarea id="f-prompt" name="initial_prompt" data-bind="initial_prompt" rows="3" { (prompt) }
                label for="f-agent" { "Agent" }
                select id="f-agent" name="agent" data-bind="agent" {
                    @for a in Agent::all() {
                        option value=(a.as_str()) selected[a == agent] { (a.label()) }
                    }
                }
                @if let Some(e) = error {
                    p class="form-error" { (e) }
                }
                div class="form-actions" {
                    button type="button" class="act"
                           data-on-click="@get('/ui/tickets/cancel')" { "Cancel" }
                    button type="submit" class="act" { "Save" }
                }
            }
        }
    }
}

/// An empty `#modal` fragment that closes/clears the dialog (returned after a
/// successful submit and on Cancel).
pub fn modal_closed() -> Markup {
    html! { div id="modal" {} }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket() -> Ticket {
        Ticket {
            id: 9,
            project_id: 1,
            title: "Add login".into(),
            description: "d".into(),
            initial_prompt: Some("do".into()),
            agent: Agent::Codex,
            status: kamaji_core::models::Status::Todo,
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
    fn create_form_posts_to_tickets_with_default_agent() {
        let html = ticket_form(1, None, Agent::Claude, None).into_string();
        assert!(html.contains("@post('/tickets')"), "create posts:\n{html}");
        assert!(
            html.contains(r#"value="claude" selected"#),
            "default agent preselected:\n{html}"
        );
        assert!(
            html.contains(r#"name="project_id""#),
            "scopes to project:\n{html}"
        );
    }

    #[test]
    fn edit_form_patches_and_prefills() {
        let t = ticket();
        let html = ticket_form(1, Some(&t), Agent::Claude, None).into_string();
        assert!(
            html.contains("@patch('/tickets/9')"),
            "edit patches:\n{html}"
        );
        assert!(html.contains("Add login"), "title prefilled:\n{html}");
        assert!(
            html.contains(r#"value="codex" selected"#),
            "agent prefilled:\n{html}"
        );
    }

    #[test]
    fn validation_error_renders_inline() {
        let html =
            ticket_form(1, None, Agent::Claude, Some("title must not be empty")).into_string();
        assert!(
            html.contains("title must not be empty"),
            "error shown:\n{html}"
        );
    }
}
