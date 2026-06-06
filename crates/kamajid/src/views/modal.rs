//! The create/edit ticket modal fragment. Returned by `GET /ui/tickets/new` and
//! `GET /ui/tickets/:id/edit`. The fragment's top-level element is `#modal` so
//! Datastar's `@get` morph-by-id replaces the page's empty `<div id="modal">`
//! mount. Submit fires the existing JSON API with an explicit `fetch()` (the
//! response is ignored — the new/updated card arrives over `/ui/events`) and
//! clears the mount on success. Cancel and Escape clear it directly. Bindings
//! use the RC.6 colon form (`data-on:click`); the hyphen form is inert. The
//! fragment is built against the shared modal chrome classes (`modal-head` /
//! `modal-body` / `modal-foot`) defined in `modal.css`.

use kamaji_core::models::{Agent, Ticket};
use maud::{html, Markup, PreEscaped};

/// JS that clears the `#modal` mount (removing the dialog). Reused by the
/// success-close, the Cancel button, and the Escape-key handler. Single-quoted
/// so it needs no HTML-attribute escaping.
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
    let (title, desc, prompt, agent, heading, submit_label) = match editing {
        Some(t) => (
            t.title.clone(),
            t.description.clone(),
            t.initial_prompt.clone().unwrap_or_default(),
            t.agent,
            "Edit ticket",
            "Save changes",
        ),
        None => (
            String::new(),
            String::new(),
            String::new(),
            default_agent,
            "New ticket",
            "Create ticket",
        ),
    };
    // Build the JSON body from the form's named controls and POST/PATCH it via an
    // explicit `fetch()` — a Datastar `@post`'s second argument is request
    // *options*, not a body, so signal/typing pitfalls are avoided entirely. Read
    // controls via `f.elements['title']` (not `f.title` — every element has a
    // `.title` property that shadows the named control). project_id is baked in as
    // a numeric literal so it deserializes into `i64` (a bound string would not).
    // On a 2xx we clear `#modal`; a 4xx leaves it open so the inline error shows.
    // Single-quoted JS only, so the expression is safe inside the double-quoted,
    // unescaped (PreEscaped) attribute value.
    let fields = "title:f.elements['title'].value,description:f.elements['description'].value,initial_prompt:f.elements['initial_prompt'].value,agent:f.elements['agent'].value";
    let close_on_ok = format!("then(r=>{{if(r.ok){{{CLEAR_MODAL_JS}}}}})");
    let submit_action = match editing {
        Some(t) => format!(
            "evt.preventDefault();const f=evt.target;fetch('/tickets/{id}',{{method:'PATCH',headers:{{'content-type':'application/json'}},body:JSON.stringify({{{fields}}})}}).{close_on_ok}",
            id = t.id,
        ),
        None => format!(
            "evt.preventDefault();const f=evt.target;fetch('/tickets',{{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify({{project_id:{project_id},{fields}}})}}).{close_on_ok}",
        ),
    };
    // Nice-to-have: Escape dismisses the modal. The window keydown handler is on
    // the dialog, so it is bound only while the dialog is mounted.
    let escape_handler = format!("if(evt.key==='Escape'){{{CLEAR_MODAL_JS}}}");
    html! {
        div id="modal" {
            dialog open class="modal" id="ticket-dialog"
                   data-on:keydown__window=(PreEscaped(escape_handler)) {
                form data-on:submit=(PreEscaped(submit_action)) {
                    div class="modal-head" {
                        span class="modal-title" { (heading) }
                        @if let Some(t) = editing {
                            span class="modal-idpill" { "#" (t.id) }
                        }
                        button type="button" class="modal-close"
                               data-on:click=(PreEscaped(CLEAR_MODAL_JS)) { "✕" }
                    }
                    div class="modal-body" {
                        div class="field" {
                            label for="f-title" { "Title" span class="req" { "*" } }
                            input id="f-title" name="title" value=(title) required;
                        }
                        div class="field" {
                            label for="f-desc" { "Description" }
                            textarea id="f-desc" name="description" rows="3" { (desc) }
                        }
                        div class="field" {
                            label for="f-prompt" { "Initial prompt" }
                            textarea id="f-prompt" name="initial_prompt" rows="3" { (prompt) }
                            div class="hint" {
                                "The first message handed to the agent when it starts."
                            }
                        }
                        div class="field" {
                            label { "Agent" }
                            // The agent value spliced into the click JS is `Agent::as_str()` —
                            // a closed match of static ASCII identifiers (claude/codex/copilot),
                            // so it needs no escaping inside the single-quoted JS literal.
                            div class="seg" role="group" aria-label="Agent" {
                                @for a in Agent::all() {
                                    button type="button"
                                           class=[(a == agent).then_some("on")]
                                           data-on:click=(PreEscaped(format!(
                                               "this.form.elements['agent'].value='{val}';this.closest('.seg').querySelectorAll('button').forEach(b=>b.classList.remove('on'));this.classList.add('on')",
                                               val = a.as_str()
                                           ))) { (a.label()) }
                                }
                            }
                            input type="hidden" name="agent" value=(agent.as_str());
                        }
                        @if let Some(e) = error {
                            p class="form-error" { (e) }
                        }
                    }
                    div class="modal-foot" {
                        button type="button" class="btn"
                               data-on:click=(PreEscaped(CLEAR_MODAL_JS)) { "Cancel" }
                        button type="submit" class="btn btn-primary" { (submit_label) }
                    }
                }
            }
        }
    }
}

/// An empty `#modal` fragment that closes/clears the dialog. Returned by
/// `GET /ui/tickets/cancel`; `@get` morphs it over the mount to clear it.
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
        let html = ticket_form(7, None, Agent::Claude, None).into_string();
        assert!(
            html.contains("fetch('/tickets',{method:'POST'"),
            "create posts via fetch:\n{html}"
        );
        assert!(
            html.contains(r#"<input type="hidden" name="agent" value="claude">"#),
            "default agent is the hidden input value:\n{html}"
        );
        assert!(
            html.contains("project_id:7"),
            "scopes to project as a numeric literal:\n{html}"
        );
        assert!(
            html.contains(r#"class="modal-title">New ticket"#),
            "create heading in the modal-title chrome:\n{html}"
        );
        assert!(
            !html.contains("modal-idpill"),
            "create mode has no id pill:\n{html}"
        );
        assert!(
            html.contains(r#"class="btn btn-primary">Create ticket"#),
            "create submit label:\n{html}"
        );
    }

    #[test]
    fn edit_form_patches_and_prefills() {
        let t = ticket();
        let html = ticket_form(1, Some(&t), Agent::Claude, None).into_string();
        assert!(
            html.contains("fetch('/tickets/9',{method:'PATCH'"),
            "edit patches via fetch:\n{html}"
        );
        assert!(html.contains("Add login"), "title prefilled:\n{html}");
        assert!(
            html.contains(r#"<input type="hidden" name="agent" value="codex">"#),
            "agent prefilled as the hidden input value:\n{html}"
        );
        assert!(
            html.contains(r#"class="modal-title">Edit ticket"#),
            "edit heading in the modal-title chrome:\n{html}"
        );
        assert!(
            html.contains(r#"class="modal-idpill">#9</span>"#),
            "edit mode renders the id pill:\n{html}"
        );
        assert!(
            html.contains(r#"class="btn btn-primary">Save changes"#),
            "edit submit label:\n{html}"
        );
    }

    /// The fragment renders against the shared modal chrome (`modal.css`).
    #[test]
    fn renders_through_shared_modal_chrome() {
        let html = ticket_form(1, None, Agent::Claude, None).into_string();
        for cls in [
            r#"class="modal-head""#,
            r#"class="modal-body""#,
            r#"class="field""#,
            r#"class="modal-foot""#,
            r#"class="modal-close""#,
        ] {
            assert!(html.contains(cls), "chrome {cls} present:\n{html}");
        }
        assert!(
            html.contains(r#"id="f-title""#),
            "title input keeps its id:\n{html}"
        );
        assert!(
            html.contains(r#"type="submit" class="btn btn-primary""#),
            "submit button carries btn btn-primary:\n{html}"
        );
        assert!(
            html.contains(r#"class="btn" data-on:click"#) && html.contains("Cancel"),
            "Cancel button carries btn:\n{html}"
        );
        // The legacy markup must be gone.
        assert!(
            !html.contains("<h2")
                && !html.contains("form-actions")
                && !html.contains(r#"class="act""#),
            "legacy h2/form-actions/.act markup removed:\n{html}"
        );
    }

    /// Datastar `@get` morphs the response's top-level element by id, so the
    /// fragment must be rooted at `#modal` to replace the page's mount.
    #[test]
    fn fragment_is_rooted_at_modal_for_morph_by_id() {
        let html = ticket_form(1, None, Agent::Claude, None).into_string();
        assert!(
            html.starts_with(r#"<div id="modal">"#),
            "fragment rooted at #modal:\n{html}"
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

    /// On a 2xx the submit `.then` clears the `#modal` mount; a 4xx leaves it
    /// open so the inline validation error is visible.
    #[test]
    fn submit_closes_modal_only_on_success() {
        let html = ticket_form(1, None, Agent::Claude, None).into_string();
        assert!(
            html.contains("if(r.ok)") && html.contains("getElementById('modal')"),
            "closes the mount only on a 2xx:\n{html}"
        );
    }

    /// Pressing Escape anywhere while the modal is open clears it.
    #[test]
    fn escape_dismisses_modal() {
        let html = ticket_form(1, None, Agent::Claude, None).into_string();
        assert!(
            html.contains("data-on:keydown__window"),
            "binds a window keydown handler (colon syntax):\n{html}"
        );
        assert!(
            html.contains("Escape"),
            "filters on the Escape key:\n{html}"
        );
    }

    #[test]
    fn bindings_use_rc6_colon_syntax_not_hyphen() {
        let html = ticket_form(1, None, Agent::Claude, None).into_string();
        assert!(html.contains("data-on:submit="), "colon submit:\n{html}");
        assert!(
            html.contains("data-on:click="),
            "colon cancel click:\n{html}"
        );
        assert!(
            !html.contains("data-on-submit")
                && !html.contains("data-on-click")
                && !html.contains("data-on-keydown"),
            "no inert hyphen bindings:\n{html}"
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

    #[test]
    fn agent_picker_is_segmented_control_not_select() {
        let html = ticket_form(1, None, Agent::Claude, None).into_string();
        assert!(
            html.contains(r#"class="seg""#),
            "renders a segmented control:\n{html}"
        );
        // The default agent's button is highlighted.
        assert!(
            html.contains(r#"class="on" data-on:click"#),
            "default agent button carries `on`:\n{html}"
        );
        // The value is still a form-named control the submit JS can read.
        assert!(
            html.contains(r#"<input type="hidden" name="agent" value="claude">"#),
            "hidden agent input carries the selected value:\n{html}"
        );
        // The seg buttons set the hidden input + move the highlight, client-side.
        assert!(
            html.contains("this.form.elements['agent'].value='codex'"),
            "a seg button writes its value into the hidden input:\n{html}"
        );
        assert!(
            !html.contains("<select"),
            "the old dropdown is gone:\n{html}"
        );
        assert_eq!(
            html.matches(r#"class="on""#).count(),
            1,
            "exactly one seg button is highlighted:\n{html}"
        );
    }
}
