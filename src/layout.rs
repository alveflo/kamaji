pub(crate) fn kdl_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Which zellij bar UI a generated layout should render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarStyle {
    /// tab-bar (top) + status-bar (bottom) — zellij 0.43's default layout.
    Default,
    /// A single-line `compact-bar` at the bottom — zellij's compact layout.
    Compact,
    /// No bars at all; just the tab's panes.
    None,
}

/// The `default_tab_template` body for `bar`. zellij clones this template for
/// every newly-created tab, so the agent command must stay out of it (issue
/// #2); each variant therefore wraps `children` (the new tab's plain shell) in
/// the chosen bars. The Default variant mirrors zellij 0.43's built-in default
/// layout, Compact mirrors its compact layout, and None drops the bars while
/// still cloning a plain shell for new tabs.
fn tab_template(bar: BarStyle) -> &'static str {
    match bar {
        BarStyle::Default => {
            "    default_tab_template {
        pane size=1 borderless=true {
            plugin location=\"tab-bar\"
        }
        children
        pane size=1 borderless=true {
            plugin location=\"status-bar\"
        }
    }
"
        }
        BarStyle::Compact => {
            "    default_tab_template {
        children
        pane size=1 borderless=true {
            plugin location=\"compact-bar\"
        }
    }
"
        }
        BarStyle::None => {
            "    default_tab_template {
        children
    }
"
        }
    }
}

/// Render a zellij KDL layout that runs `command` (argv) in `cwd`, drawing the
/// bars selected by `bar`.
///
/// The agent command lives inside an explicit `tab` so it runs only in the
/// first tab; new tabs (Ctrl+T n) inherit the bars-only template and open a
/// plain shell via `children`. `command` must be non-empty.
pub fn render_layout(cwd: &str, command: &[String], bar: BarStyle) -> String {
    let program = kdl_escape(&command[0]);
    let cwd_esc = kdl_escape(cwd);
    let args = &command[1..];
    let pane = if args.is_empty() {
        format!("        pane command=\"{program}\" cwd=\"{cwd_esc}\"\n")
    } else {
        let mut args_line = String::from("            args");
        for a in args {
            args_line.push_str(&format!(" \"{}\"", kdl_escape(a)));
        }
        format!(
            "        pane command=\"{program}\" cwd=\"{cwd_esc}\" {{\n{args_line}\n        }}\n"
        )
    };
    let template = tab_template(bar);
    format!("layout {{\n{template}    tab {{\n{pane}    }}\n}}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_args_layout() {
        let out = render_layout("/wt", &["claude".to_string()], BarStyle::Default);
        assert_eq!(
            out,
            "\
layout {
    default_tab_template {
        pane size=1 borderless=true {
            plugin location=\"tab-bar\"
        }
        children
        pane size=1 borderless=true {
            plugin location=\"status-bar\"
        }
    }
    tab {
        pane command=\"claude\" cwd=\"/wt\"
    }
}
"
        );
    }

    #[test]
    fn with_args_layout() {
        let out = render_layout(
            "/wt",
            &["claude".to_string(), "fix the bug".to_string()],
            BarStyle::Default,
        );
        assert_eq!(
            out,
            "\
layout {
    default_tab_template {
        pane size=1 borderless=true {
            plugin location=\"tab-bar\"
        }
        children
        pane size=1 borderless=true {
            plugin location=\"status-bar\"
        }
    }
    tab {
        pane command=\"claude\" cwd=\"/wt\" {
            args \"fix the bug\"
        }
    }
}
"
        );
    }

    #[test]
    fn escapes_quotes() {
        let out = render_layout("/w\"t", &["claude".to_string()], BarStyle::Default);
        assert!(out.contains("cwd=\"/w\\\"t\""));
    }

    /// Compact style mirrors zellij 0.43's compact layout: a single-line
    /// `compact-bar` below the tab's panes, and no tab-bar/status-bar.
    #[test]
    fn compact_layout_uses_compact_bar() {
        let out = render_layout("/wt", &["claude".to_string()], BarStyle::Compact);
        assert_eq!(
            out,
            "\
layout {
    default_tab_template {
        children
        pane size=1 borderless=true {
            plugin location=\"compact-bar\"
        }
    }
    tab {
        pane command=\"claude\" cwd=\"/wt\"
    }
}
"
        );
    }

    /// None style drops every bar but still keeps a `default_tab_template` whose
    /// only content is `children`, so new tabs open a plain shell (issue #2).
    #[test]
    fn none_layout_has_no_bars() {
        let out = render_layout("/wt", &["claude".to_string()], BarStyle::None);
        assert_eq!(
            out,
            "\
layout {
    default_tab_template {
        children
    }
    tab {
        pane command=\"claude\" cwd=\"/wt\"
    }
}
"
        );
        assert!(!out.contains("plugin location="));
    }

    /// Regression for #2: the agent command must live inside an explicit `tab`,
    /// not the `default_tab_template`. The template is what zellij clones for
    /// each newly-created tab (Ctrl+T n), so if the command leaked into it every
    /// new tab would re-launch the agent with the initial prompt. Holds for
    /// every bar style.
    #[test]
    fn agent_runs_only_in_first_tab() {
        for bar in [BarStyle::Default, BarStyle::Compact, BarStyle::None] {
            let out = render_layout("/wt", &["claude".to_string(), "do it".to_string()], bar);
            let (template, rest) = out
                .split_once("    tab {")
                .expect("layout must contain an explicit tab block");
            assert!(
                template.contains("default_tab_template {"),
                "{bar:?}: expected a default_tab_template so new tabs get a plain shell"
            );
            assert!(
                !template.contains("command="),
                "{bar:?}: agent command leaked into the new-tab template: {template}"
            );
            assert!(
                template.contains("\n        children\n"),
                "{bar:?}: template must have a children placeholder for new tabs' shell"
            );
            assert!(rest.contains("command=\"claude\""));
        }
    }

    /// Regression for #2: a custom layout replaces zellij's default, so the
    /// default style must re-add the tab-bar and status-bar plugins itself or
    /// they vanish.
    #[test]
    fn includes_tab_and_status_bars() {
        let out = render_layout("/wt", &["claude".to_string()], BarStyle::Default);
        assert!(out.contains("plugin location=\"tab-bar\""));
        assert!(out.contains("plugin location=\"status-bar\""));
    }
}
