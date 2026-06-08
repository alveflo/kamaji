//! Bridges the user's zellij configuration to kamaji's layout generation:
//! resolves the configured `zellij_bar` value into a concrete [`BarStyle`] and
//! scans the user's zellij KDL for its `default_layout` to drive `auto` mode.

use crate::config;
use crate::layout::BarStyle;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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

/// Return `kdl` with web sharing forced on: drop any existing uncommented
/// top-level `web_sharing` node and append a single canonical `web_sharing
/// "on"`. zellij sessions default to `web_sharing "off"` ("do not allow web
/// sharing unless sessions explicitly opt-in"), so a browser client attaching
/// to a default-created session is refused with "Web clients are not allowed to
/// attach to this session". kamaji's sessions exist to be viewed in the browser,
/// so every session it creates must opt in.
///
/// Tolerant by design (same lightweight line scan as [`parse_default_layout`],
/// not a full KDL parse): commented `web_sharing` lines are inert and kept;
/// dropping then re-appending guarantees exactly one active node, avoiding a
/// duplicate-key KDL error.
pub fn ensure_web_sharing_on(kdl: &str) -> String {
    let mut out = String::with_capacity(kdl.len() + 24);
    for line in kdl.lines() {
        let trimmed = line.trim_start();
        let active_web_sharing = !trimmed.starts_with("//")
            && !trimmed.starts_with("/-")
            && trimmed
                .strip_prefix("web_sharing")
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_whitespace() || c == '"'));
        if active_web_sharing {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("web_sharing \"on\"\n");
    out
}

/// Return `kdl` with session serialization forced on: drop any existing
/// uncommented top-level `session_serialization` node and append a single
/// canonical `session_serialization true`. zellij defaults this on, but a user
/// who set `session_serialization false` would silently break kamaji's
/// reboot-persistence: kamaji relies on zellij serializing each session to its
/// cache folder so that, after a restart, the session is listed as resurrectable
/// and `resurrect_session` can relaunch the agent (see the daemon's attach path).
/// Every session kamaji creates therefore forces it on, the same way it forces
/// `web_sharing`.
///
/// Tolerant by design (same lightweight line scan as [`ensure_web_sharing_on`]):
/// commented lines are inert and kept; dropping then re-appending guarantees
/// exactly one active node. `session_serialization` takes a bare KDL boolean
/// (`true`/`false`), so — unlike the string-valued `web_sharing`/`theme` nodes —
/// only a whitespace delimiter is accepted after the key.
pub fn ensure_session_serialization_on(kdl: &str) -> String {
    let mut out = String::with_capacity(kdl.len() + 28);
    for line in kdl.lines() {
        let trimmed = line.trim_start();
        let active_serialization = !trimmed.starts_with("//")
            && !trimmed.starts_with("/-")
            && trimmed
                .strip_prefix("session_serialization")
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_whitespace()));
        if active_serialization {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("session_serialization true\n");
    out
}

/// Resolve the configured `web_theme` value into the zellij theme name to force
/// on browser sessions, or `None` to leave the user's config untouched.
/// `"auto"` (and empty) → `None` (respect the user's file). `"match"` → the
/// built-in `catppuccin-mocha` (the board's palette). Any other value is taken
/// as a literal zellij theme name to force.
fn resolve_web_theme(web_theme: &str) -> Option<&str> {
    match web_theme.trim() {
        "auto" | "" => None,
        "match" => Some("catppuccin-mocha"),
        other => Some(other),
    }
}

/// Return `kdl` with its theme forced to `theme_name`: drop any existing
/// uncommented top-level `theme` node and append a single canonical
/// `theme "<name>"`. Mirrors [`ensure_web_sharing_on`]'s tolerant line scan, and
/// like it requires a delimiter after `theme` so the `themes { … }` block that
/// *defines* custom themes is preserved, not stripped.
pub fn ensure_theme(kdl: &str, theme_name: &str) -> String {
    let mut out = String::with_capacity(kdl.len() + theme_name.len() + 12);
    for line in kdl.lines() {
        let trimmed = line.trim_start();
        let active_theme = !trimmed.starts_with("//")
            && !trimmed.starts_with("/-")
            && trimmed
                .strip_prefix("theme")
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_whitespace() || c == '"'));
        if active_theme {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&format!("theme \"{theme_name}\"\n"));
    out
}

/// Path to a kamaji-managed zellij config that mirrors the user's config but
/// forces `web_sharing "on"` (and, when `web_theme` is not `"auto"`, a theme),
/// for use as the `-c` argument when *creating* sessions (so they are
/// browser-attachable while keeping the user's keybindings etc.). Built once per
/// process: zellij reads web settings only at session/server start ("Requires
/// restart"), so a config change needs a daemon restart to take effect anyway.
/// Returns `None` only if the file can't be written, in which case callers
/// create the session without `-c` (preserving the prior, non-shareable behavior
/// rather than failing the start).
pub fn web_sharing_config_file() -> Option<&'static Path> {
    static CFG: OnceLock<Option<PathBuf>> = OnceLock::new();
    CFG.get_or_init(build_web_sharing_config_file).as_deref()
}

/// Pure derivation of the session config from the user's zellij `base` config and
/// the configured `web_theme`: always forces `web_sharing "on"` and
/// `session_serialization true`, and — unless `web_theme` is `"auto"` — forces
/// the resolved theme onto the chrome (layer 1). `"auto"` leaves the user's own
/// theme untouched. Split out from the IO in [`build_web_sharing_config_file`] so
/// the whole policy is unit-testable.
fn derive_web_config(base: &str, web_theme: &str) -> String {
    let with_sharing = ensure_web_sharing_on(base);
    let with_serialization = ensure_session_serialization_on(&with_sharing);
    match resolve_web_theme(web_theme) {
        Some(theme) => ensure_theme(&with_serialization, theme),
        None => with_serialization,
    }
}

fn build_web_sharing_config_file() -> Option<PathBuf> {
    // Missing/unreadable user config → start from empty, so the derived config is
    // just `web_sharing "on"` (zellij fills the rest with its defaults).
    let base = config_file_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    // Read the browser theme policy from kamaji's own config; any read failure
    // falls back to "auto" (respect the user's zellij theme).
    let web_theme = config::config_path()
        .and_then(|p| config::load_from(&p))
        .map(|c| c.daemon.web_theme)
        .unwrap_or_else(|_| "auto".to_string());
    let derived = derive_web_config(&base, &web_theme);
    let dir = std::env::temp_dir().join("kamaji-zellij");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("web-config.kdl");
    std::fs::write(&path, derived).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Count the active (uncommented) `web_sharing` nodes in some KDL.
    fn active_web_sharing_lines(kdl: &str) -> Vec<&str> {
        kdl.lines()
            .map(str::trim_start)
            .filter(|l| {
                !l.starts_with("//")
                    && !l.starts_with("/-")
                    && l.strip_prefix("web_sharing")
                        .is_some_and(|r| r.starts_with(|c: char| c.is_whitespace() || c == '"'))
            })
            .collect()
    }

    #[test]
    fn web_sharing_added_to_empty_config() {
        assert_eq!(ensure_web_sharing_on(""), "web_sharing \"on\"\n");
    }

    #[test]
    fn web_sharing_replaces_existing_value_leaving_exactly_one() {
        // An explicit `off` must be overridden, with no duplicate node left.
        let out = ensure_web_sharing_on("theme \"gruvbox\"\nweb_sharing \"off\"\n");
        assert_eq!(active_web_sharing_lines(&out), vec!["web_sharing \"on\""]);
        // Unrelated config is preserved.
        assert!(out.contains("theme \"gruvbox\""));
    }

    #[test]
    fn web_sharing_preserves_commented_lines_and_stays_single() {
        // A commented example line is inert and kept; we still add one active node.
        let out = ensure_web_sharing_on("// web_sharing \"on\"\nkeybinds {}\n");
        assert!(out.contains("// web_sharing \"on\""));
        assert!(out.contains("keybinds {}"));
        assert_eq!(active_web_sharing_lines(&out), vec!["web_sharing \"on\""]);
    }

    #[test]
    fn web_sharing_ignores_keys_that_only_share_a_prefix() {
        // `web_sharing_foo` is a different key and must be left untouched.
        let out = ensure_web_sharing_on("web_sharing_foo \"x\"\n");
        assert!(out.contains("web_sharing_foo \"x\""));
        assert_eq!(active_web_sharing_lines(&out), vec!["web_sharing \"on\""]);
    }

    /// Count the active (uncommented) `session_serialization` nodes in some KDL.
    fn active_session_serialization_lines(kdl: &str) -> Vec<&str> {
        kdl.lines()
            .map(str::trim_start)
            .filter(|l| {
                !l.starts_with("//")
                    && !l.starts_with("/-")
                    && l.strip_prefix("session_serialization")
                        .is_some_and(|r| r.starts_with(|c: char| c.is_whitespace()))
            })
            .collect()
    }

    #[test]
    fn session_serialization_added_to_empty_config() {
        assert_eq!(
            ensure_session_serialization_on(""),
            "session_serialization true\n"
        );
    }

    #[test]
    fn session_serialization_replaces_existing_value_leaving_exactly_one() {
        // A user who disabled serialization must be overridden, with no duplicate.
        let out =
            ensure_session_serialization_on("theme \"gruvbox\"\nsession_serialization false\n");
        assert_eq!(
            active_session_serialization_lines(&out),
            vec!["session_serialization true"]
        );
        // Unrelated config is preserved.
        assert!(out.contains("theme \"gruvbox\""));
    }

    #[test]
    fn session_serialization_preserves_commented_lines_and_stays_single() {
        let out = ensure_session_serialization_on("// session_serialization false\nkeybinds {}\n");
        assert!(out.contains("// session_serialization false"));
        assert!(out.contains("keybinds {}"));
        assert_eq!(
            active_session_serialization_lines(&out),
            vec!["session_serialization true"]
        );
    }

    #[test]
    fn session_serialization_ignores_keys_that_only_share_a_prefix() {
        // `session_serialization_foo` is a different key and must be left untouched.
        let out = ensure_session_serialization_on("session_serialization_foo \"x\"\n");
        assert!(out.contains("session_serialization_foo \"x\""));
        assert_eq!(
            active_session_serialization_lines(&out),
            vec!["session_serialization true"]
        );
    }

    #[test]
    fn derive_web_config_always_forces_session_serialization() {
        // Whatever the user's setting (or absence), the derived session config
        // forces serialization on — kamaji's reboot-resurrection contract.
        let from_empty = derive_web_config("", "auto");
        assert_eq!(
            active_session_serialization_lines(&from_empty),
            vec!["session_serialization true"]
        );
        let from_disabled = derive_web_config("session_serialization false\n", "auto");
        assert_eq!(
            active_session_serialization_lines(&from_disabled),
            vec!["session_serialization true"]
        );
    }

    /// Count the active (uncommented) `theme` nodes in some KDL.
    fn active_theme_lines(kdl: &str) -> Vec<&str> {
        kdl.lines()
            .map(str::trim_start)
            .filter(|l| {
                !l.starts_with("//")
                    && !l.starts_with("/-")
                    && l.strip_prefix("theme")
                        .is_some_and(|r| r.starts_with(|c: char| c.is_whitespace() || c == '"'))
            })
            .collect()
    }

    #[test]
    fn resolve_web_theme_maps_auto_match_and_explicit() {
        assert_eq!(
            resolve_web_theme("auto"),
            None,
            "auto respects the user file"
        );
        assert_eq!(resolve_web_theme(""), None, "empty behaves like auto");
        assert_eq!(resolve_web_theme("  auto "), None, "trimmed");
        assert_eq!(resolve_web_theme("match"), Some("catppuccin-mocha"));
        assert_eq!(
            resolve_web_theme("tokyo-night"),
            Some("tokyo-night"),
            "an explicit name is forced verbatim"
        );
    }

    #[test]
    fn ensure_theme_replaces_existing_leaving_exactly_one() {
        // The user's theme is overridden, with no duplicate node left.
        let out = ensure_theme(
            "theme \"tokyo-night\"\nweb_sharing \"on\"\n",
            "catppuccin-mocha",
        );
        assert_eq!(active_theme_lines(&out), vec!["theme \"catppuccin-mocha\""]);
        // Unrelated config is preserved.
        assert!(out.contains("web_sharing \"on\""));
    }

    #[test]
    fn ensure_theme_added_when_absent() {
        let out = ensure_theme("web_sharing \"on\"\n", "catppuccin-mocha");
        assert_eq!(active_theme_lines(&out), vec!["theme \"catppuccin-mocha\""]);
    }

    #[test]
    fn derive_web_config_auto_respects_user_theme() {
        // The default: web sharing is forced, but the user's theme is untouched.
        let out = derive_web_config("theme \"tokyo-night\"\n", "auto");
        assert_eq!(active_web_sharing_lines(&out), vec!["web_sharing \"on\""]);
        assert_eq!(
            active_theme_lines(&out),
            vec!["theme \"tokyo-night\""],
            "auto leaves the user's theme as-is"
        );
    }

    #[test]
    fn derive_web_config_match_forces_board_theme() {
        let out = derive_web_config("theme \"tokyo-night\"\n", "match");
        assert_eq!(active_web_sharing_lines(&out), vec!["web_sharing \"on\""]);
        assert_eq!(
            active_theme_lines(&out),
            vec!["theme \"catppuccin-mocha\""],
            "match overrides the user's theme with the board palette"
        );
    }

    #[test]
    fn derive_web_config_explicit_name_is_forced() {
        let out = derive_web_config("", "gruvbox-dark");
        assert_eq!(active_web_sharing_lines(&out), vec!["web_sharing \"on\""]);
        assert_eq!(active_theme_lines(&out), vec!["theme \"gruvbox-dark\""]);
    }

    #[test]
    fn ensure_theme_preserves_themes_definition_block_and_comments() {
        // `themes { … }` *defines* custom themes — it shares the `theme` prefix but
        // must not be stripped. A commented theme line is inert and kept.
        let kdl = "// theme \"old\"\nthemes {\n    kamaji {}\n}\ntheme \"gruvbox\"\n";
        let out = ensure_theme(kdl, "catppuccin-mocha");
        assert!(out.contains("themes {"), "themes block preserved");
        assert!(out.contains("kamaji {}"), "themes block body preserved");
        assert!(out.contains("// theme \"old\""), "commented line preserved");
        assert_eq!(
            active_theme_lines(&out),
            vec!["theme \"catppuccin-mocha\""],
            "exactly one active theme, the forced one"
        );
    }

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
