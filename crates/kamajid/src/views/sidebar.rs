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
                span class="rail-mark" {
                    svg width="20" height="20" viewBox="0 0 24 24" fill="none"
                        stroke="#fff" stroke-width="2.2" stroke-linecap="round"
                        stroke-linejoin="round" {
                        rect x="3" y="3" width="7" height="8" rx="2" {}
                        rect x="14" y="3" width="7" height="13" rx="2" {}
                        rect x="3" y="14" width="7" height="7" rx="2" {}
                    }
                }
                span class="rail-word" { "kamaji" }
                button class="rail-toggle" aria-label="Toggle sidebar"
                       data-on:click="document.body.classList.toggle('rail-open')" {
                    span class="ic-collapsed" { "☰" }
                    span class="ic-open" { "‹" }
                }
            }
            div class="rail-label" { "Projects" }
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
            // Footer actions pinned at the bottom of the rail.
            div class="rail-foot" {
                // "+ Add project" — opens the new-project modal by morphing the
                // `#modal` mount with the `/ui/projects/new` fragment.
                div class="rail-add" data-on:click="@get('/ui/projects/new')" {
                    span class="ws-tile" { "+" }
                    span class="ws-label" { "Add project" }
                }
                // Light/dark theme toggle — calls the global wired by theme.js.
                // The shown glyph/label is the theme you'd switch TO (CSS-driven).
                div class="rail-theme" aria-label="Toggle theme"
                    data-on:click="window.__kamajiToggleTheme()" {
                    span class="rail-glyph" {
                        span class="theme-to-dark" {
                            svg width="19" height="19" viewBox="0 0 24 24" fill="none"
                                stroke="currentColor" stroke-width="2.1" stroke-linecap="round"
                                stroke-linejoin="round" {
                                path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" {}
                            }
                        }
                        span class="theme-to-light" {
                            svg width="19" height="19" viewBox="0 0 24 24" fill="none"
                                stroke="currentColor" stroke-width="2.1" stroke-linecap="round"
                                stroke-linejoin="round" {
                                circle cx="12" cy="12" r="4.5" {}
                                path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" {}
                            }
                        }
                    }
                    span class="ws-label" {
                        span class="theme-to-dark" { "Dark theme" }
                        span class="theme-to-light" { "Light theme" }
                    }
                }
                // Gear — opens the config-editor modal by morphing the `#modal`
                // mount with the `/ui/config` fragment.
                div class="rail-settings" data-on:click="@get('/ui/config')" {
                    span class="rail-glyph" {
                        svg width="19" height="19" viewBox="0 0 24 24" fill="none"
                            stroke="currentColor" stroke-width="2.1" stroke-linecap="round"
                            stroke-linejoin="round" {
                            circle cx="12" cy="12" r="3" {}
                            path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" {}
                        }
                    }
                    span class="ws-label" { "Settings" }
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
    fn settings_gear_present_and_opens_config_modal() {
        let ps = [project(1, "acme")];
        let html = rail(&ps, 1, 0).into_string();
        assert!(
            html.contains(r#"class="rail-settings""#),
            "settings row present:\n{html}"
        );
        assert!(
            html.contains(r#"data-on:click="@get('/ui/config')""#),
            "gear opens the config modal:\n{html}"
        );
        assert!(html.contains("Settings"), "settings label:\n{html}");
    }

    #[test]
    fn theme_toggle_present_and_calls_global() {
        let ps = [project(1, "acme")];
        let html = rail(&ps, 1, 0).into_string();
        assert!(
            html.contains(r#"class="rail-theme""#),
            "theme toggle row present:\n{html}"
        );
        assert!(
            html.contains(r#"data-on:click="window.__kamajiToggleTheme()""#),
            "theme toggle calls the global wired by theme.js:\n{html}"
        );
    }

    #[test]
    fn add_project_and_toggle_present() {
        let ps = [project(1, "acme")];
        let html = rail(&ps, 1, 0).into_string();
        assert!(html.contains(r#"class="rail-add""#), "rail-add:\n{html}");
        assert!(html.contains("Add project"), "add label:\n{html}");
        assert!(
            html.contains(r#"data-on:click="@get('/ui/projects/new')""#),
            "add-project opens the new-project modal:\n{html}"
        );
        assert!(
            html.contains(r#"data-on:click="document.body.classList.toggle('rail-open')""#),
            "toggle binding:\n{html}"
        );
    }
}
