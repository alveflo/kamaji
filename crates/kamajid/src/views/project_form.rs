//! The new-project modal fragment. Returned by `GET /ui/projects/new`. Like the
//! ticket modal, the fragment's top-level element is `#modal` so Datastar's
//! `@get` morph-by-id replaces the page's empty `<div id="modal">` mount. Submit
//! fires the existing JSON API (`POST /projects`) with an explicit `fetch()` (a
//! Datastar `@post`'s 2nd argument is request *options*, not a body) and, on a
//! 2xx, navigates to the freshly created project so its tile shows in the rail —
//! projects broadcast no SSE event, so a navigation (not a live patch) is how the
//! rail learns of the new tile. Cancel, the ✕, and Escape clear the mount
//! directly. Bindings use the RC.6 colon form (`data-on:click`); the hyphen form
//! is inert. The fragment composes from the shared modal chrome classes
//! (`modal-head` / `modal-body` / `modal-foot` / `field` / `seg` / `btn`) defined
//! in `modal.css` — this slice adds no new CSS.

use kamaji_core::models::Agent;
use maud::{html, Markup, PreEscaped};

/// JS that clears the `#modal` mount (removing the dialog). Reused by the
/// Cancel button, the ✕, and the Escape-key handler. Single-quoted so it needs
/// no HTML-attribute escaping.
const CLEAR_MODAL_JS: &str = "document.getElementById('modal').replaceChildren()";

/// Render the new-project modal. `default_agent` pre-selects the segmented agent
/// control (it comes from config — there is no project yet to override it).
/// `error` is shown inline when re-rendered after a 400.
pub fn project_form(default_agent: Agent, error: Option<&str>) -> Markup {
    // Build the JSON body from the form's named controls and POST it via an
    // explicit `fetch()`. Read controls via `f.elements['name']` (not `f.name` —
    // a form's `.name` property shadows the named control). The agent lives in a
    // hidden input driven by the segmented buttons. On a 2xx we read the created
    // project and navigate to it so the rail shows the new tile; a 4xx leaves the
    // modal open so the inline error stays visible. Single-quoted JS only, so the
    // expression is safe inside the double-quoted, unescaped attribute value.
    let fields = "name:f.elements['name'].value,root_dir:f.elements['root_dir'].value,default_agent:f.elements['default_agent'].value";
    let submit_action = format!(
        "evt.preventDefault();const f=evt.target;fetch('/projects',{{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify({{{fields}}})}}).then(r=>{{if(r.ok){{r.json().then(p=>{{window.location='/?project='+p.id}})}}}})",
    );
    // Escape dismisses the modal. The window keydown handler is on the dialog, so
    // it is bound only while the dialog is mounted.
    let escape_handler = format!("if(evt.key==='Escape'){{{CLEAR_MODAL_JS}}}");
    html! {
        div id="modal" {
            dialog open class="modal" id="project-dialog"
                   data-on:keydown__window=(PreEscaped(escape_handler)) {
                form data-on:submit=(PreEscaped(submit_action)) {
                    div class="modal-head" {
                        span class="modal-title" { "New project" }
                        button type="button" class="modal-close"
                               data-on:click=(PreEscaped(CLEAR_MODAL_JS)) { "✕" }
                    }
                    div class="modal-body" {
                        div class="field" {
                            label for="proj-name" { "Name" span class="req" { "*" } }
                            input id="proj-name" name="name" placeholder="My Project" required;
                        }
                        div class="field" {
                            label for="proj-root" { "Root directory" span class="req" { "*" } }
                            input id="proj-root" name="root_dir" class="mono"
                                  placeholder="~/dev/kamaji" required;
                            div class="hint" { "Absolute path to the project's git repository." }
                        }
                        div class="field" {
                            label { "Default agent" }
                            input type="hidden" name="default_agent" id="proj-agent"
                                  value=(default_agent.as_str());
                            div class="seg" {
                                @for a in Agent::all() {
                                    // Each button bakes its agent value: on click it
                                    // writes the hidden input and toggles `.on` so
                                    // exactly one button is active.
                                    @let set = format!(
                                        "const b=evt.currentTarget;document.getElementById('proj-agent').value='{}';b.parentElement.querySelectorAll('button').forEach(x=>x.classList.remove('on'));b.classList.add('on')",
                                        a.as_str(),
                                    );
                                    button type="button"
                                           class=(if a == default_agent { "on" } else { "" })
                                           data-on:click=(PreEscaped(set)) { (a.label()) }
                                }
                            }
                            div class="hint" { "Used for new tickets unless overridden." }
                        }
                        @if let Some(e) = error {
                            p class="form-error" { (e) }
                        }
                    }
                    div class="modal-foot" {
                        button type="button" class="btn"
                               data-on:click=(PreEscaped(CLEAR_MODAL_JS)) { "Cancel" }
                        button type="submit" class="btn btn-primary" { "Create project" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_is_rooted_at_modal_for_morph_by_id() {
        let html = project_form(Agent::Claude, None).into_string();
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
    fn submit_posts_to_projects_via_fetch() {
        let html = project_form(Agent::Claude, None).into_string();
        assert!(
            html.contains("fetch('/projects',{method:'POST'"),
            "creates via POST /projects:\n{html}"
        );
        // Body carries the three named controls, read via f.elements (not f.name,
        // which a form's own .name property shadows).
        for field in [
            "name:f.elements['name'].value",
            "root_dir:f.elements['root_dir'].value",
            "default_agent:f.elements['default_agent'].value",
        ] {
            assert!(html.contains(field), "body field {field}:\n{html}");
        }
    }

    #[test]
    fn success_navigates_to_new_project_so_rail_shows_it() {
        let html = project_form(Agent::Claude, None).into_string();
        assert!(
            html.contains("if(r.ok)") && html.contains("window.location='/?project='+p.id"),
            "on 2xx navigate to the created project:\n{html}"
        );
    }

    #[test]
    fn renders_through_shared_modal_chrome_no_new_css() {
        let html = project_form(Agent::Claude, None).into_string();
        for cls in [
            r#"class="modal-head""#,
            r#"class="modal-body""#,
            r#"class="modal-foot""#,
            r#"class="modal-close""#,
            r#"class="field""#,
            r#"class="seg""#,
        ] {
            assert!(html.contains(cls), "chrome {cls} present:\n{html}");
        }
        assert!(
            html.contains(r#"class="modal-title">New project"#),
            "heading in the modal-title chrome:\n{html}"
        );
        assert!(
            html.contains(r#"type="submit" class="btn btn-primary">Create project"#),
            "submit button carries btn btn-primary:\n{html}"
        );
    }

    #[test]
    fn name_is_required_and_root_is_mono_with_hint() {
        let html = project_form(Agent::Claude, None).into_string();
        assert!(
            html.contains(r#"name="name""#) && html.contains("required"),
            "name input is required:\n{html}"
        );
        assert!(
            html.contains(r#"name="root_dir" class="mono""#),
            "root directory is a mono path input:\n{html}"
        );
        assert!(
            html.contains("Absolute path to the project's git repository."),
            "root directory hint present:\n{html}"
        );
    }

    #[test]
    fn agent_is_a_segmented_control_with_default_preselected() {
        let html = project_form(Agent::Codex, None).into_string();
        // Hidden input seeds the submitted value with the default agent.
        assert!(
            html.contains(
                r#"<input type="hidden" name="default_agent" id="proj-agent" value="codex">"#
            ),
            "hidden agent input seeded with default:\n{html}"
        );
        // One button per agent, all three labelled.
        for a in Agent::all() {
            assert!(
                html.contains(a.label()),
                "agent button {}:\n{html}",
                a.label()
            );
        }
        // Exactly one button starts active (the default).
        assert_eq!(
            html.matches(r#"class="on""#).count(),
            1,
            "exactly one preselected agent button:\n{html}"
        );
        assert!(
            html.contains(r#"class="on" data-on:click"#)
                && html.contains("document.getElementById('proj-agent').value='codex'"),
            "default (codex) button is the active one:\n{html}"
        );
    }

    #[test]
    fn escape_and_cancel_dismiss_the_modal() {
        let html = project_form(Agent::Claude, None).into_string();
        assert!(
            html.contains("data-on:keydown__window") && html.contains("Escape"),
            "window keydown handler filters on Escape:\n{html}"
        );
        assert!(
            html.contains(r#"class="btn" data-on:click"#) && html.contains("Cancel"),
            "Cancel button clears the mount:\n{html}"
        );
        assert!(
            html.contains("getElementById('modal').replaceChildren()"),
            "clear-modal JS present:\n{html}"
        );
    }

    #[test]
    fn bindings_use_rc6_colon_syntax_not_hyphen() {
        let html = project_form(Agent::Claude, None).into_string();
        assert!(html.contains("data-on:submit="), "colon submit:\n{html}");
        assert!(html.contains("data-on:click="), "colon click:\n{html}");
        assert!(
            !html.contains("data-on-submit")
                && !html.contains("data-on-click")
                && !html.contains("data-on-keydown"),
            "no inert hyphen bindings:\n{html}"
        );
    }

    #[test]
    fn validation_error_renders_inline() {
        let html = project_form(Agent::Claude, Some("name must not be empty")).into_string();
        assert!(
            html.contains("name must not be empty"),
            "error shown inline:\n{html}"
        );
    }
}
