//! The confirmation modal fragment. Returned by
//! `GET /ui/tickets/:id/confirm?action=done|delete`. Replaces the browser's
//! native `confirm()` for the destructive card actions with an in-page modal
//! built against the shared modal chrome (`modal-head` / `modal-body` /
//! `modal-foot`, see `modal.css`). The fragment is rooted at `#modal` so
//! Datastar's `@get` morph-by-id swaps it into the page's empty mount.
//!
//! The Confirm button fires the existing JSON command API with an explicit
//! `fetch()` (the board update arrives over `/ui/events`) and clears the mount
//! on a 2xx; Cancel, the ✕ close button, and Escape clear it directly. Bindings
//! use the RC.6 colon form (`data-on:click`); the hyphen form is inert.

use maud::{html, Markup, PreEscaped};
use serde::Deserialize;

/// JS that clears the `#modal` mount. Reused by the success-close, Cancel, the
/// ✕ button, and the Escape handler. Single-quoted so it needs no escaping.
const CLEAR_MODAL_JS: &str = "document.getElementById('modal').replaceChildren()";

/// Which destructive action a confirmation dialog guards. Deserialized from the
/// `?action=` query of `GET /ui/tickets/:id/confirm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfirmAction {
    Done,
    Delete,
}

/// The confirmation modal for a destructive ticket action. Pure:
/// `(id, action) -> Markup`. The Done variant reproduces the old confirm's
/// teardown (POST `/done` with `{cleanup:true}`); Delete hits the JSON
/// DELETE. Both clear the modal on success — the card change arrives over SSE.
pub fn ticket_confirm(id: i64, action: ConfirmAction) -> Markup {
    let (heading, message, confirm_label, action_js, danger) = match action {
        ConfirmAction::Done => (
            "Mark ticket done?",
            format!("Mark #{id} done and tear down its session? Its zellij session and worktree will be removed."),
            "Mark done",
            format!(
                "fetch('/tickets/{id}/done',{{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify({{cleanup:true}})}})"
            ),
            false,
        ),
        ConfirmAction::Delete => (
            "Delete ticket?",
            format!("Delete #{id}? This cannot be undone."),
            "Delete",
            format!("fetch('/tickets/{id}',{{method:'DELETE'}})"),
            true,
        ),
    };
    confirm_dialog(heading, &message, confirm_label, &action_js, danger)
}

/// The confirmation modal for clearing a project's whole Done column
/// (`GET /ui/projects/:id/confirm-delete-done`). Pure: `project_id -> Markup`.
/// Confirm fires the bulk `DELETE /projects/:id/done-tickets`; the removed cards
/// disappear over SSE and the modal clears on success.
pub fn delete_done_confirm(project_id: i64) -> Markup {
    confirm_dialog(
        "Delete all done tickets?",
        "Permanently delete every ticket in the Done column? This cannot be undone. Worktrees and zellij sessions are not torn down.",
        "Delete all",
        &format!("fetch('/projects/{project_id}/done-tickets',{{method:'DELETE'}})"),
        true,
    )
}

/// Generic confirmation modal chrome. `action_js` is a bare `fetch(...)`
/// expression; this wraps it so the mount is cleared only on a 2xx. `danger`
/// styles the confirm button as `btn-danger` (destructive) vs `btn-primary`.
fn confirm_dialog(
    heading: &str,
    message: &str,
    confirm_label: &str,
    action_js: &str,
    danger: bool,
) -> Markup {
    // Run the command, then clear the mount on success; the board update arrives
    // over /ui/events. Single-quoted JS only, so the expression is safe inside
    // the double-quoted, unescaped (PreEscaped) attribute value.
    let confirm_action = format!("{action_js}.then(r=>{{if(r.ok){{{CLEAR_MODAL_JS}}}}})");
    // Escape dismisses the dialog. Bound on the window only while mounted.
    let escape_handler = format!("if(evt.key==='Escape'){{{CLEAR_MODAL_JS}}}");
    let confirm_class = if danger {
        "btn btn-danger"
    } else {
        "btn btn-primary"
    };
    html! {
        div id="modal" {
            dialog open class="modal" id="confirm-dialog"
                   data-on:keydown__window=(PreEscaped(escape_handler)) {
                div class="modal-head" {
                    span class="modal-title" { (heading) }
                    button type="button" class="modal-close"
                           data-on:click=(PreEscaped(CLEAR_MODAL_JS)) { "✕" }
                }
                div class="modal-body" {
                    p class="confirm-message" { (message) }
                }
                div class="modal-foot" {
                    button type="button" class="btn"
                           data-on:click=(PreEscaped(CLEAR_MODAL_JS)) { "Cancel" }
                    button type="button" class=(confirm_class)
                           data-on:click=(PreEscaped(confirm_action)) { (confirm_label) }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Datastar `@get` morphs by id, so the fragment must be rooted at `#modal`.
    #[test]
    fn fragment_is_rooted_at_modal_for_morph_by_id() {
        let html = ticket_confirm(5, ConfirmAction::Done).into_string();
        assert!(
            html.starts_with(r#"<div id="modal">"#),
            "fragment rooted at #modal:\n{html}"
        );
        assert!(html.contains("<dialog"), "carries the dialog:\n{html}");
    }

    /// Renders against the shared modal chrome (`modal.css`).
    #[test]
    fn renders_through_shared_modal_chrome() {
        let html = ticket_confirm(5, ConfirmAction::Done).into_string();
        for cls in [
            r#"class="modal-head""#,
            r#"class="modal-body""#,
            r#"class="modal-foot""#,
            r#"class="modal-close""#,
        ] {
            assert!(html.contains(cls), "chrome {cls} present:\n{html}");
        }
    }

    #[test]
    fn done_shows_teardown_message_and_posts_cleanup() {
        let html = ticket_confirm(7, ConfirmAction::Done).into_string();
        assert!(
            html.contains("Mark #7 done and tear down its session?"),
            "done message names the ticket and teardown:\n{html}"
        );
        assert!(
            html.contains("fetch('/tickets/7/done',{method:'POST'"),
            "done posts to the done endpoint:\n{html}"
        );
        assert!(
            html.contains("body:JSON.stringify({cleanup:true})"),
            "done still carries the cleanup flag in the body:\n{html}"
        );
        assert!(
            html.contains(r#"class="btn btn-primary""#),
            "done confirm is the primary button:\n{html}"
        );
    }

    #[test]
    fn delete_shows_irreversible_message_and_deletes() {
        let html = ticket_confirm(3, ConfirmAction::Delete).into_string();
        assert!(
            html.contains("Delete #3? This cannot be undone."),
            "delete message warns it is irreversible:\n{html}"
        );
        assert!(
            html.contains("fetch('/tickets/3',{method:'DELETE'})"),
            "delete hits the JSON DELETE:\n{html}"
        );
        assert!(
            html.contains(r#"class="btn btn-danger""#),
            "delete confirm is the danger button:\n{html}"
        );
    }

    /// The confirm action clears the mount only on a 2xx; a failure leaves the
    /// dialog open. The card change itself arrives over /ui/events.
    #[test]
    fn delete_done_confirm_warns_and_hits_bulk_delete() {
        let html = delete_done_confirm(9).into_string();
        assert!(
            html.starts_with(r#"<div id="modal">"#),
            "fragment rooted at #modal for morph-by-id:\n{html}"
        );
        assert!(
            html.contains("Delete all done tickets?"),
            "heading names the bulk action:\n{html}"
        );
        assert!(
            html.contains("This cannot be undone."),
            "warns it is irreversible:\n{html}"
        );
        assert!(
            html.contains("fetch('/projects/9/done-tickets',{method:'DELETE'})"),
            "confirm hits the bulk delete endpoint:\n{html}"
        );
        assert!(
            html.contains(r#"class="btn btn-danger""#),
            "confirm is the danger button:\n{html}"
        );
    }

    #[test]
    fn confirm_clears_modal_only_on_success() {
        let html = ticket_confirm(5, ConfirmAction::Done).into_string();
        assert!(
            html.contains("if(r.ok)") && html.contains("getElementById('modal')"),
            "clears the mount only on a 2xx:\n{html}"
        );
    }

    #[test]
    fn cancel_and_close_clear_the_mount() {
        let html = ticket_confirm(5, ConfirmAction::Delete).into_string();
        assert!(
            html.contains(r#"class="btn" data-on:click"#) && html.contains("Cancel"),
            "Cancel button clears the mount:\n{html}"
        );
        assert!(
            html.contains(r#"class="modal-close""#),
            "has a ✕ close button:\n{html}"
        );
    }

    #[test]
    fn escape_dismisses_the_dialog() {
        let html = ticket_confirm(5, ConfirmAction::Done).into_string();
        assert!(
            html.contains("data-on:keydown__window") && html.contains("Escape"),
            "binds a window Escape handler (colon syntax):\n{html}"
        );
    }

    #[test]
    fn bindings_use_rc6_colon_syntax_not_hyphen() {
        let html = ticket_confirm(5, ConfirmAction::Done).into_string();
        assert!(
            html.contains("data-on:click="),
            "colon click binding:\n{html}"
        );
        assert!(
            !html.contains("data-on-click=") && !html.contains("data-on-keydown"),
            "no inert hyphen bindings:\n{html}"
        );
    }
}
