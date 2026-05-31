//! The full HTML document shell: head (CSS link + vendored Datastar module +
//! viewport), a top bar (wordmark, project switcher, "+ Ticket"), the board,
//! and an empty modal mount. `data-init` opens the `/ui/events` SSE stream
//! (RC.6 has no `on-load` event; `data-init` runs once when the element is
//! first processed). Parameterized bindings use a colon (`data-on:click`).

use kamaji_core::models::{Project, Status, Ticket};
use maud::{html, Markup, DOCTYPE};

use super::board::board;

/// Render the board page for `project`, with `projects` populating the switcher.
pub fn page(
    project: &Project,
    projects: &[Project],
    by_status: &[(Status, Vec<Ticket>)],
) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" data-theme="dark" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "kamaji — " (project.name) }
                link rel="stylesheet" href="/assets/app.css";
                script type="module" src="/assets/datastar.js" {}
            }
            body data-init="@get('/ui/events')" {
                header class="topbar" {
                    span class="wordmark" { "kamaji" }
                    div class="project-switcher" {
                        label for="project-select" { "project" }
                        select id="project-select"
                               data-on:change="window.location = '/?project=' + evt.target.value" {
                            @for p in projects {
                                option value=(p.id) selected[p.id == project.id] { (p.name) }
                            }
                        }
                    }
                    button class="new-ticket"
                           data-on:click=(maud::PreEscaped(format!("@get('/ui/tickets/new?project={}')", project.id))) {
                        "+ Ticket"
                    }
                }
                (board(by_status))
                div id="modal" {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kamaji_core::models::Agent;
    use std::path::PathBuf;

    fn project(id: i64, name: &str) -> Project {
        Project {
            id,
            name: name.into(),
            root_dir: PathBuf::from("/tmp/p"),
            default_agent: Some(Agent::Claude),
            created_at: String::new(),
        }
    }

    fn empty_board() -> Vec<(Status, Vec<Ticket>)> {
        Status::all().into_iter().map(|s| (s, Vec::new())).collect()
    }

    #[test]
    fn page_links_css_and_vendored_datastar() {
        let p = project(1, "acme");
        let html = page(&p, std::slice::from_ref(&p), &empty_board()).into_string();
        assert!(
            html.contains(r#"href="/assets/app.css""#),
            "css link:\n{html}"
        );
        assert!(
            html.contains(r#"src="/assets/datastar.js""#),
            "datastar module:\n{html}"
        );
    }

    #[test]
    fn page_opens_ui_events_on_init() {
        let p = project(1, "acme");
        let html = page(&p, std::slice::from_ref(&p), &empty_board()).into_string();
        // RC.6: `data-init` (not `data-on-load`) fires once on element processing.
        assert!(
            html.contains(r#"data-init="@get('/ui/events')""#),
            "sse hook:\n{html}"
        );
        assert!(
            !html.contains("data-on-load"),
            "no inert hyphen on-load:\n{html}"
        );
    }

    #[test]
    fn page_has_modal_mount_and_switcher() {
        let p = project(1, "acme");
        let html = page(&p, std::slice::from_ref(&p), &empty_board()).into_string();
        assert!(html.contains(r#"id="modal""#), "modal mount:\n{html}");
        assert!(html.contains("acme"), "switcher option:\n{html}");
    }
}
