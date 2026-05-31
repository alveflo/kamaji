//! The create/edit ticket modal fragment. Returned by `GET /ui/tickets/new` and
//! `GET /ui/tickets/:id/edit`, it targets `#modal`. Fields map 1:1 to
//! `CreateTicket`/`UpdateTicket`; submit fires the existing JSON API.

use kamaji_core::models::{Agent, Ticket};
use maud::{html, Markup, PreEscaped};

/// JS that clears the `#modal` mount (removing the dialog). Reused by the
/// success-`.then` submit close and the Escape-key handler. Single-quoted so it
/// needs no HTML-attribute escaping; a `function` literal avoids an escaped `>`.
const CLEAR_MODAL_JS: &str = "document.getElementById('modal').replaceChildren()";

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
    // On a successful submit, close the modal client-side: the command action's
    // promise resolves only on a 2xx (a 4xx rejects), so `.then` clears `#modal`
    // on success and leaves it open on validation failure. Mutations still go
    // through the existing JSON command API.
    let submit_action = format!("{submit_action}.then(function(){{{CLEAR_MODAL_JS}}})");
    // Nice-to-have: Escape dismisses the modal. The window keydown handler lives
    // on the dialog, so it is bound only while the dialog is mounted.
    let escape_handler = format!("if(evt.key==='Escape'){{{CLEAR_MODAL_JS}}}");
    html! {
        div id="modal" {
            dialog open class="modal" id="ticket-dialog"
                   data-on-keydown__window=(PreEscaped(escape_handler)) {
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

    /// Datastar's default `outer` patch morphs the element whose `id` matches an
    /// existing node. The form is rendered inside the `#modal` mount so the
    /// fragment morphs `#modal` (the only persistent target on the page) and the
    /// dialog actually appears — and is symmetric with `modal_closed()`.
    #[test]
    fn form_is_mounted_in_modal() {
        let html = ticket_form(1, None, Agent::Claude, None).into_string();
        assert!(
            html.starts_with(r#"<div id="modal">"#),
            "wrapped in #modal mount:\n{html}"
        );
        assert!(
            html.contains("<dialog"),
            "carries the dialog inside the mount:\n{html}"
        );
    }

    #[test]
    fn modal_closed_renders_empty_mount() {
        assert_eq!(modal_closed().into_string(), r#"<div id="modal"></div>"#);
    }

    /// On a successful submit the modal closes client-side: the command action's
    /// promise `.then` clears the `#modal` mount. A 4xx rejects the promise, so
    /// the modal stays open on validation failure. Mutations still go through the
    /// existing JSON command API — no duplicate command logic.
    #[test]
    fn create_submit_closes_modal_on_success() {
        let html = ticket_form(1, None, Agent::Claude, None).into_string();
        assert!(html.contains("@post('/tickets')"), "posts to API:\n{html}");
        assert!(
            html.contains(".then(function()"),
            "closes on the resolved promise:\n{html}"
        );
        assert!(
            html.contains("getElementById('modal')"),
            "clears the #modal mount:\n{html}"
        );
    }

    #[test]
    fn edit_submit_closes_modal_on_success() {
        let t = ticket();
        let html = ticket_form(1, Some(&t), Agent::Claude, None).into_string();
        assert!(
            html.contains("@patch('/tickets/9')"),
            "patches API:\n{html}"
        );
        assert!(
            html.contains(".then(function()") && html.contains("getElementById('modal')"),
            "closes on success:\n{html}"
        );
    }

    /// Pressing Escape anywhere while the modal is open clears it. The window
    /// keydown handler lives on the dialog, so it is bound only while the dialog
    /// is mounted.
    #[test]
    fn escape_dismisses_modal() {
        let html = ticket_form(1, None, Agent::Claude, None).into_string();
        assert!(
            html.contains("data-on-keydown__window"),
            "binds a window keydown handler:\n{html}"
        );
        assert!(
            html.contains("Escape"),
            "filters on the Escape key:\n{html}"
        );
    }
}
