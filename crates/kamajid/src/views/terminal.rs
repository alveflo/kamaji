//! The inline terminal panel: a resizable, movable "terminal window" modal that
//! embeds a ticket's `zellij` session in an iframe (served through the
//! authenticating reverse proxy, so no token prompt). Rooted at `#modal` so
//! Datastar's `@get` morph-by-id drops it into the page mount.
//!
//! Dense-redesign look (Slice 5): a title bar with a live-dot + `#id` + task
//! name on the left and, upper-right, a ⤢ maximize/restore toggle next to the
//! ✕ close; the black terminal body (the real session iframe); a zellij-style
//! status bar (running indicator + session name + agent label) along the
//! bottom. Bindings use the RC.6 colon form.
//!
//! The window opens near-fullscreen but can be **resized** from any edge/corner
//! grip (`.term-rh[data-dir]`), **moved** by dragging the title bar, and
//! **maximized/restored** via the ⤢ button (or a title-bar double-click) —
//! all driven by `assets/term-resize.js`, which persists the geometry. The
//! maximize button has no inline binding; the script wires its toggle and flips
//! its glyph. zellij web reflows
//! the embedded session on its own when the iframe element changes size, so the
//! proxy is unchanged; this is view + CSS + that one client script.
//!
//! Deliberately NO Escape-to-close: the embedded terminal captures the keyboard
//! and Escape is a vital terminal key (vim, menus, …) — closing the panel on it
//! would steal it. The ✕ is the only close affordance.

use kamaji_core::models::Agent;
use maud::{html, Markup, PreEscaped};

/// JS that clears the `#modal` mount, closing the panel.
const CLOSE_JS: &str = "document.getElementById('modal').replaceChildren()";

/// The eight edge/corner resize grips, by compass direction. `term-resize.js`
/// reads each grip's `data-dir` to know which edges to move.
const RESIZE_DIRS: [&str; 8] = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];

/// Render the terminal panel for ticket `id` titled `title`, run by `agent` in
/// session `session_name`, embedding `src` (the proxy iframe URL,
/// `<proxy_base>/<session>`).
pub fn terminal_panel(id: i64, title: &str, agent: Agent, session_name: &str, src: &str) -> Markup {
    html! {
        div id="modal" {
            div class="term-modal" {
                header class="term-bar" {
                    span class="term-livedot" {}
                    span class="term-title" {
                        span class="term-id" { "#" (id) }
                        (PreEscaped("&nbsp; ")) (title)
                    }
                    // Maximize/restore toggle — wired (and its glyph flipped)
                    // by term-resize.js, so deliberately no inline binding.
                    button class="term-max" aria-label="Maximize terminal"
                           title="Maximize" { "⤢" }
                    button class="term-close" aria-label="Close terminal"
                           data-on:click=(PreEscaped(CLOSE_JS)) { "✕" }
                }
                iframe class="term-frame" src=(src) {}
                footer class="term-status" {
                    span class="term-stat" {
                        span class="term-livedot" {} " running"
                    }
                    span class="term-session" { (session_name) }
                    span class="term-agent" { (agent.label()) }
                }
                // Edge/corner resize grips — wired up by term-resize.js. Kept
                // inside the modal's clipped box so the rounded corners survive.
                @for dir in RESIZE_DIRS {
                    div class=(format!("term-rh term-rh-{dir}")) data-dir=(dir) {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> String {
        terminal_panel(
            7,
            "Add login",
            Agent::Claude,
            "kamaji-7-add-login",
            "http://127.0.0.1:8756/kamaji-7-add-login",
        )
        .into_string()
    }

    #[test]
    fn panel_is_rooted_at_modal_with_title_and_iframe() {
        let html = panel();
        assert!(
            html.starts_with(r#"<div id="modal">"#),
            "rooted at #modal:\n{html}"
        );
        assert!(html.contains("Add login"), "shows the task title:\n{html}");
        assert!(
            html.contains(r#"src="http://127.0.0.1:8756/kamaji-7-add-login""#),
            "embeds the proxy iframe:\n{html}"
        );
    }

    /// The title bar carries a live-dot and the `#id` left of the task name —
    /// the dense-redesign terminal-window look.
    #[test]
    fn title_bar_has_live_dot_and_ticket_id() {
        let html = panel();
        assert!(
            html.contains(r#"class="term-livedot""#),
            "live-dot present:\n{html}"
        );
        assert!(
            html.contains(r##"class="term-id">#7<"##),
            "ticket #id left of the title:\n{html}"
        );
    }

    /// A zellij-style status bar along the bottom: a running indicator, the
    /// session name, and the agent label.
    #[test]
    fn status_bar_shows_session_and_agent() {
        let html = panel();
        assert!(
            html.contains(r#"class="term-status""#),
            "status bar present:\n{html}"
        );
        assert!(html.contains("running"), "running indicator:\n{html}");
        assert!(
            html.contains("kamaji-7-add-login"),
            "session name in status bar:\n{html}"
        );
        assert!(
            html.contains("Claude Code"),
            "agent label in status bar:\n{html}"
        );
    }

    /// A maximize/restore button sits in the title bar, left of the ✕ close.
    /// It carries no inline binding — `term-resize.js` wires its stateful
    /// toggle (and flips its glyph) — so it must NOT bind `data-on:click`.
    #[test]
    fn title_bar_has_maximize_button_left_of_close() {
        let html = panel();
        assert!(
            html.contains(r#"class="term-max""#),
            "maximize button present:\n{html}"
        );
        assert!(
            html.contains(r#"aria-label="Maximize terminal""#),
            "maximize button is labelled:\n{html}"
        );
        let max_at = html.find(r#"class="term-max""#).unwrap();
        let close_at = html.find(r#"class="term-close""#).unwrap();
        assert!(max_at < close_at, "maximize sits left of close:\n{html}");
        // The toggle is JS-driven, not an inline Datastar binding.
        let max_btn = &html[max_at..close_at];
        assert!(
            !max_btn.contains("data-on:click"),
            "maximize has no inline binding (term-resize.js wires it):\n{max_btn}"
        );
    }

    #[test]
    fn close_uses_colon_binding_and_clears_modal() {
        let html = panel();
        assert!(
            html.contains("data-on:click="),
            "colon close binding:\n{html}"
        );
        assert!(
            !html.contains("data-on-click"),
            "no inert hyphen binding:\n{html}"
        );
        assert!(
            html.contains("getElementById('modal')"),
            "clears the mount:\n{html}"
        );
    }

    /// The terminal captures the keyboard, so the panel must NOT bind Escape
    /// (it would steal a vital terminal key). The ✕ is the only close.
    #[test]
    fn does_not_bind_escape() {
        let html = panel();
        assert!(!html.contains("keydown"), "no keydown handler:\n{html}");
        assert!(!html.contains("Escape"), "no Escape handler:\n{html}");
    }

    /// The window carries eight edge/corner resize grips (one per compass
    /// direction), each tagged with a `data-dir` that `term-resize.js` reads.
    #[test]
    fn has_eight_resize_handles_with_directions() {
        let html = panel();
        for dir in ["n", "s", "e", "w", "ne", "nw", "se", "sw"] {
            assert!(
                html.contains(&format!(
                    r#"class="term-rh term-rh-{dir}" data-dir="{dir}""#
                )),
                "resize grip for {dir}:\n{html}"
            );
        }
        assert_eq!(
            html.matches("term-rh ").count(),
            8,
            "exactly eight grips:\n{html}"
        );
    }

    #[test]
    fn title_is_html_escaped() {
        let html = terminal_panel(1, "a<b>&c", Agent::Claude, "s", "http://x/s").into_string();
        assert!(html.contains("a&lt;b&gt;&amp;c"), "title escaped:\n{html}");
    }
}
