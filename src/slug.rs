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
}
