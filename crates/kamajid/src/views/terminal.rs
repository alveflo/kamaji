//! The inline terminal panel: a near-fullscreen "terminal window" modal that
//! embeds a ticket's `zellij` session in an iframe (served through the
//! authenticating reverse proxy, so no token prompt). Rooted at `#modal` so
//! Datastar's `@get` morph-by-id drops it into the page mount; the task title
//! sits top-left and an ✕ top-right closes it. Bindings use the RC.6 colon form.
//!
//! Deliberately NO Escape-to-close: the embedded terminal captures the keyboard
//! and Escape is a vital terminal key (vim, menus, …) — closing the panel on it
//! would steal it. The ✕ is the only close affordance.

use maud::{html, Markup, PreEscaped};

/// JS that clears the `#modal` mount, closing the panel.
const CLOSE_JS: &str = "document.getElementById('modal').replaceChildren()";

/// Render the terminal panel titled `title`, embedding `src` (the proxy iframe
/// URL, `<proxy_base>/<session>`).
pub fn terminal_panel(title: &str, src: &str) -> Markup {
    html! {
        div id="modal" {
            div class="term-modal" {
                header class="term-bar" {
                    span class="term-title" { (title) }
                    button class="term-close" aria-label="Close terminal"
                           data-on:click=(PreEscaped(CLOSE_JS)) { "✕" }
                }
                iframe class="term-frame" src=(src) {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_is_rooted_at_modal_with_title_and_iframe() {
        let html =
            terminal_panel("Add login", "http://127.0.0.1:8756/kamaji-7-add-login").into_string();
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

    #[test]
    fn close_uses_colon_binding_and_clears_modal() {
        let html = terminal_panel("t", "http://x/s").into_string();
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
        let html = terminal_panel("t", "http://x/s").into_string();
        assert!(!html.contains("keydown"), "no keydown handler:\n{html}");
        assert!(!html.contains("Escape"), "no Escape handler:\n{html}");
    }

    #[test]
    fn title_is_html_escaped() {
        let html = terminal_panel("a<b>&c", "http://x/s").into_string();
        assert!(html.contains("a&lt;b&gt;&amp;c"), "title escaped:\n{html}");
    }
}
