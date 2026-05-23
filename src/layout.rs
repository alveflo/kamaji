fn kdl_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A `default_tab_template` mirroring zellij 0.43's built-in default layout: a
/// tab-bar on top, the tab's own panes via `children`, and a status-bar below.
/// Because a custom layout replaces zellij's default, we must re-add these bars
/// ourselves, and the template (not a bare top-level pane) is what zellij clones
/// for each newly-created tab — keeping the agent out of it (issue #2).
const TAB_TEMPLATE: &str = "    default_tab_template {
        pane size=1 borderless=true {
            plugin location=\"tab-bar\"
        }
        children
        pane size=1 borderless=true {
            plugin location=\"status-bar\"
        }
    }
";

/// Render a zellij KDL layout that runs `command` (argv) in `cwd`.
///
/// The agent command lives inside an explicit `tab` so it runs only in the
/// first tab; new tabs (Ctrl+T n) inherit the bars-only template and open a
/// plain shell via `children`. `command` must be non-empty.
pub fn render_layout(cwd: &str, command: &[String]) -> String {
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
    format!("layout {{\n{TAB_TEMPLATE}    tab {{\n{pane}    }}\n}}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_args_layout() {
        let out = render_layout("/wt", &["claude".to_string()]);
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
        let out = render_layout("/wt", &["claude".to_string(), "fix the bug".to_string()]);
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
        let out = render_layout("/w\"t", &["claude".to_string()]);
        assert!(out.contains("cwd=\"/w\\\"t\""));
    }

    /// Regression for #2: the agent command must live inside an explicit `tab`,
    /// not the `default_tab_template`. The template is what zellij clones for
    /// each newly-created tab (Ctrl+T n), so if the command leaked into it every
    /// new tab would re-launch the agent with the initial prompt.
    #[test]
    fn agent_runs_only_in_first_tab() {
        let out = render_layout("/wt", &["claude".to_string(), "do it".to_string()]);
        let (template, rest) = out
            .split_once("    tab {")
            .expect("layout must contain an explicit tab block");
        assert!(
            template.contains("default_tab_template {"),
            "expected a default_tab_template so new tabs get a plain shell"
        );
        assert!(
            !template.contains("command="),
            "agent command leaked into the new-tab template: {template}"
        );
        assert!(
            template.contains("\n        children\n"),
            "template must have a children placeholder for new tabs' shell"
        );
        assert!(rest.contains("command=\"claude\""));
    }

    /// Regression for #2: a custom layout replaces zellij's default, so we must
    /// re-add the tab-bar and status-bar plugins ourselves or they vanish.
    #[test]
    fn includes_tab_and_status_bars() {
        let out = render_layout("/wt", &["claude".to_string()]);
        assert!(out.contains("plugin location=\"tab-bar\""));
        assert!(out.contains("plugin location=\"status-bar\""));
    }
}
