# Robust Per-Agent Activity Detection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Codex hook-instrumented (its native `~/.codex/hooks.json` maintains the idle marker, exactly like Claude) and switch Copilot to a screen-change timeout, retiring the fragile idle-substring screen-scrape path.

**Architecture:** Keep kamaji's existing marker-file + poll-loop model end to end. Only change *how each agent's `SignalLevel` is produced*: Claude + Codex → marker file (instrumented); Copilot → screen-change timeout. `detect::decide`, the poll move logic, and SSE events are unchanged.

**Tech Stack:** Rust (Cargo workspace). `serde_json` for the Codex hooks merge, `directories` for `~/.codex`, `toml`/`serde` for config. All already dependencies of `kamaji-core`.

**Spec:** `docs/superpowers/specs/2026-06-10-robust-agent-activity-detection-design.md`

---

## File structure

| File | Change |
|------|--------|
| `crates/kamaji-core/src/detect.rs` | Add `ScreenChangeState` + `screen_change_level` (Copilot). Add Codex hooks builder/merge/installer (`codex_hook_command`, `codex_managed_entries`, `merge_codex_hooks`, `strip_managed_entry`, `codex_hook_is_managed`, `codex_hooks_path`, `install_codex_hooks`, `install_codex_hooks_at`). Later remove dead `scrape_level`. |
| `crates/kamaji-core/src/config.rs` | Add `auto_review.copilot_idle_secs` + `copilot_idle_after_unchanged()`. Later remove `ScrapePatterns`/`patterns`/`auto_review_patterns`. |
| `crates/kamaji-core/src/session.rs` | Extend instrumentation to Codex (install hooks instead of `--settings`). |
| `crates/kamaji-core/src/poll.rs` | Swap `scrape_hash` → `screen_state`; pick detector per agent in `gather_levels`. |
| Workspace-wide | Remove references to deleted symbols (`auto_review_patterns`, `ScrapePatterns`, `scrape_level`, the `patterns` config UI). |

Ordering keeps the workspace compiling after every task: new code is added first (Tasks 1–6), dead code and its references are removed last (Task 7).

---

### Task 1: Copilot screen-change detector

**Files:**
- Modify: `crates/kamaji-core/src/detect.rs`
- Test: `crates/kamaji-core/src/detect.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/kamaji-core/src/detect.rs`:

```rust
    #[test]
    fn screen_change_changed_screen_is_active() {
        let mut st = ScreenChangeState::default();
        assert_eq!(screen_change_level(Some("a"), &mut st, 2), SignalLevel::Active);
        // Different content => activity, counter resets.
        assert_eq!(screen_change_level(Some("b"), &mut st, 2), SignalLevel::Active);
    }

    #[test]
    fn screen_change_unchanged_below_threshold_is_active() {
        let mut st = ScreenChangeState::default();
        // threshold 2: first sight (count 0) and one repeat (count 1) stay Active.
        assert_eq!(screen_change_level(Some("x"), &mut st, 2), SignalLevel::Active);
        assert_eq!(screen_change_level(Some("x"), &mut st, 2), SignalLevel::Active);
    }

    #[test]
    fn screen_change_unchanged_at_threshold_is_idle() {
        let mut st = ScreenChangeState::default();
        assert_eq!(screen_change_level(Some("x"), &mut st, 2), SignalLevel::Active); // count 0
        assert_eq!(screen_change_level(Some("x"), &mut st, 2), SignalLevel::Active); // count 1
        assert_eq!(screen_change_level(Some("x"), &mut st, 2), SignalLevel::Idle); // count 2
    }

    #[test]
    fn screen_change_threshold_of_one_idles_on_first_repeat() {
        let mut st = ScreenChangeState::default();
        assert_eq!(screen_change_level(Some("x"), &mut st, 1), SignalLevel::Active); // count 0
        assert_eq!(screen_change_level(Some("x"), &mut st, 1), SignalLevel::Idle); // count 1
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
        assert_eq!(screen_change_level(Some("x"), &mut st, 1), SignalLevel::Idle); // count 1
        // New content => back to Active.
        assert_eq!(screen_change_level(Some("y"), &mut st, 1), SignalLevel::Active);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kamaji-core screen_change`
Expected: FAIL — `cannot find type ScreenChangeState` / `cannot find function screen_change_level`.

- [ ] **Step 3: Write the implementation**

Add to `crates/kamaji-core/src/detect.rs`, right after the `marker_level` function (it shares the hashing idiom with the soon-to-be-removed `scrape_level`):

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kamaji-core screen_change`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/kamaji-core/src/detect.rs
git commit -m "feat(core): screen-change idle detector for Copilot"
```

---

### Task 2: Codex hooks JSON builder + merge (pure)

**Files:**
- Modify: `crates/kamaji-core/src/detect.rs`
- Test: `crates/kamaji-core/src/detect.rs` (inline `#[cfg(test)]`)

This task is pure (no IO): build kamaji's managed hook entries and merge them into an existing parsed `serde_json::Value`. The Codex `hooks.json` shape (confirmed against slayzone's `codex-hook-installer.ts`) is:
`{"hooks": {"<Event>": [ {"matcher"?: "...", "hooks": [ {"type":"command","command":"...","_kamajiManaged":true} ]} ]}}`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/kamaji-core/src/detect.rs`:

```rust
    fn hooks_of<'a>(v: &'a serde_json::Value, event: &str) -> &'a Vec<serde_json::Value> {
        v["hooks"][event].as_array().unwrap()
    }

    #[test]
    fn merge_fresh_wires_all_four_events() {
        let merged = merge_codex_hooks(serde_json::json!({}), "/s/state");
        for event in ["UserPromptSubmit", "PreToolUse", "Stop", "PermissionRequest"] {
            assert_eq!(hooks_of(&merged, event).len(), 1, "event {event}");
        }
        // Active events run `rm -f`; idle events `touch`.
        let cmd = |e: &str| hooks_of(&merged, e)[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .to_string();
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
        assert_eq!(hooks_of(&merged, "Stop")[0]["hooks"][0]["_kamajiManaged"], true);
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
        for event in ["UserPromptSubmit", "PreToolUse", "Stop", "PermissionRequest"] {
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kamaji-core merge_`
Expected: FAIL — `cannot find function merge_codex_hooks`.

- [ ] **Step 3: Write the implementation**

Add to `crates/kamaji-core/src/detect.rs` (after the new `screen_change_level`). Add `use serde_json::{json, Value};` near the top of the file if not present (the file already uses serde derive but check the imports — add the line if missing):

```rust
/// Marker key stamped on kamaji-managed Codex hook entries so re-installs
/// replace only our entries and never touch the user's own hooks.
const CODEX_MARKER_KEY: &str = "_kamajiManaged";

/// The four Codex hook events kamaji wires, and which marker op each performs.
/// Active events clear the marker (agent working); idle events create it.
const CODEX_ACTIVE_EVENTS: [&str; 2] = ["UserPromptSubmit", "PreToolUse"];
const CODEX_IDLE_EVENTS: [&str; 2] = ["Stop", "PermissionRequest"];

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
    let mut entry = json!({
        "hooks": [ { "type": "command", "command": command, CODEX_MARKER_KEY: true } ]
    });
    if let Some(m) = matcher {
        entry
            .as_object_mut()
            .expect("entry is an object")
            .insert("matcher".to_string(), json!(m));
    }
    entry
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
        let matcher = if event == "PreToolUse" { Some(".*") } else { None };
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kamaji-core merge_`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/kamaji-core/src/detect.rs
git commit -m "feat(core): build + merge kamaji-managed Codex hook entries"
```

---

### Task 3: Codex hooks installer (IO)

**Files:**
- Modify: `crates/kamaji-core/src/detect.rs`
- Test: `crates/kamaji-core/src/detect.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/kamaji-core/src/detect.rs`:

```rust
    #[test]
    fn install_creates_fresh_hooks_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("hooks.json");
        install_codex_hooks_at(&path, "/s/state").unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn install_is_idempotent_no_duplicate_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        install_codex_hooks_at(&path, "/s").unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        install_codex_hooks_at(&path, "/s").unwrap();
        let second = std::fs::read_to_string(&path).unwrap();
        assert_eq!(first, second, "second install must not change the file");
        let v: serde_json::Value = serde_json::from_str(&second).unwrap();
        assert_eq!(v["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn install_preserves_existing_user_hook() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        std::fs::write(
            &path,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
        )
        .unwrap();
        install_codex_hooks_at(&path, "/s").unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "echo hi");
    }

    #[test]
    fn install_empty_file_is_treated_as_empty_object() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        std::fs::write(&path, "   \n").unwrap();
        install_codex_hooks_at(&path, "/s").unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(v["hooks"]["Stop"].is_array());
    }

    #[test]
    fn install_aborts_on_invalid_json_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        std::fs::write(&path, "not json {").unwrap();
        assert!(install_codex_hooks_at(&path, "/s").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json {");
    }

    #[test]
    fn install_aborts_on_non_object_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        std::fs::write(&path, "[1,2,3]").unwrap();
        assert!(install_codex_hooks_at(&path, "/s").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[1,2,3]");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kamaji-core install_`
Expected: FAIL — `cannot find function install_codex_hooks_at`.

- [ ] **Step 3: Write the implementation**

Add to `crates/kamaji-core/src/detect.rs`. Note `anyhow::{bail, Result}` — add the import line `use anyhow::{bail, Result};` at the top of the file if not already present:

```rust
/// Path to the user's global Codex hooks file (`~/.codex/hooks.json`).
/// `KAMAJI_CODEX_HOOKS_PATH` overrides it (used by tests to avoid touching the
/// real home dir). `None` if no home directory can be determined.
pub fn codex_hooks_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("KAMAJI_CODEX_HOOKS_PATH") {
        return Some(PathBuf::from(p));
    }
    directories::BaseDirs::new().map(|b| b.home_dir().join(".codex").join("hooks.json"))
}

/// Idempotently merge kamaji's managed hook entries into the user's global
/// `~/.codex/hooks.json`. Resolves the path via [`codex_hooks_path`].
pub fn install_codex_hooks(state_dir: &Path) -> Result<()> {
    let path = codex_hooks_path()
        .ok_or_else(|| anyhow::anyhow!("cannot determine ~/.codex/hooks.json path"))?;
    install_codex_hooks_at(&path, &state_dir.to_string_lossy())
}

/// [`install_codex_hooks`] against an explicit path. Reads the existing file
/// (missing or whitespace-only => empty object), **aborts without writing** if
/// it is present but not a JSON object (never destroy a file kamaji can't
/// parse), merges, and writes back only when the content actually changed.
pub fn install_codex_hooks_at(path: &Path, state_dir: &str) -> Result<()> {
    let existing = match std::fs::read_to_string(path) {
        Ok(raw) if raw.trim().is_empty() => Value::Object(serde_json::Map::new()),
        Ok(raw) => {
            let v: Value = serde_json::from_str(&raw).map_err(|e| {
                anyhow::anyhow!(
                    "{} is not valid JSON, refusing to overwrite: {e}",
                    path.display()
                )
            })?;
            if !v.is_object() {
                bail!("{} is not a JSON object, refusing to overwrite", path.display());
            }
            v
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Object(serde_json::Map::new()),
        Err(e) => return Err(e.into()),
    };
    let merged = merge_codex_hooks(existing, state_dir);
    let body = serde_json::to_string_pretty(&merged)? + "\n";
    if std::fs::read_to_string(path).ok().as_deref() != Some(body.as_str()) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, body)?;
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kamaji-core install_`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/kamaji-core/src/detect.rs
git commit -m "feat(core): idempotent ~/.codex/hooks.json installer"
```

---

### Task 4: Config — add `copilot_idle_secs` (keep old patterns for now)

**Files:**
- Modify: `crates/kamaji-core/src/config.rs`
- Test: `crates/kamaji-core/src/config.rs` (inline `#[cfg(test)]`)

Do **not** remove `ScrapePatterns`/`patterns`/`auto_review_patterns` yet — `poll.rs` still uses them until Task 6. This task only *adds* the new field and helper.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/kamaji-core/src/config.rs`:

```rust
    #[test]
    fn copilot_idle_secs_defaults_to_eight() {
        let c = Config::default();
        assert_eq!(c.auto_review.copilot_idle_secs, 8);
    }

    #[test]
    fn copilot_idle_after_unchanged_is_polls_to_cover_the_window() {
        let mut c = Config::default();
        c.auto_review.poll_interval_secs = 5;
        c.auto_review.copilot_idle_secs = 8; // ceil(8/5) = 2
        assert_eq!(c.copilot_idle_after_unchanged(), 2);
        c.auto_review.copilot_idle_secs = 5; // ceil(5/5) = 1
        assert_eq!(c.copilot_idle_after_unchanged(), 1);
        c.auto_review.copilot_idle_secs = 3; // ceil(3/5) = 1 (floor of 1)
        assert_eq!(c.copilot_idle_after_unchanged(), 1);
        c.auto_review.copilot_idle_secs = 11; // ceil(11/5) = 3
        assert_eq!(c.copilot_idle_after_unchanged(), 3);
    }

    #[test]
    fn config_missing_copilot_idle_secs_defaults() {
        // A config predating the key still loads, defaulting to 8.
        let text = "default_agent = \"claude\"\nbase_branch = \"auto\"\n\
             [agents.claude]\nwith_prompt = [\"claude\", \"{prompt}\"]\nno_prompt = [\"claude\"]\n\
             [agents.codex]\nwith_prompt = [\"codex\", \"{prompt}\"]\nno_prompt = [\"codex\"]\n\
             [agents.copilot]\nwith_prompt = [\"copilot\", \"{prompt}\"]\nno_prompt = [\"copilot\"]\n\
             [auto_review]\nenabled = true\npoll_interval_secs = 5\n";
        let loaded: Config = toml::from_str(text).unwrap();
        assert_eq!(loaded.auto_review.copilot_idle_secs, 8);
    }

    #[test]
    fn config_with_legacy_patterns_table_still_loads() {
        // Old configs carrying [auto_review.patterns] must still load (serde
        // ignores it once the field is gone; for now it just deserializes).
        let text = "default_agent = \"claude\"\nbase_branch = \"auto\"\n\
             [agents.claude]\nwith_prompt = [\"claude\", \"{prompt}\"]\nno_prompt = [\"claude\"]\n\
             [agents.codex]\nwith_prompt = [\"codex\", \"{prompt}\"]\nno_prompt = [\"codex\"]\n\
             [agents.copilot]\nwith_prompt = [\"copilot\", \"{prompt}\"]\nno_prompt = [\"copilot\"]\n\
             [auto_review.patterns]\ncodex = [\"x\"]\ncopilot = [\"y\"]\n";
        assert!(toml::from_str::<Config>(text).is_ok());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kamaji-core copilot_idle`
Expected: FAIL — no field `copilot_idle_secs`.

- [ ] **Step 3: Write the implementation**

In `crates/kamaji-core/src/config.rs`, add a default fn near the other defaults (after `default_poll_interval`):

```rust
fn default_copilot_idle_secs() -> u64 {
    8
}
```

Add the field to `AutoReview` (keep `patterns` for now):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoReview {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    /// Seconds a Copilot session's screen may stay byte-for-byte unchanged
    /// before it is considered idle. Quantized to whole polls by
    /// [`Config::copilot_idle_after_unchanged`].
    #[serde(default = "default_copilot_idle_secs")]
    pub copilot_idle_secs: u64,
    #[serde(default)]
    pub patterns: ScrapePatterns,
}
```

Update its `Default`:

```rust
impl Default for AutoReview {
    fn default() -> Self {
        AutoReview {
            enabled: true,
            poll_interval_secs: 5,
            copilot_idle_secs: default_copilot_idle_secs(),
            patterns: ScrapePatterns::default(),
        }
    }
}
```

Add the helper to the `impl Config` block (near `poll_interval`):

```rust
    /// Consecutive unchanged polls before the Copilot screen-change detector
    /// declares idle: `ceil(copilot_idle_secs / poll_interval_secs)`, at least 1.
    /// Computed without `div_ceil` to avoid a toolchain floor.
    pub fn copilot_idle_after_unchanged(&self) -> u32 {
        let interval = self.auto_review.poll_interval_secs.max(1);
        let polls = (self.auto_review.copilot_idle_secs + interval - 1) / interval;
        polls.max(1).min(u32::MAX as u64) as u32
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kamaji-core -- config`
Expected: PASS (existing config tests + 4 new ones). The existing `auto_review_defaults_on` still passes (it asserts `patterns` is empty, which is still true).

- [ ] **Step 5: Commit**

```bash
git add crates/kamaji-core/src/config.rs
git commit -m "feat(core): config copilot_idle_secs + polls-to-idle helper"
```

---

### Task 5: Wire Codex instrumentation in session prepare

**Files:**
- Modify: `crates/kamaji-core/src/session.rs:81-89`
- Test: `crates/kamaji-core/src/session.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/kamaji-core/src/session.rs`. It builds a real temp git repo (mirrors `cleanup_ticket_removes_worktree_and_clears_session`) and redirects the Codex hooks file via `KAMAJI_CODEX_HOOKS_PATH` so the real `~/.codex` is never touched:

```rust
    #[test]
    fn prepare_session_instruments_codex_and_installs_hooks() {
        // Real git repo so `worktree add` has a base.
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        let run = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.email", "t@t.t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("README.md"), "hi").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "init"]);

        let hooks_dir = tempfile::tempdir().unwrap();
        let hooks_path = hooks_dir.path().join("hooks.json");
        std::env::set_var("KAMAJI_CODEX_HOOKS_PATH", &hooks_path);

        let wt = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.worktree_base = Some(wt.path().join("wt").to_string_lossy().to_string());

        let db = Db::open_in_memory().unwrap();
        let project = db.create_project("p", root, None).unwrap();
        let ticket = db
            .create_ticket(project.id, "codex task", "", None, Agent::Codex)
            .unwrap();

        let state_dir = tempfile::tempdir().unwrap();
        let prepared =
            prepare_session(&project, &config, state_dir.path(), &ticket).unwrap();

        assert!(prepared.instrumented, "codex sessions must be instrumented");
        assert!(hooks_path.exists(), "codex hooks file should be installed");
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&hooks_path).unwrap()).unwrap();
        assert!(v["hooks"]["Stop"].is_array());

        std::env::remove_var("KAMAJI_CODEX_HOOKS_PATH");
    }
```

Note: `prepare_session` takes `&Project`, so this test builds a `Project` from the DB (`create_project` returns one). If `create_project`'s signature differs, adapt the construction — the goal is a `Project` whose `root_dir` is the git repo.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p kamaji-core prepare_session_instruments_codex`
Expected: FAIL — `prepared.instrumented` is `false` (Codex not yet instrumented).

- [ ] **Step 3: Write the implementation**

In `crates/kamaji-core/src/session.rs`, replace the instrumentation block (currently lines 81-89):

```rust
    let instrumented = config.auto_review.enabled && ticket.agent == Agent::Claude;
    let argv = if instrumented {
        let marker = detect::marker_path(state_dir, &name);
        let _ = std::fs::create_dir_all(state_dir);
        let _ = std::fs::remove_file(&marker);
        detect::inject_claude_settings(argv, &marker.to_string_lossy())
    } else {
        argv
    };
```

with:

```rust
    let instrumented =
        config.auto_review.enabled && matches!(ticket.agent, Agent::Claude | Agent::Codex);
    let argv = if instrumented {
        let marker = detect::marker_path(state_dir, &name);
        let _ = std::fs::create_dir_all(state_dir);
        let _ = std::fs::remove_file(&marker); // start "active": no marker
        match ticket.agent {
            // Claude takes per-invocation hooks via --settings.
            Agent::Claude => detect::inject_claude_settings(argv, &marker.to_string_lossy()),
            // Codex has no per-invocation hook flag; install its global
            // hooks.json (idempotent). The hook derives the same marker path
            // from $ZELLIJ_SESSION_NAME, so argv is unchanged.
            Agent::Codex => {
                detect::install_codex_hooks(state_dir)?;
                argv
            }
            _ => argv,
        }
    } else {
        argv
    };
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p kamaji-core prepare_session_instruments_codex`
Expected: PASS. Also run `cargo test -p kamaji-core -- session` to confirm no regression.

- [ ] **Step 5: Commit**

```bash
git add crates/kamaji-core/src/session.rs
git commit -m "feat(core): instrument Codex sessions via ~/.codex/hooks.json"
```

---

### Task 6: Poll loop — pick the detector per agent

**Files:**
- Modify: `crates/kamaji-core/src/poll.rs` (struct field, `forget_ticket`, `gather_levels`)
- Test: existing `poll.rs` tests must keep passing (they drive `apply` with crafted levels, so they are detector-agnostic).

- [ ] **Step 1: Update the `PollLoop` state field**

In `crates/kamaji-core/src/poll.rs`, replace the `scrape_hash` field:

```rust
    /// Per-ticket scrape screen hash for the scrape detector's stability guard.
    scrape_hash: HashMap<i64, Option<u64>>,
```

with:

```rust
    /// Per-ticket Copilot screen-change detector state (last screen hash +
    /// consecutive-unchanged count).
    screen_state: HashMap<i64, detect::ScreenChangeState>,
```

In `forget_ticket`, replace `self.scrape_hash.remove(&id);` with `self.screen_state.remove(&id);`.

- [ ] **Step 2: Rewrite the detector selection in `gather_levels`**

Replace the `let level = match agent { ... };` block (currently lines 176-193) with:

```rust
            let level = if instrumented {
                // Claude + Codex: their own hooks maintain the idle marker.
                detect::marker_level(&detect::marker_path(state_dir, &session))
            } else if agent == Agent::Copilot && config.auto_review.enabled {
                // Copilot: screen-change timeout (no usable hooks; TUI is too
                // noisy to pattern-match).
                let screen = zellij::dump_screen(&session);
                let st = self.screen_state.entry(id).or_default();
                detect::screen_change_level(
                    screen.as_deref(),
                    st,
                    config.copilot_idle_after_unchanged(),
                )
            } else {
                // Auto-review disabled, or an un-instrumented Claude/Codex:
                // no trustworthy signal this poll.
                SignalLevel::Unknown
            };
```

The surrounding `for (id, agent, session, instrumented) in live { ... out.insert(id, level); }` and the exited-session short-circuit above it are unchanged.

- [ ] **Step 3: Run the poll tests to verify they pass**

Run: `cargo test -p kamaji-core -- poll`
Expected: PASS (all existing poll tests; they use `apply` with crafted levels and don't touch the field name).

- [ ] **Step 4: Build the whole crate to confirm it compiles**

Run: `cargo build -p kamaji-core`
Expected: builds. (`scrape_level` and `auto_review_patterns` are now unused — a dead-code/unused warning is fine; Task 7 removes them.)

- [ ] **Step 5: Commit**

```bash
git add crates/kamaji-core/src/poll.rs
git commit -m "feat(core): poll loop drives Codex via marker, Copilot via screen-change"
```

---

### Task 7: Remove the dead scrape path and update all references

**Files:**
- Modify: `crates/kamaji-core/src/detect.rs` (remove `scrape_level` + its 6 tests)
- Modify: `crates/kamaji-core/src/config.rs` (remove `ScrapePatterns`, the `patterns` field, `auto_review_patterns`, and the patterns assertions/tests)
- Modify: any other workspace file that referenced the removed symbols (found by grep)

- [ ] **Step 1: Find every reference to the symbols being removed**

Run:
```bash
grep -rn "scrape_level\|auto_review_patterns\|ScrapePatterns\|\.patterns" crates/
```
Expected references: `detect.rs` (`scrape_level` + tests), `config.rs` (`ScrapePatterns`, `patterns`, `auto_review_patterns`, tests). **Also inspect `crates/kamajid` and `crates/kamaji`** for any config UI/form that reads or writes `auto_review.patterns` (e.g. a web config editor field or a TUI config screen). Every hit must be removed or updated in this task so the workspace builds.

- [ ] **Step 2: Remove `scrape_level` and its tests from `detect.rs`**

Delete the `scrape_level` function (the `/// Scrape detector...` doc + fn) and these tests: `scrape_idle_requires_match_and_stability`, `scrape_changed_screen_is_active`, `scrape_no_match_is_active`, `scrape_empty_patterns_never_idle`, `scrape_failed_dump_is_unknown`. (`json_escape`, `claude_settings_json`, `inject_claude_settings`, `marker_*` stay.)

- [ ] **Step 3: Remove the patterns config from `config.rs`**

- Delete the `ScrapePatterns` struct.
- Delete the `patterns: ScrapePatterns` field from `AutoReview` and from its `Default` impl.
- Delete the `auto_review_patterns` method.
- Delete the `patterns_lookup_by_agent` test.
- In `auto_review_defaults_on`, delete the two `assert!(...patterns...is_empty())` lines.

`config_with_legacy_patterns_table_still_loads` (added in Task 4) MUST still pass — serde ignores the now-unknown `[auto_review.patterns]` table, so old configs keep loading. Keep that test.

- [ ] **Step 4: Update any kamajid/kamaji references found in Step 1**

For each hit outside `kamaji-core`, remove the dead field/form control. If a web config form or TUI screen exposed `auto_review.patterns`, delete that control and any code reading it. (If Step 1 found none outside core, note that and skip.)

- [ ] **Step 5: Build + test the whole workspace**

Run:
```bash
cargo build
cargo test -p kamaji-core
```
Expected: clean build (no unused-code warnings for the removed items), all `kamaji-core` tests pass.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(core): retire idle-substring scrape path (patterns config + scrape_level)"
```

---

### Task 8: Verify, lint, format, and document

**Files:**
- Modify: `ARCHITECTURE.md` and/or config doc comments if they describe the old scrape/patterns behavior

- [ ] **Step 1: Format**

Run: `cargo fmt`
Then `git diff --stat` — review any reformatting.

- [ ] **Step 2: Clippy (match CI: deny warnings)**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings. Fix any that appear (e.g. needless clones in the merge code).

- [ ] **Step 3: Full test suite**

Run: `cargo test`
Expected: all tests pass across the workspace.

- [ ] **Step 4: Update docs**

Grep docs for stale descriptions: `grep -rni "scrape\|idle substring\|patterns" ARCHITECTURE.md docs/ | grep -vi superpowers`. Update `ARCHITECTURE.md` (and any config sample/comment) so the auto-review description reads: Claude **and Codex** are hook-instrumented (idle marker maintained by the agent's own hooks); Copilot uses a screen-change timeout (`auto_review.copilot_idle_secs`). Do **not** edit files under `docs/superpowers/` (historical record).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: fmt, clippy, and doc the new Codex/Copilot detection"
```

---

## Final review (whole branch)

- [ ] Run `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`.
- [ ] Manual acceptance after `make restart` (daemon picks up new code):
  - **Codex:** start a Codex ticket; it stays In Progress while working and auto-moves to Needs attention when it stops. Confirm `~/.codex/hooks.json` has the four kamaji-managed entries and any prior user hooks are intact.
  - **Copilot:** start a Copilot ticket; Active while it works, Idle (→ Needs attention) after ~10s of a static screen.
- [ ] Open a PR per the repo workflow (`gh pr create --fill --base main`).

## Spec-coverage check

- Codex hook-instrumented (marker via `~/.codex/hooks.json`, `$ZELLIJ_SESSION_NAME`-derived, `kamaji-*` guarded): Tasks 2, 3, 5. ✓
- Codex idempotent safe merge, abort on unparsable file: Tasks 2, 3 (O1 resolved: abort, never overwrite). ✓
- Codex hooks.json schema verified vs slayzone reference (O2): event names + nested `{matcher?, hooks:[...]}` shape encoded in Task 2; Task 8 clippy/build confirms it serializes. ✓
- Install timing in shared `prepare_with_argv` (O3): Task 5; idempotent merge + write-only-on-change makes concurrent Codex starts safe. ✓
- Copilot screen-change timeout, drop substrings: Tasks 1, 6, 7. ✓
- `copilot_idle_secs` config + quantization: Task 4. ✓
- `instrumented = true` for Codex: Task 5. ✓
- Old configs still load (leftover `patterns` table): Tasks 4, 7. ✓
- Claude path unchanged; `decide` unchanged: untouched, regression-guarded by existing tests. ✓
