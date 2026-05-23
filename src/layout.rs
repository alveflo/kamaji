fn kdl_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Render a single-pane zellij KDL layout that runs `command` (argv) in `cwd`.
/// `command` must be non-empty.
pub fn render_layout(cwd: &str, command: &[String]) -> String {
    let program = kdl_escape(&command[0]);
    let cwd_esc = kdl_escape(cwd);
    let args = &command[1..];
    if args.is_empty() {
        format!("layout {{\n    pane command=\"{program}\" cwd=\"{cwd_esc}\"\n}}\n")
    } else {
        let mut args_line = String::from("        args");
        for a in args {
            args_line.push_str(&format!(" \"{}\"", kdl_escape(a)));
        }
        format!(
            "layout {{\n    pane command=\"{program}\" cwd=\"{cwd_esc}\" {{\n{args_line}\n    }}\n}}\n"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_args_layout() {
        let out = render_layout("/wt", &["claude".to_string()]);
        assert_eq!(out, "layout {\n    pane command=\"claude\" cwd=\"/wt\"\n}\n");
    }

    #[test]
    fn with_args_layout() {
        let out = render_layout("/wt", &["claude".to_string(), "fix the bug".to_string()]);
        assert_eq!(
            out,
            "layout {\n    pane command=\"claude\" cwd=\"/wt\" {\n        args \"fix the bug\"\n    }\n}\n"
        );
    }

    #[test]
    fn escapes_quotes() {
        let out = render_layout("/w\"t", &["claude".to_string()]);
        assert!(out.contains("cwd=\"/w\\\"t\""));
    }
}
