use crate::models::Status;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// What a detector believes about an agent session right now. Serializes as a
/// lowercase string (`"idle"`/`"active"`/`"unknown"`) so it can ride the SSE
/// `session.signal` event to clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    crate::paths::data_dir()
        .map(|d| d.join("state"))
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

/// Per-session state for the Copilot screen-change detector, held across polls.
#[derive(Default)]
pub struct ScreenChangeState {
    last_hash: Option<u64>,
    unchanged_count: u32,
}

/// Screen-change detector (Copilot). kamaji's daemon can't see keystrokes or
/// raw PTY output — only `zellij dump-screen` each poll — so "is it working?"
/// is inferred from whether the screen moves: a working TUI redraws (spinner,
/// streaming output), a finished or input-blocked one is static. A byte-for-byte
/// identical screen for `idle_after_unchanged` consecutive polls => `Idle`; any
/// change => `Active`. A failed dump (`None`) => `Unknown` (never moves a ticket)
/// and leaves state untouched, so a transient blank screen can't force idle.
pub fn screen_change_level(
    screen: Option<&str>,
    state: &mut ScreenChangeState,
    idle_after_unchanged: u32,
) -> SignalLevel {
    let Some(screen) = screen else {
        return SignalLevel::Unknown;
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    screen.hash(&mut hasher);
    let hash = hasher.finish();
    if state.last_hash == Some(hash) {
        state.unchanged_count = state.unchanged_count.saturating_add(1);
    } else {
        state.last_hash = Some(hash);
        state.unchanged_count = 0;
    }
    if state.unchanged_count >= idle_after_unchanged.max(1) {
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

/// Splice `--settings <json>` after `argv[0]` (a global claude flag, before the
/// positional prompt). The session preparation path validates that `argv` is
/// non-empty before calling this helper.
pub fn inject_claude_settings(argv: Vec<String>, marker_path: &str) -> Vec<String> {
    let json = claude_settings_json(marker_path);
    let mut out = Vec::with_capacity(argv.len() + 2);
    out.push(argv[0].clone());
    out.push("--settings".to_string());
    out.push(json);
    out.extend_from_slice(&argv[1..]);
    out
}

/// Marker key stamped on kamaji-managed Codex hook entries so re-installs
/// replace only our entries and never touch the user's own hooks.
const CODEX_MARKER_KEY: &str = "_kamajiManaged";

/// Codex hook events signalling the agent is active (clear the idle marker).
const CODEX_ACTIVE_EVENTS: [&str; 2] = ["UserPromptSubmit", "PreToolUse"];
/// Codex hook events signalling the agent is idle (create the idle marker).
const CODEX_IDLE_EVENTS: [&str; 2] = ["Stop", "PermissionRequest"];
/// The one tool-scoped Codex event; it needs a `.*` matcher to fire for every tool.
const CODEX_TOOL_EVENT: &str = "PreToolUse";

/// Shell command a Codex hook runs to set/clear the idle marker. `op` is
/// `"touch"` (idle) or `"rm -f"` (active). The marker path is derived at run
/// time from `$ZELLIJ_SESSION_NAME` — which Codex's hook subprocess inherits
/// from its pane — so one global hooks file serves every kamaji Codex session.
/// The `kamaji-*` guard keeps the global hook inert for the user's own
/// (non-kamaji) Codex sessions and for any non-zellij run (no session name).
/// `state_dir` is assumed free of single quotes (it is an XDG path; the Claude
/// path makes the same assumption for its marker).
fn codex_hook_command(state_dir: &str, op: &str) -> String {
    format!(
        "sh -c 'case \"$ZELLIJ_SESSION_NAME\" in kamaji-*) {op} \"{state_dir}/$ZELLIJ_SESSION_NAME.idle\";; esac'"
    )
}

/// One kamaji-managed Codex hook entry: `{matcher?, hooks:[{type,command,marker}]}`.
fn codex_managed_entry(command: &str, matcher: Option<&str>) -> Value {
    let mut obj = serde_json::Map::new();
    if let Some(m) = matcher {
        obj.insert("matcher".to_string(), json!(m));
    }
    obj.insert(
        "hooks".to_string(),
        json!([ { "type": "command", "command": command, CODEX_MARKER_KEY: true } ]),
    );
    Value::Object(obj)
}

/// kamaji's managed entries keyed by event: active events (`UserPromptSubmit`,
/// `PreToolUse`) clear the marker; idle events (`Stop`, `PermissionRequest`)
/// create it. Tool-scoped `PreToolUse` carries a `.*` matcher (fire for every
/// tool); lifecycle events take none.
fn codex_managed_entries(state_dir: &str) -> Vec<(&'static str, Value)> {
    let active = codex_hook_command(state_dir, "rm -f");
    let idle = codex_hook_command(state_dir, "touch");
    let mut out = Vec::new();
    for event in CODEX_ACTIVE_EVENTS {
        let matcher = if event == CODEX_TOOL_EVENT {
            Some(".*")
        } else {
            None
        };
        out.push((event, codex_managed_entry(&active, matcher)));
    }
    for event in CODEX_IDLE_EVENTS {
        out.push((event, codex_managed_entry(&idle, None)));
    }
    out
}

/// Is this inner-hook object kamaji-managed (carries the marker key == true)?
fn codex_hook_is_managed(hook: &Value) -> bool {
    hook.get(CODEX_MARKER_KEY).and_then(|v| v.as_bool()) == Some(true)
}

/// Strip kamaji-managed inner hooks from one entry. Returns the entry with the
/// remaining user hooks, or `None` if nothing user-defined is left (so an
/// entry that was kamaji-only disappears instead of lingering empty).
fn strip_managed_entry(entry: Value) -> Option<Value> {
    let mut obj = entry.as_object()?.clone();
    let inner = obj.get("hooks")?.as_array()?.clone();
    let kept: Vec<Value> = inner
        .into_iter()
        .filter(|h| !codex_hook_is_managed(h))
        .collect();
    if kept.is_empty() {
        return None;
    }
    obj.insert("hooks".to_string(), Value::Array(kept));
    Some(Value::Object(obj))
}

/// Merge kamaji's managed hook entries into an existing parsed hooks file,
/// preserving user-defined hooks and unrelated keys. Idempotent: any prior
/// kamaji-managed entry is stripped before the current one is appended.
fn merge_codex_hooks(existing: Value, state_dir: &str) -> Value {
    let mut root = match existing {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    let mut hooks = match root.remove("hooks") {
        Some(Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    };
    for (event, managed) in codex_managed_entries(state_dir) {
        let prior = match hooks.remove(event) {
            Some(Value::Array(a)) => a,
            _ => Vec::new(),
        };
        let mut entries: Vec<Value> = prior.into_iter().filter_map(strip_managed_entry).collect();
        entries.push(managed);
        hooks.insert(event.to_string(), Value::Array(entries));
    }
    root.insert("hooks".to_string(), Value::Object(hooks));
    Value::Object(root)
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

    #[test]
    fn screen_change_changed_screen_is_active() {
        let mut st = ScreenChangeState::default();
        assert_eq!(
            screen_change_level(Some("a"), &mut st, 2),
            SignalLevel::Active
        );
        // Different content => activity, counter resets.
        assert_eq!(
            screen_change_level(Some("b"), &mut st, 2),
            SignalLevel::Active
        );
    }

    #[test]
    fn screen_change_unchanged_below_threshold_is_active() {
        let mut st = ScreenChangeState::default();
        // threshold 2: first sight (count 0) and one repeat (count 1) stay Active.
        assert_eq!(
            screen_change_level(Some("x"), &mut st, 2),
            SignalLevel::Active
        );
        assert_eq!(
            screen_change_level(Some("x"), &mut st, 2),
            SignalLevel::Active
        );
    }

    #[test]
    fn screen_change_unchanged_at_threshold_is_idle() {
        let mut st = ScreenChangeState::default();
        assert_eq!(
            screen_change_level(Some("x"), &mut st, 2),
            SignalLevel::Active
        ); // count 0
        assert_eq!(
            screen_change_level(Some("x"), &mut st, 2),
            SignalLevel::Active
        ); // count 1
        assert_eq!(
            screen_change_level(Some("x"), &mut st, 2),
            SignalLevel::Idle
        ); // count 2
    }

    #[test]
    fn screen_change_threshold_of_one_idles_on_first_repeat() {
        let mut st = ScreenChangeState::default();
        assert_eq!(
            screen_change_level(Some("x"), &mut st, 1),
            SignalLevel::Active
        ); // count 0
        assert_eq!(
            screen_change_level(Some("x"), &mut st, 1),
            SignalLevel::Idle
        ); // count 1
    }

    #[test]
    fn screen_change_failed_dump_is_unknown() {
        let mut st = ScreenChangeState::default();
        assert_eq!(screen_change_level(None, &mut st, 2), SignalLevel::Unknown);
    }

    #[test]
    fn screen_change_reactivates_after_idle_when_screen_moves() {
        let mut st = ScreenChangeState::default();
        screen_change_level(Some("x"), &mut st, 1); // Active, count 0
        assert_eq!(
            screen_change_level(Some("x"), &mut st, 1),
            SignalLevel::Idle
        ); // count 1
           // New content => back to Active.
        assert_eq!(
            screen_change_level(Some("y"), &mut st, 1),
            SignalLevel::Active
        );
    }

    fn hooks_of<'a>(v: &'a serde_json::Value, event: &str) -> &'a Vec<serde_json::Value> {
        v["hooks"][event].as_array().unwrap()
    }

    #[test]
    fn merge_fresh_wires_all_four_events() {
        let merged = merge_codex_hooks(serde_json::json!({}), "/s/state");
        for event in [
            "UserPromptSubmit",
            "PreToolUse",
            "Stop",
            "PermissionRequest",
        ] {
            assert_eq!(hooks_of(&merged, event).len(), 1, "event {event}");
        }
        // Active events run `rm -f`; idle events `touch`.
        let cmd = |e: &str| {
            hooks_of(&merged, e)[0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert!(cmd("UserPromptSubmit").contains("rm -f"));
        assert!(cmd("PreToolUse").contains("rm -f"));
        assert!(cmd("Stop").contains("touch"));
        assert!(cmd("PermissionRequest").contains("touch"));
    }

    #[test]
    fn merge_command_derives_marker_from_session_with_guard() {
        let merged = merge_codex_hooks(serde_json::json!({}), "/s/state");
        let cmd = hooks_of(&merged, "Stop")[0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(cmd.contains("case \"$ZELLIJ_SESSION_NAME\" in kamaji-*"));
        assert!(cmd.contains("/s/state/$ZELLIJ_SESSION_NAME.idle"));
    }

    #[test]
    fn merge_pretooluse_has_wildcard_matcher() {
        let merged = merge_codex_hooks(serde_json::json!({}), "/s");
        assert_eq!(hooks_of(&merged, "PreToolUse")[0]["matcher"], ".*");
        // Lifecycle events take no matcher.
        assert!(hooks_of(&merged, "Stop")[0].get("matcher").is_none());
    }

    #[test]
    fn merge_marks_entries_kamaji_managed() {
        let merged = merge_codex_hooks(serde_json::json!({}), "/s");
        assert_eq!(
            hooks_of(&merged, "Stop")[0]["hooks"][0]["_kamajiManaged"],
            true
        );
    }

    #[test]
    fn merge_preserves_user_hooks() {
        let existing = serde_json::json!({
            "hooks": {
                "Stop": [ { "hooks": [ { "type": "command", "command": "echo user" } ] } ]
            }
        });
        let merged = merge_codex_hooks(existing, "/s");
        let stop = hooks_of(&merged, "Stop");
        // User entry preserved, kamaji entry appended.
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "echo user");
        assert_eq!(stop[1]["hooks"][0]["_kamajiManaged"], true);
    }

    #[test]
    fn merge_is_idempotent() {
        let once = merge_codex_hooks(serde_json::json!({}), "/s");
        let twice = merge_codex_hooks(once.clone(), "/s");
        // Re-merging strips the prior kamaji entry before re-adding: still one each.
        for event in [
            "UserPromptSubmit",
            "PreToolUse",
            "Stop",
            "PermissionRequest",
        ] {
            assert_eq!(hooks_of(&twice, event).len(), 1, "event {event}");
        }
        assert_eq!(once, twice);
    }

    #[test]
    fn merge_preserves_unrelated_top_level_keys() {
        let existing = serde_json::json!({ "other": 42, "hooks": {} });
        let merged = merge_codex_hooks(existing, "/s");
        assert_eq!(merged["other"], 42);
    }
}
