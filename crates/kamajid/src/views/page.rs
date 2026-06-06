//! The full HTML document shell: head (the design-system stylesheets +
//! vendored Datastar module + the board drag-and-drop script + the search-slice
//! module + viewport), a flex shell of the Slack-style workspace rail (project
//! selection lives here now) beside a `.main` column holding the topbar
//! (breadcrumb + live-search box + "+ New"), the board, and an empty modal
//! mount. `data-init` opens the `/ui/events` SSE stream (RC.6 has
//! no `on-load` event; `data-init` runs once when the element is first
//! processed). Parameterized bindings use a colon (`data-on:click`).

use kamaji_core::models::{Project, Status, Ticket};
use maud::{html, Markup, PreEscaped, DOCTYPE};

use super::board::board;

/// Render the board page for `project`, with `projects` populating the rail.
pub fn page(
    project: &Project,
    projects: &[Project],
    by_status: &[(Status, Vec<Ticket>)],
) -> Markup {
    // "needs attention" = current project's Review-status ticket count.
    let attention = by_status
        .iter()
        .find(|(s, _)| *s == Status::Review)
        .map(|(_, ts)| ts.len())
        .unwrap_or(0);
    // The crumb dot's gradient matches the active rail tile's cN.
    let active_pos = projects
        .iter()
        .position(|p| p.id == project.id)
        .unwrap_or(0);
    let active_c = super::sidebar::project_color_index(active_pos);

    html! {
        (DOCTYPE)
        html lang="en" data-theme="dark" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "kamaji — " (project.name) }
                link rel="stylesheet" href="/assets/tokens.css";
                link rel="stylesheet" href="/assets/layout.css";
                link rel="stylesheet" href="/assets/sidebar.css";
                link rel="stylesheet" href="/assets/modal.css";
                link rel="stylesheet" href="/assets/board.css";
                link rel="stylesheet" href="/assets/search.css";
                link rel="stylesheet" href="/assets/terminal.css";
                script type="module" src="/assets/datastar.js" {}
                script defer src="/assets/board-dnd.js" {}
                script type="module" src="/assets/search.js" {}
                script type="module" src="/assets/keybindings.js" {}
            }
            body class="rail-open" data-init="@get('/ui/events')" {
                (super::sidebar::rail(projects, project.id, attention))
                div class="main" {
                    header class="topbar" {
                        span class="crumb" {
                            span class=(format!("crumb-dot c{active_c}")) {}
                            span class="crumb-name" { (project.name) }
                        }
                        div class="search-slot" {
                            div class="search-wrap" {
                                svg class="search-ico" width="13" height="13"
                                    viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                    stroke-width="2.3" stroke-linecap="round" {
                                    circle cx="11" cy="11" r="7" {}
                                    line x1="20.5" y1="20.5" x2="16.5" y2="16.5" {}
                                }
                                input id="search" class="search" type="search"
                                      placeholder="Search tickets…" autocomplete="off"
                                      aria-label="Search tickets";
                                button class="search-clear" type="button"
                                       aria-label="Clear search" { "✕" }
                                span class="search-kbd kbd" { "/" }
                            }
                        }
                        span class="search-count" aria-live="polite" {}
                        span class="spacer" {}
                        button class="sessions-btn"
                               data-on:click=(PreEscaped("@get('/ui/sessions/manage')")) {
                            "Sessions"
                        }
                        button class="new-ticket"
                               data-on:click=(PreEscaped(format!("@get('/ui/tickets/new?project={}')", project.id))) {
                            "+ New"
                        }
                    }
                    (board(by_status))
                    div id="modal" {}
                }
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
        for sheet in [
            "tokens.css",
            "layout.css",
            "sidebar.css",
            "modal.css",
            "board.css",
            "terminal.css",
        ] {
            assert!(
                html.contains(&format!(r#"href="/assets/{sheet}""#)),
                "css link {sheet}:\n{html}"
            );
        }
        assert!(
            html.contains(r#"src="/assets/datastar.js""#),
            "datastar module:\n{html}"
        );
        assert!(
            html.contains(r#"src="/assets/board-dnd.js""#),
            "board drag-and-drop script:\n{html}"
        );
        assert!(
            html.contains(r#"src="/assets/keybindings.js""#),
            "global keybindings module:\n{html}"
        );
        assert!(
            !html.contains(r#"href="/assets/app.css""#),
            "old monolithic app.css must be gone:\n{html}"
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
    fn page_has_modal_mount_and_rail() {
        let p = project(1, "acme");
        let html = page(&p, std::slice::from_ref(&p), &empty_board()).into_string();
        assert!(html.contains(r#"id="modal""#), "modal mount:\n{html}");
        assert!(html.contains(r#"class="rail""#), "rail aside:\n{html}");
        assert!(
            html.contains(r#"class="ws active""#),
            "active project tile:\n{html}"
        );
        assert!(
            html.contains(r#"<span class="ws-label">acme</span>"#),
            "project name as ws-label:\n{html}"
        );
        assert!(
            !html.contains(r#"id="project-select""#),
            "old switcher select must be gone:\n{html}"
        );
    }

    #[test]
    fn topbar_fills_the_search_slot_and_links_search_assets() {
        // Slice 4 owns the topbar search markup + its CSS/JS. The foundation left
        // an empty `.search-slot`; this fills it and links the assets.
        let p = project(1, "acme");
        let html = page(&p, std::slice::from_ref(&p), &empty_board()).into_string();
        assert!(
            html.contains(r#"href="/assets/search.css""#),
            "search css link:\n{html}"
        );
        assert!(
            html.contains(r#"src="/assets/search.js""#),
            "search js module:\n{html}"
        );
        assert!(
            html.contains(r#"id="search""#),
            "search input id (focused by `/`):\n{html}"
        );
        assert!(
            html.contains(r#"placeholder="Search tickets"#),
            "search placeholder:\n{html}"
        );
        assert!(html.contains("search-clear"), "✕ clear button:\n{html}");
        assert!(
            html.contains(r#"class="search-count""#),
            "results total slot:\n{html}"
        );
        // The slot the foundation left must no longer be empty.
        assert!(
            !html.contains(r#"<div class="search-slot"></div>"#),
            "search slot must be filled, not empty:\n{html}"
        );
    }

    #[test]
    fn topbar_has_sessions_button_opening_the_manage_modal() {
        let p = project(1, "acme");
        let html = page(&p, std::slice::from_ref(&p), &empty_board()).into_string();
        assert!(
            html.contains(r#"class="sessions-btn""#),
            "sessions button present:\n{html}"
        );
        assert!(
            html.contains("@get('/ui/sessions/manage')"),
            "sessions button opens the manage modal:\n{html}"
        );
    }

    #[test]
    fn topbar_new_button_and_rail_toggle_contract() {
        let p = project(5, "acme");
        let html = page(&p, std::slice::from_ref(&p), &empty_board()).into_string();
        assert!(
            html.contains(r#"class="new-ticket""#),
            "new-ticket class:\n{html}"
        );
        assert!(
            html.contains("@get('/ui/tickets/new?project=5')"),
            "new-ticket fires the new-ticket route:\n{html}"
        );
        assert!(
            html.contains(r#"class="rail-open""#),
            "body rail-open:\n{html}"
        );
        assert!(
            html.contains(r#"data-on:click="document.body.classList.toggle('rail-open')""#),
            "rail-toggle flips rail-open:\n{html}"
        );
    }
}
