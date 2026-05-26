/// Lowercase ASCII slug: alphanumerics kept, runs of anything else become a
/// single '-', trimmed and capped at 40 chars.
pub fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let mut s: String = out.chars().take(40).collect();
    while s.ends_with('-') {
        s.pop();
    }
    s
}

/// Zellij session name for a project's "main" session — a workspace not tied to
/// any ticket. Keyed by the project's (globally unique) id so it never collides
/// across projects, nor with a ticket session (those are `kamaji-<id>-…`, with a
/// number where this has the literal `main`).
pub fn main_session_name(project_id: i64) -> String {
    format!("kamaji-main-{project_id}")
}

/// `kamaji-<id>-<slug>`, or `kamaji-<id>` when the slug is empty.
pub fn ticket_name(id: i64, title: &str) -> String {
    let slug = slugify(title);
    if slug.is_empty() {
        format!("kamaji-{id}")
    } else {
        format!("kamaji-{id}-{slug}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_normalizes() {
        assert_eq!(slugify("Add Login!"), "add-login");
        assert_eq!(slugify("  Refactor   DB  "), "refactor-db");
        assert_eq!(slugify("!!!"), "");
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn slugify_caps_length_without_trailing_dash() {
        let s = slugify(&"a ".repeat(60));
        assert!(s.len() <= 40);
        assert!(!s.ends_with('-'));
    }

    #[test]
    fn ticket_name_formats() {
        assert_eq!(ticket_name(42, "Add Login"), "kamaji-42-add-login");
        assert_eq!(ticket_name(7, "!!!"), "kamaji-7");
    }

    #[test]
    fn main_session_name_is_per_project_and_distinct_from_tickets() {
        assert_eq!(main_session_name(3), "kamaji-main-3");
        // A main session can never collide with a ticket session: ticket names
        // place a numeric id right after `kamaji-`, never the literal `main`.
        assert_ne!(main_session_name(3), ticket_name(3, "main"));
    }
}
