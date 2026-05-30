use kamaji_core::layout::BarStyle;
use std::path::PathBuf;

/// Resolve the configured `zellij_bar` value into a concrete [`BarStyle`].
///
/// `"compact"`, `"default"` and `"none"` force that style regardless of the
/// user's zellij config. `"auto"` (and any unrecognized value) follows the
/// user's zellij `default_layout`, passed as `detected_layout`: `"compact"`
/// selects the compact bar, anything else (or `None`) keeps the default bars.
pub fn resolve_bar_style(configured: &str, detected_layout: Option<&str>) -> BarStyle {
    match configured.trim().to_ascii_lowercase().as_str() {
        "compact" => BarStyle::Compact,
        "default" => BarStyle::Default,
        "none" => BarStyle::None,
        // "auto" and any unrecognized value fall back to auto-detection.
        _ => match detected_layout.map(str::trim) {
            Some("compact") => BarStyle::Compact,
            _ => BarStyle::Default,
        },
    }
}

/// Scan zellij KDL config text for an uncommented `default_layout "<name>"`
/// node and return its value. Tolerant by design (the issue calls for a
/// lightweight scan, not a full KDL parser): skips `//` and `/-` commented
/// lines, ignores keys that merely share the `default_layout` prefix, and
/// returns `None` when no such node is present.
pub fn parse_default_layout(kdl: &str) -> Option<String> {
    for line in kdl.lines() {
        let line = line.trim_start();
        if line.starts_with("//") || line.starts_with("/-") {
            continue;
        }
        let Some(rest) = line.strip_prefix("default_layout") else {
            continue;
        };
        // Require a delimiter so `default_layout_dir` doesn't match.
        if !rest.starts_with(|c: char| c.is_whitespace() || c == '"') {
            continue;
        }
        let start = rest.find('"')? + 1;
        let end = rest[start..].find('"')? + start;
        return Some(rest[start..end].to_string());
    }
    None
}

/// Path to the user's zellij `config.kdl`, honoring `$ZELLIJ_CONFIG_FILE`, then
/// `$ZELLIJ_CONFIG_DIR`, then `$XDG_CONFIG_HOME`, else `~/.config/zellij`.
/// Returns `None` only when no home directory can be determined.
fn config_file_path() -> Option<PathBuf> {
    if let Some(file) = std::env::var_os("ZELLIJ_CONFIG_FILE") {
        return Some(PathBuf::from(file));
    }
    if let Some(dir) = std::env::var_os("ZELLIJ_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("config.kdl"));
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("zellij").join("config.kdl"));
    }
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("zellij")
            .join("config.kdl"),
    )
}

/// The user's zellij `default_layout`, or `None` if it can't be determined
/// (no/unreadable config file, or no uncommented `default_layout` node). Any
/// failure is treated as "unset", which [`resolve_bar_style`] maps to the
/// default bars.
pub fn detect_default_layout() -> Option<String> {
    let text = std::fs::read_to_string(config_file_path()?).ok()?;
    parse_default_layout(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_layout_value() {
        assert_eq!(
            parse_default_layout("default_layout \"compact\"\n").as_deref(),
            Some("compact")
        );
    }

    #[test]
    fn parse_tolerates_indentation_and_inline_comment() {
        assert_eq!(
            parse_default_layout("    default_layout \"compact\" // my bar\n").as_deref(),
            Some("compact")
        );
    }

    #[test]
    fn parse_ignores_line_comments() {
        assert_eq!(
            parse_default_layout("// default_layout \"compact\"\n"),
            None
        );
        assert_eq!(
            parse_default_layout("/- default_layout \"compact\"\n"),
            None
        );
    }

    #[test]
    fn parse_ignores_keys_that_only_share_a_prefix() {
        assert_eq!(parse_default_layout("default_layout_dir \"/x\"\n"), None);
    }

    #[test]
    fn parse_returns_none_when_absent() {
        assert_eq!(parse_default_layout("theme \"gruvbox\"\n"), None);
        assert_eq!(parse_default_layout(""), None);
    }

    #[test]
    fn resolve_overrides_win_over_detection() {
        assert_eq!(resolve_bar_style("compact", None), BarStyle::Compact);
        assert_eq!(
            resolve_bar_style("default", Some("compact")),
            BarStyle::Default
        );
        assert_eq!(resolve_bar_style("none", Some("compact")), BarStyle::None);
    }

    #[test]
    fn resolve_auto_follows_detected_layout() {
        assert_eq!(
            resolve_bar_style("auto", Some("compact")),
            BarStyle::Compact
        );
        assert_eq!(
            resolve_bar_style("auto", Some("default")),
            BarStyle::Default
        );
        assert_eq!(resolve_bar_style("auto", None), BarStyle::Default);
    }

    #[test]
    fn resolve_unknown_value_behaves_like_auto() {
        assert_eq!(resolve_bar_style("wat", Some("compact")), BarStyle::Compact);
        assert_eq!(resolve_bar_style("", None), BarStyle::Default);
    }

    #[test]
    fn resolve_is_case_and_whitespace_insensitive() {
        assert_eq!(resolve_bar_style("  Compact ", None), BarStyle::Compact);
    }
}
