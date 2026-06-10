//! The project-management modal fragment. Returned by
//! `GET /ui/projects/:id/edit`. Lists the project's properties (its id, root
//! directory / basepath, default agent, and creation time) and lets the user
//! edit the name, root dir, and default agent, or delete the whole project.
//!
//! Like the other modals the fragment's top-level element is `#modal` so
//! Datastar's `@get` morph-by-id replaces the page's empty `<div id="modal">`
//! mount. Submit fires the JSON API (`PATCH /projects/:id`) with an explicit
//! `fetch()` (a Datastar `@patch`'s 2nd argument is request *options*, not a
//! body) and, on a 2xx, navigates to the (possibly renamed) project so the rail
//! and crumb refresh — projects broadcast no SSE event, so a navigation is how
//! the board learns of the edit. The Delete button morphs in the delete-project
//! confirmation (`GET /ui/projects/:id/confirm-delete`). Cancel, the ✕, and
//! Escape clear the mount. Bindings use the RC.6 colon form (`data-on:click`);
//! the hyphen form is inert. Composes from the shared modal chrome classes
//! (`modal-head` / `modal-body` / `modal-foot` / `field` / `btn`) in `modal.css`
//! — this slice adds no new CSS.

use kamaji_core::models::{Agent, Project};
use maud::{html, Markup, PreEscaped};

/// JS that clears the `#modal` mount (removing the dialog). Reused by the
/// Cancel button, the ✕, and the Escape-key handler. Single-quoted so it needs
/// no HTML-attribute escaping.
const CLEAR_MODAL_JS: &str = "document.getElementById('modal').replaceChildren()";

/// Render the project-management modal for `project`, pre-filled from its row.
/// `error` is shown inline when re-rendered after a 400.
pub fn project_manage_form(project: &Project, error: Option<&str>) -> Markup {
    let id = project.id;
    let root = project.root_dir.to_string_lossy().to_string();
    // Build the JSON body from the form's named controls and PATCH it via an
    // explicit `fetch()`. Read controls via `f.elements['name']` (not `f.name` —
    // a form's `.name` property shadows the named control). The agent select's
    // empty value means "use the global default" → null. On a 2xx we navigate to
    // the project so the rail/crumb reflect the rename; a 4xx leaves the modal
    // open with the inline error.
    let submit_action = format!(
        "evt.preventDefault();const f=evt.target,els=f.elements;\
const body={{name:els['name'].value,root_dir:els['root_dir'].value,default_agent:els['default_agent'].value||null}};\
fetch('/projects/{id}',{{method:'PATCH',headers:{{'content-type':'application/json'}},body:JSON.stringify(body)}}).then(r=>{{if(r.ok){{r.json().then(p=>{{window.location='/?project='+p.id}})}}else{{r.json().then(j=>{{document.getElementById('pm-error').textContent=(j&&j.error)?j.error:'Save failed'}}).catch(()=>{{document.getElementById('pm-error').textContent='Save failed'}})}}}})"
    );
    // The Delete button swaps the delete-project confirmation into `#modal`.
    let delete_action = format!("@get('/ui/projects/{id}/confirm-delete')");
    let escape_handler = format!("if(evt.key==='Escape'){{{CLEAR_MODAL_JS}}}");

    html! {
        div id="modal" {
            dialog open class="modal" id="project-manage-dialog"
                   data-on:keydown__window=(PreEscaped(escape_handler)) {
                form data-on:submit=(PreEscaped(submit_action)) {
                    div class="modal-head" {
                        span class="modal-title" { "Project settings" }
                        button type="button" class="modal-close"
                               data-on:click=(PreEscaped(CLEAR_MODAL_JS)) { "✕" }
                    }
                    div class="modal-body" {
                        div class="field" {
                            label for="pm-name" { "Name" span class="req" { "*" } }
                            input id="pm-name" name="name" value=(project.name) required;
                        }
                        div class="field" {
                            label for="pm-root" { "Root directory" span class="req" { "*" } }
                            input id="pm-root" name="root_dir" class="mono"
                                  value=(root) required;
                            div class="hint" { "Absolute path to the project's git repository." }
                        }
                        div class="field" {
                            label for="pm-agent" { "Default agent" }
                            select id="pm-agent" name="default_agent" {
                                option value="" selected[project.default_agent.is_none()] {
                                    "(global default)"
                                }
                                @for a in Agent::all() {
                                    option value=(a.as_str())
                                           selected[project.default_agent == Some(a)] {
                                        (a.label())
                                    }
                                }
                            }
                            div class="hint" { "Used for new tickets unless overridden." }
                        }
                        // Read-only properties, shown for reference.
                        div class="field" {
                            label { "Project ID" }
                            input class="mono" value=(id) disabled;
                        }
                        @if !project.created_at.is_empty() {
                            div class="field" {
                                label { "Created" }
                                input class="mono" value=(project.created_at) disabled;
                            }
                        }
                        @if let Some(e) = error {
                            p class="form-error" { (e) }
                        }
                        p id="pm-error" class="form-error" {}
                    }
                    div class="modal-foot" {
                        button type="button" class="btn btn-danger"
                               data-on:click=(PreEscaped(delete_action)) { "Delete project" }
                        span class="spacer" {}
                        button type="button" class="btn"
                               data-on:click=(PreEscaped(CLEAR_MODAL_JS)) { "Cancel" }
                        button type="submit" class="btn btn-primary" { "Save changes" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn project(default_agent: Option<Agent>) -> Project {
        Project {
            id: 7,
            name: "kamaji".into(),
            root_dir: PathBuf::from("/home/u/dev/kamaji"),
            default_agent,
            created_at: "2026-06-10T12:00:00Z".into(),
        }
    }

    #[test]
    fn fragment_is_rooted_at_modal_for_morph_by_id() {
        let html = project_manage_form(&project(Some(Agent::Claude)), None).into_string();
        assert!(
            html.starts_with(r#"<div id="modal">"#),
            "fragment rooted at #modal:\n{html}"
        );
        assert!(html.contains("<dialog"), "carries the dialog:\n{html}");
    }

    #[test]
    fn renders_through_shared_modal_chrome_no_new_css() {
        let html = project_manage_form(&project(None), None).into_string();
        for cls in [
            r#"class="modal-head""#,
            r#"class="modal-body""#,
            r#"class="modal-foot""#,
            r#"class="modal-close""#,
            r#"class="field""#,
        ] {
            assert!(html.contains(cls), "chrome {cls} present:\n{html}");
        }
        assert!(
            html.contains(r#"class="modal-title">Project settings"#),
            "title in the modal-title chrome:\n{html}"
        );
    }

    #[test]
    fn submit_patches_the_project_via_fetch() {
        let html = project_manage_form(&project(Some(Agent::Codex)), None).into_string();
        assert!(
            html.contains("fetch('/projects/7',{method:'PATCH'"),
            "edits via PATCH /projects/7:\n{html}"
        );
        for field in [
            "name:els['name'].value",
            "root_dir:els['root_dir'].value",
            "default_agent:els['default_agent'].value||null",
        ] {
            assert!(html.contains(field), "body field {field}:\n{html}");
        }
    }

    #[test]
    fn success_navigates_to_the_edited_project() {
        let html = project_manage_form(&project(None), None).into_string();
        assert!(
            html.contains("if(r.ok)") && html.contains("window.location='/?project='+p.id"),
            "on 2xx navigate to the edited project so the rail/crumb refresh:\n{html}"
        );
    }

    #[test]
    fn lists_the_project_properties() {
        let p = project(Some(Agent::Claude));
        let html = project_manage_form(&p, None).into_string();
        // Name + root (basepath) are prefilled into editable inputs.
        assert!(
            html.contains(r#"name="name" value="kamaji""#),
            "name prefilled:\n{html}"
        );
        assert!(
            html.contains(r#"name="root_dir" class="mono" value="/home/u/dev/kamaji""#),
            "root dir (basepath) prefilled:\n{html}"
        );
        // Read-only id + created-at are surfaced.
        assert!(
            html.contains(r#"value="7" disabled"#),
            "project id shown read-only:\n{html}"
        );
        assert!(
            html.contains("2026-06-10T12:00:00Z"),
            "created-at shown:\n{html}"
        );
    }

    #[test]
    fn default_agent_select_preselects_the_projects_agent() {
        let html = project_manage_form(&project(Some(Agent::Codex)), None).into_string();
        assert!(
            html.contains(r#"<option value="codex" selected>"#),
            "the project's agent is preselected:\n{html}"
        );
        // The "(global default)" option exists and is NOT selected here.
        assert!(
            html.contains(r#"<option value="">(global default)</option>"#),
            "global-default option present and unselected:\n{html}"
        );
    }

    #[test]
    fn none_agent_preselects_the_global_default_option() {
        let html = project_manage_form(&project(None), None).into_string();
        assert!(
            html.contains(r#"<option value="" selected>(global default)</option>"#),
            "no project agent → global-default option is selected:\n{html}"
        );
    }

    #[test]
    fn delete_button_opens_the_delete_confirmation() {
        let html = project_manage_form(&project(None), None).into_string();
        assert!(
            html.contains(r#"class="btn btn-danger""#) && html.contains("Delete project"),
            "a danger Delete button is present:\n{html}"
        );
        assert!(
            html.contains("@get('/ui/projects/7/confirm-delete')"),
            "Delete opens the project's delete confirmation:\n{html}"
        );
    }

    #[test]
    fn escape_and_cancel_dismiss_the_modal() {
        let html = project_manage_form(&project(None), None).into_string();
        assert!(
            html.contains("data-on:keydown__window") && html.contains("Escape"),
            "window keydown handler filters on Escape:\n{html}"
        );
        assert!(
            html.contains("getElementById('modal').replaceChildren()"),
            "clear-modal JS present:\n{html}"
        );
    }

    #[test]
    fn bindings_use_rc6_colon_syntax_not_hyphen() {
        let html = project_manage_form(&project(None), None).into_string();
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
        let html =
            project_manage_form(&project(None), Some("name must not be empty")).into_string();
        assert!(
            html.contains("name must not be empty"),
            "error shown inline:\n{html}"
        );
    }
}
