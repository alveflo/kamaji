//! The Slack-style workspace rail. Projects render as colour-coded tiles in a
//! collapsible left rail; the active project is highlighted and carries a
//! "needs attention" badge. The rail-toggle flips `body.rail-open` purely
//! client-side; tiles navigate by setting `window.location`. Parameterized
//! bindings use a colon (`data-on:click`) — the hyphen form is inert in RC.6.

use kamaji_core::models::Project;
use maud::{html, Markup, PreEscaped};

/// Map a project's position in the workspace list to its CSS colour-class
/// suffix (`c1`..`c4`), cycling the four design-system tile gradients. Shared so
/// the rail tile and the topbar crumb-dot stay in sync for the active project.
pub fn project_color_index(pos: usize) -> usize {
    (pos % 4) + 1
}

/// Two-letter tile initials for `name`: the first two alphanumeric characters
/// across the first two whitespace-separated words, uppercased. When the second
/// word contributes no alphanumeric character the second character is taken from
/// the first word instead, so the result is always two characters when the name
/// has any alphanumeric content. An empty (or fully non-alphanumeric) name falls
/// back to "?". e.g. "My Project"→"MP", "kamaji"→"KA", "test"→"TE",
/// "foo ---"→"FO".
fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let out: String = if words.len() >= 2 {
        // Try the first alphanumeric from each of the first two words.
        let first: String = words
            .iter()
            .take(2)
            .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
            .collect();
        if first.len() >= 2 {
            first
        } else {
            // Second word had no alphanumeric — fill both chars from word 0.
            words[0]
                .chars()
                .filter(|c| c.is_alphanumeric())
                .take(2)
                .collect()
        }
    } else {
        // single word (or empty): first two alphanumeric characters
        name.chars()
            .filter(|c| c.is_alphanumeric())
            .take(2)
            .collect()
    };
    if out.is_empty() {
        "?".to_string()
    } else {
        out.to_uppercase()
    }
}

/// The Slack-style workspace rail. `projects` render as tiles (the active one
/// highlighted); `active_id` is the current project; `attention` is the current
/// project's "needs attention" (Review) count, shown as a badge on the active
/// tile only (0 → no badge).
pub fn rail(projects: &[Project], active_id: i64, attention: usize) -> Markup {
    html! {
        aside class="rail" {
            div class="rail-head" {
                span class="rail-mark" {}
                span class="rail-word" { "kamaji" }
                button class="rail-toggle" aria-label="Toggle sidebar"
                       data-on:click="document.body.classList.toggle('rail-open')" {
                    span class="ic-collapsed" { "☰" }
                    span class="ic-open" { "‹" }
                }
            }
            div class="rail-list" {
                @for (i, p) in projects.iter().enumerate() {
                    @let active = p.id == active_id;
                    @let cls = if active { "ws active" } else { "ws" };
                    div class=(cls)
                        data-on:click=(PreEscaped(format!("window.location='/?project={}'", p.id))) {
                        span class="ws-ind" {}
                        span class=(format!("ws-tile c{}", project_color_index(i))) {
                            (initials(&p.name))
                            @if active && attention > 0 {
                                span class="ws-badge" { (attention) }
                            }
                        }
                        span class="ws-label" { (p.name) }
                    }
                }
            }
            div class="rail-spacer" {}
            // "+ Add project" pinned bottom — rendered but inert (a later slice wires it).
            div class="rail-add" {
                span class="ws-tile" { "+" }
                span class="ws-label" { "Add project" }
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

    #[test]
    fn initials_multi_word_takes_first_two_initials() {
        assert_eq!(initials("My Project"), "MP");
        assert_eq!(initials("Alpha Beta Gamma"), "AB");
    }

    #[test]
    fn initials_single_word_takes_first_two_letters() {
        assert_eq!(initials("kamaji"), "KA");
        assert_eq!(initials("test"), "TE");
    }

    #[test]
    fn initials_empty_falls_back_to_question_mark() {
        assert_eq!(initials(""), "?");
        assert_eq!(initials("   "), "?");
        assert_eq!(initials("!!!"), "?");
    }

    #[test]
    fn initials_second_word_no_alphanumeric_fills_from_first_word() {
        // 2nd word is all dashes — must fall back to first word for 2nd char.
        assert_eq!(initials("foo ---"), "FO");
    }

    #[test]
    fn initials_hyphenated_single_word_takes_first_two_letters() {
        // split_whitespace sees "foo-bar" as one token → single-word branch.
        assert_eq!(initials("foo-bar"), "FO");
    }

    #[test]
    fn active_tile_gets_ws_active_others_just_ws() {
        let ps = [project(1, "acme"), project(2, "beta")];
        let html = rail(&ps, 1, 0).into_string();
        assert!(html.contains(r#"class="ws active""#), "active ws:\n{html}");
        assert!(html.contains(r#"class="ws""#), "inactive ws:\n{html}");
        // Exactly one active tile — catch "both tiles active" regressions.
        assert_eq!(
            html.matches(r#"class="ws active""#).count(),
            1,
            "exactly one active tile:\n{html}"
        );
    }

    #[test]
    fn no_active_tile_when_active_id_matches_nothing() {
        let ps = [project(1, "acme"), project(2, "beta")];
        // active_id 99 matches neither project.
        let html = rail(&ps, 99, 0).into_string();
        assert_eq!(
            html.matches(r#"class="ws active""#).count(),
            0,
            "zero active tiles when id matches nothing:\n{html}"
        );
    }

    #[test]
    fn badge_renders_only_on_active_tile_when_attention_positive() {
        let ps = [project(1, "acme"), project(2, "beta")];

        // attention 0 → no badge at all
        let none = rail(&ps, 1, 0).into_string();
        assert!(!none.contains("ws-badge"), "no badge at 0:\n{none}");

        // attention 3 on active → badge "3"
        let some = rail(&ps, 1, 3).into_string();
        assert!(
            some.contains(r#"<span class="ws-badge">3</span>"#),
            "badge 3 on active:\n{some}"
        );
        // exactly one badge (the active tile only)
        assert_eq!(some.matches("ws-badge").count(), 1, "single badge:\n{some}");
    }

    #[test]
    fn tiles_navigate_with_colon_binding_no_hyphen() {
        let ps = [project(7, "acme")];
        let html = rail(&ps, 7, 0).into_string();
        assert!(
            html.contains(r#"data-on:click="window.location='/?project=7'""#),
            "colon nav binding:\n{html}"
        );
        assert!(
            !html.contains("data-on-click"),
            "no hyphen binding:\n{html}"
        );
    }

    #[test]
    fn add_project_and_toggle_present() {
        let ps = [project(1, "acme")];
        let html = rail(&ps, 1, 0).into_string();
        assert!(html.contains(r#"class="rail-add""#), "rail-add:\n{html}");
        assert!(html.contains("Add project"), "add label:\n{html}");
        assert!(
            html.contains(r#"data-on:click="document.body.classList.toggle('rail-open')""#),
            "toggle binding:\n{html}"
        );
    }
}
