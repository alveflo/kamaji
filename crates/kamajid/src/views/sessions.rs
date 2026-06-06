//! The session-cleanup modal fragment. Returned by `GET /ui/sessions/manage`.
//! Rooted at `#modal` so Datastar's `@get` morph-by-id replaces the page's empty
//! mount. Checkboxes select sessions; the footer swaps to an in-modal confirm
//! bar before POSTing `/sessions/delete`. Bindings use the RC.6 colon form and
//! single-quoted JS (no HTML-attribute escaping needed), like `views::modal`.

use kamaji_core::session::{SessionEntry, SessionKind};
use maud::{html, Markup, PreEscaped};

/// JS that clears the `#modal` mount. Reused by Cancel, the close button, the
/// Escape handler, and the confirm-success `.then`.
const CLEAR_MODAL_JS: &str = "document.getElementById('modal').replaceChildren()";

/// The session-cleanup modal. `entries` is the classified session list (snapshot
/// at open time); `zellij_reachable` is false when `list-sessions` couldn't be
/// queried, which shows an explanatory note instead of an empty list.
pub fn sessions_modal(entries: &[SessionEntry], zellij_reachable: bool) -> Markup {
    let escape_handler = format!("if(evt.key==='Escape'){{{CLEAR_MODAL_JS}}}");

    let recount = "const d=el.closest('dialog');const n=d.querySelectorAll('input[name=session]:checked').length;const b=d.querySelector('#sess-del-btn');b.disabled=n===0;b.textContent='Delete selected ('+n+')'";
    let show_confirm = "const d=el.closest('dialog');const n=d.querySelectorAll('input[name=session]:checked').length;if(n===0)return;d.querySelector('#sess-confirm-text').textContent='Delete '+n+' session(s)? Ticket sessions also remove their git worktree and branch.';d.querySelector('#sess-foot-main').hidden=true;d.querySelector('#sess-foot-confirm').hidden=false";
    let hide_confirm = "const d=el.closest('dialog');d.querySelector('#sess-foot-confirm').hidden=true;d.querySelector('#sess-foot-main').hidden=false";
    let do_delete = format!(
        "const d=el.closest('dialog');const names=[...d.querySelectorAll('input[name=session]:checked')].map(c=>c.value);fetch('/sessions/delete',{{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify({{names}})}}).then(r=>{{if(r.ok){{{CLEAR_MODAL_JS}}}}})"
    );

    html! {
        div id="modal" {
            dialog open class="modal" id="sessions-dialog"
                   data-on:keydown__window=(PreEscaped(escape_handler)) {
                div class="modal-head" {
                    span class="modal-title" { "Sessions" }
                    button type="button" class="modal-close"
                           data-on:click=(PreEscaped(CLEAR_MODAL_JS)) { "✕" }
                }
                div class="modal-body" {
                    @if !zellij_reachable {
                        p class="sess-empty" { "Could not reach zellij." }
                    } @else if entries.is_empty() {
                        p class="sess-empty" { "No active kamaji sessions." }
                    } @else {
                        div id="sess-list" class="sess-list"
                            data-on:change=(PreEscaped(recount)) {
                            @for e in entries {
                                (session_row(e))
                            }
                        }
                    }
                }
                div class="modal-foot" {
                    div id="sess-foot-main" class="sess-foot-row" {
                        button type="button" class="btn"
                               data-on:click=(PreEscaped(CLEAR_MODAL_JS)) { "Cancel" }
                        button type="button" class="btn btn-danger" id="sess-del-btn" disabled
                               data-on:click=(PreEscaped(show_confirm)) { "Delete selected (0)" }
                    }
                    div id="sess-foot-confirm" class="sess-foot-row" hidden {
                        span id="sess-confirm-text" class="sess-confirm-text" {}
                        span class="foot-spacer" {}
                        button type="button" class="btn"
                               data-on:click=(PreEscaped(hide_confirm)) { "Back" }
                        button type="button" class="btn btn-danger"
                               data-on:click=(PreEscaped(do_delete)) { "Confirm" }
                    }
                }
            }
        }
    }
}

/// One checkbox row. Main sessions carry `sess-warn` (heavier to kill: a project
/// workspace) and a ⚠ marker. The checkbox value is the raw session name.
fn session_row(e: &SessionEntry) -> Markup {
    let warn = matches!(e.kind, SessionKind::Main);
    html! {
        label class=(if warn { "check sess-row sess-warn" } else { "check sess-row" }) {
            input type="checkbox" name="session" value=(e.name);
            span class="sess-name" { (e.name) }
            (kind_tag(&e.kind))
        }
    }
}

/// The right-aligned descriptor for a session's kind.
fn kind_tag(kind: &SessionKind) -> Markup {
    match kind {
        SessionKind::Ticket { id, title, status } => html! {
            span class="sess-tag sess-tag-ticket" {
                "ticket #" (id) " · \"" (title) "\" · " (status.title())
            }
        },
        SessionKind::Main => html! {
            span class="sess-tag sess-tag-main" { "project workspace ⚠" }
        },
        SessionKind::Orphan => html! {
            span class="sess-tag sess-tag-orphan" { "orphan" }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kamaji_core::models::Status;

    fn entries() -> Vec<SessionEntry> {
        vec![
            SessionEntry {
                name: "kamaji-12-fix-auth".into(),
                kind: SessionKind::Ticket {
                    id: 12,
                    title: "Fix auth".into(),
                    status: Status::Review,
                },
            },
            SessionEntry {
                name: "kamaji-main-3".into(),
                kind: SessionKind::Main,
            },
            SessionEntry {
                name: "kamaji-7-old".into(),
                kind: SessionKind::Orphan,
            },
        ]
    }

    #[test]
    fn fragment_is_rooted_at_modal_with_dialog() {
        let html = sessions_modal(&entries(), true).into_string();
        assert!(html.starts_with(r#"<div id="modal">"#), "{html}");
        assert!(html.contains("<dialog"), "{html}");
        assert!(html.contains(r#"class="modal-title">Sessions"#), "{html}");
    }

    #[test]
    fn renders_a_checkbox_row_per_session_with_kind_labels() {
        let html = sessions_modal(&entries(), true).into_string();
        for name in ["kamaji-12-fix-auth", "kamaji-main-3", "kamaji-7-old"] {
            assert!(
                html.contains(&format!(r#"name="session" value="{name}""#)),
                "checkbox for {name}:\n{html}"
            );
        }
        assert!(html.contains("Fix auth"), "ticket title:\n{html}");
        assert!(html.contains("#12"), "ticket id:\n{html}");
        assert!(html.contains("project workspace"), "main label:\n{html}");
        assert!(html.contains("orphan"), "orphan label:\n{html}");
        assert!(html.contains("sess-warn"), "main row flagged:\n{html}");
    }

    #[test]
    fn confirm_bar_posts_selected_to_sessions_delete() {
        let html = sessions_modal(&entries(), true).into_string();
        assert!(
            html.contains("sess-foot-confirm"),
            "confirm bar present:\n{html}"
        );
        assert!(
            html.contains("fetch('/sessions/delete',{method:'POST'"),
            "confirm posts to delete:\n{html}"
        );
        assert!(
            html.contains("input[name=session]:checked"),
            "gathers checked names:\n{html}"
        );
        assert!(html.contains("data-on:click="), "colon bindings:\n{html}");
        assert!(!html.contains("data-on-click"), "no inert hyphen:\n{html}");
    }

    #[test]
    fn empty_list_shows_empty_state() {
        let html = sessions_modal(&[], true).into_string();
        assert!(html.contains("No active kamaji sessions"), "{html}");
        assert!(!html.contains(r#"name="session""#), "{html}");
    }

    #[test]
    fn unreachable_zellij_shows_a_note() {
        let html = sessions_modal(&[], false).into_string();
        assert!(html.contains("Could not reach zellij"), "{html}");
    }
}
