use directories::ProjectDirs;
use kamaji_core::models::Status;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// What a detector believes about an agent session right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalLevel {
    /// Agent is waiting for user input (finished, or needs permission).
    Idle,
    /// Agent is actively working.
    Active,
    /// No information this poll (e.g. screen dump failed). Never moves a ticket.
    Unknown,
}

/// Pure, edge-triggered move decision. Returns the column to move to, or `None`.
///
/// - First observation (`last == None`) only establishes a baseline: no move.
/// - `Active -> Idle` while In Progress  => move to Review.
/// - `Idle -> Active` while in Review AND kamaji auto-moved it => move to In Progress.
/// - `Unknown` current level never moves anything.
pub fn decide(
    last: Option<SignalLevel>,
    current: SignalLevel,
    status: Status,
    was_auto_reviewed: bool,
) -> Option<Status> {
    if current == SignalLevel::Unknown {
        return None;
    }
    let last = last?;
    match (last, current) {
        (SignalLevel::Active, SignalLevel::Idle) if status == Status::InProgress => {
            Some(Status::Review)
        }
        (SignalLevel::Idle, SignalLevel::Active)
            if status == Status::Review && was_auto_reviewed =>
        {
            Some(Status::InProgress)
        }
        _ => None,
    }
}

/// Directory holding per-session idle markers (XDG data dir; temp fallback).
pub fn default_state_dir() -> PathBuf {
    ProjectDirs::from("", "", "kamaji")
        .map(|d| d.data_dir().join("state"))
        .unwrap_or_else(|| std::env::temp_dir().join("kamaji").join("state"))
}

/// Absolute marker path for a session.
pub fn marker_path(state_dir: &Path, session: &str) -> PathBuf {
    state_dir.join(format!("{session}.idle"))
}

/// Claude detector: marker present => Idle, absent => Active. Absence is
/// meaningful (the agent is working), so this never returns Unknown.
pub fn marker_level(path: &Path) -> SignalLevel {
    if path.exists() {
        SignalLevel::Idle
    } else {
        SignalLevel::Active
    }
}

/// Scrape detector. `Idle` only when the buffer matches a configured idle
/// substring AND is unchanged since the previous poll (stability guard).
/// `None` screen (dump failed) => Unknown. Empty patterns => never Idle.
/// `last_hash` is updated in place so the next poll can detect change.
pub fn scrape_level(
    screen: Option<&str>,
    idle_substrings: &[String],
    last_hash: &mut Option<u64>,
) -> SignalLevel {
    let Some(screen) = screen else {
        return SignalLevel::Unknown;
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    screen.hash(&mut hasher);
    let hash = hasher.finish();
    let stable = *last_hash == Some(hash);
    *last_hash = Some(hash);

    let matches =
        !idle_substrings.is_empty() && idle_substrings.iter().any(|p| screen.contains(p.as_str()));
    if matches && stable {
        SignalLevel::Idle
    } else {
        SignalLevel::Active
    }
}

/// Minimal JSON string-body escaper (enough for shell command strings).
pub fn json_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

/// Claude settings JSON whose hooks maintain the idle marker at `marker_path`.
/// Stop/Notification create it (idle); UserPromptSubmit/PreToolUse remove it
/// (active). `marker_path` is single-quoted for the shell; kamaji session names
/// are slugs, so the path contains no single quotes.
pub fn claude_settings_json(marker_path: &str) -> String {
    let touch = json_escape(&format!("touch '{marker_path}'"));
    let rm = json_escape(&format!("rm -f '{marker_path}'"));
    let cmd = |c: &str| format!("[{{\"hooks\":[{{\"type\":\"command\",\"command\":\"{c}\"}}]}}]");
    format!(
        "{{\"hooks\":{{\"Stop\":{stop},\"Notification\":{notif},\"UserPromptSubmit\":{ups},\"PreToolUse\":{ptu}}}}}",
        stop = cmd(&touch),
        notif = cmd(&touch),
        ups = cmd(&rm),
        ptu = cmd(&rm),
    )
}

/// Splice `--settings <json>` after argv[0] (a global claude flag, before the
/// positional prompt). `argv` must be non-empty (build_command guarantees it).
pub fn inject_claude_settings(argv: Vec<String>, marker_path: &str) -> Vec<String> {
    let json = claude_settings_json(marker_path);
    let mut out = Vec::with_capacity(argv.len() + 2);
    out.push(argv[0].clone());
    out.push("--settings".to_string());
    out.push(json);
    out.extend_from_slice(&argv[1..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_observation_is_baseline_only() {
        assert_eq!(
            decide(None, SignalLevel::Idle, Status::InProgress, false),
            None
        );
    }

    #[test]
    fn finished_in_progress_moves_to_review() {
        assert_eq!(
            decide(
                Some(SignalLevel::Active),
                SignalLevel::Idle,
                Status::InProgress,
                false
            ),
            Some(Status::Review)
        );
    }

    #[test]
    fn resumed_auto_reviewed_card_moves_back() {
        assert_eq!(
            decide(
                Some(SignalLevel::Idle),
                SignalLevel::Active,
                Status::Review,
                true
            ),
            Some(Status::InProgress)
        );
    }

    #[test]
    fn never_drags_manually_placed_review_card() {
        assert_eq!(
            decide(
                Some(SignalLevel::Idle),
                SignalLevel::Active,
                Status::Review,
                false
            ),
            None
        );
    }

    #[test]
    fn no_move_without_a_transition() {
        assert_eq!(
            decide(
                Some(SignalLevel::Idle),
                SignalLevel::Idle,
                Status::InProgress,
                false
            ),
            None
        );
        assert_eq!(
            decide(
                Some(SignalLevel::Active),
                SignalLevel::Active,
                Status::Review,
                true
            ),
            None
        );
    }

    #[test]
    fn unknown_never_moves() {
        assert_eq!(
            decide(
                Some(SignalLevel::Active),
                SignalLevel::Unknown,
                Status::InProgress,
                false
            ),
            None
        );
    }

    #[test]
    fn idle_while_already_in_review_does_not_move() {
        assert_eq!(
            decide(
                Some(SignalLevel::Active),
                SignalLevel::Idle,
                Status::Review,
                true
            ),
            None
        );
    }

    #[test]
    fn marker_path_is_session_dot_idle() {
        let p = marker_path(std::path::Path::new("/var/state"), "kamaji-1-x");
        assert_eq!(p, std::path::PathBuf::from("/var/state/kamaji-1-x.idle"));
    }

    #[test]
    fn marker_present_is_idle_absent_is_active() {
        let dir = tempfile::tempdir().unwrap();
        let p = marker_path(dir.path(), "s");
        assert_eq!(marker_level(&p), SignalLevel::Active); // absent
        std::fs::write(&p, "").unwrap();
        assert_eq!(marker_level(&p), SignalLevel::Idle); // present
    }

    #[test]
    fn scrape_idle_requires_match_and_stability() {
        let pats = vec!["waiting for input".to_string()];
        let mut h: Option<u64> = None;
        let screen = "...\nwaiting for input\n";
        // First sight of a matching screen: not yet stable => Active.
        assert_eq!(
            scrape_level(Some(screen), &pats, &mut h),
            SignalLevel::Active
        );
        // Unchanged + still matching => Idle.
        assert_eq!(scrape_level(Some(screen), &pats, &mut h), SignalLevel::Idle);
    }

    #[test]
    fn scrape_changed_screen_is_active() {
        let pats = vec!["waiting".to_string()];
        let mut h: Option<u64> = None;
        assert_eq!(
            scrape_level(Some("waiting a"), &pats, &mut h),
            SignalLevel::Active
        );
        assert_eq!(
            scrape_level(Some("waiting b"), &pats, &mut h),
            SignalLevel::Active
        );
    }

    #[test]
    fn scrape_no_match_is_active() {
        let pats = vec!["waiting".to_string()];
        let mut h: Option<u64> = None;
        assert_eq!(
            scrape_level(Some("nvim"), &pats, &mut h),
            SignalLevel::Active
        );
        assert_eq!(
            scrape_level(Some("nvim"), &pats, &mut h),
            SignalLevel::Active
        );
    }

    #[test]
    fn scrape_empty_patterns_never_idle() {
        let pats: Vec<String> = vec![];
        let mut h: Option<u64> = None;
        assert_eq!(
            scrape_level(Some("anything"), &pats, &mut h),
            SignalLevel::Active
        );
        assert_eq!(
            scrape_level(Some("anything"), &pats, &mut h),
            SignalLevel::Active
        );
    }

    #[test]
    fn scrape_failed_dump_is_unknown() {
        let pats = vec!["x".to_string()];
        let mut h: Option<u64> = None;
        assert_eq!(scrape_level(None, &pats, &mut h), SignalLevel::Unknown);
    }

    #[test]
    fn settings_json_wires_all_four_hooks() {
        let j = claude_settings_json("/s/kamaji-1-x.idle");
        assert!(j.contains("\"Stop\""));
        assert!(j.contains("\"Notification\""));
        assert!(j.contains("\"UserPromptSubmit\""));
        assert!(j.contains("\"PreToolUse\""));
        assert!(j.contains("touch '/s/kamaji-1-x.idle'"));
        assert!(j.contains("rm -f '/s/kamaji-1-x.idle'"));
    }

    #[test]
    fn json_escape_escapes_quotes_and_backslashes() {
        assert_eq!(json_escape("a\"b\\c"), "a\\\"b\\\\c");
    }

    #[test]
    fn inject_puts_settings_after_program_before_prompt() {
        let argv = vec!["claude".to_string(), "do it".to_string()];
        let out = inject_claude_settings(argv, "/s/m.idle");
        assert_eq!(out[0], "claude");
        assert_eq!(out[1], "--settings");
        assert!(out[2].contains("\"Stop\""));
        assert_eq!(out[3], "do it");
    }

    #[test]
    fn inject_handles_no_prompt_argv() {
        let argv = vec!["claude".to_string()];
        let out = inject_claude_settings(argv, "/s/m.idle");
        assert_eq!(out.len(), 3);
        assert_eq!(out[1], "--settings");
    }
}
