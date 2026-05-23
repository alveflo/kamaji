# Auto-move tickets to Review Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Auto-move a live ticket In Progress ↔ Review based on whether its agent is idle/waiting for input, without fighting manual moves.

**Architecture:** A synchronous detection tick in the board loop reads a per-agent `SignalLevel` (Idle/Active/Unknown). Claude reports via a launch-injected hook marker file; Codex/Copilot via screen scraping. A pure, edge-triggered `decide()` function turns level *transitions* into column moves, gated by an in-memory provenance set so manually-placed cards are never dragged.

**Tech Stack:** Rust, ratatui, rusqlite, serde/toml, anyhow. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-23-auto-move-to-review-design.md`

---

## File Structure

- **Create `src/detect.rs`** — detection primitives: `SignalLevel`, the pure `decide()` transition function, marker-file helpers, scrape helper, and the Claude `--settings` JSON/argv injection helpers. Pure and unit-tested; no engine coupling.
- **Modify `src/config.rs`** — `AutoReview` config (enabled, poll interval, per-agent scrape patterns), all serde-defaulted; pattern + interval accessors.
- **Modify `src/zellij.rs`** — `dump_screen(session)` to capture a background session's focused pane.
- **Modify `src/engine.rs`** — in-memory detection state, `detect_tick`, marker reset + argv injection in `start_session`, marker/state cleanup, provenance clear on manual move.
- **Modify `src/main.rs`** — declare `mod detect;` and add the poll cadence to `run_board`.

---

## Task 1: AutoReview configuration

**Files:**
- Modify: `src/config.rs`

- [ ] **Step 1: Write failing tests**

Add to the `tests` module in `src/config.rs`:

```rust
    #[test]
    fn auto_review_defaults_on() {
        let c = Config::default();
        assert!(c.auto_review.enabled);
        assert_eq!(c.auto_review.poll_interval_secs, 5);
        assert!(c.auto_review.patterns.codex.is_empty());
        assert!(c.auto_review.patterns.copilot.is_empty());
        assert_eq!(c.poll_interval(), std::time::Duration::from_secs(5));
    }

    #[test]
    fn patterns_lookup_by_agent() {
        let mut c = Config::default();
        c.auto_review.patterns.codex = vec!["▌".into()];
        assert_eq!(c.auto_review_patterns(Agent::Codex), &["▌".to_string()]);
        assert!(c.auto_review_patterns(Agent::Claude).is_empty());
        assert!(c.auto_review_patterns(Agent::Copilot).is_empty());
    }

    #[test]
    fn config_without_auto_review_section_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // A pre-feature config file: no [auto_review] table at all.
        std::fs::write(
            &path,
            "default_agent = \"claude\"\nworktree_base = \"{root}/../wt\"\nbase_branch = \"auto\"\n\
             [agents.claude]\nwith_prompt = [\"claude\", \"{prompt}\"]\nno_prompt = [\"claude\"]\n\
             [agents.codex]\nwith_prompt = [\"codex\", \"{prompt}\"]\nno_prompt = [\"codex\"]\n\
             [agents.copilot]\nwith_prompt = [\"copilot\", \"{prompt}\"]\nno_prompt = [\"copilot\"]\n",
        )
        .unwrap();
        let loaded = load_from(&path).unwrap();
        assert!(loaded.auto_review.enabled);
        assert_eq!(loaded.auto_review.poll_interval_secs, 5);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib config 2>&1 | tail -20`
Expected: compile error — `auto_review`, `poll_interval`, `auto_review_patterns` do not exist.

- [ ] **Step 3: Add the config types and accessors**

In `src/config.rs`, add these structs above `pub struct Config` (after `Agents`):

```rust
fn default_true() -> bool {
    true
}
fn default_poll_interval() -> u64 {
    5
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScrapePatterns {
    #[serde(default)]
    pub codex: Vec<String>,
    #[serde(default)]
    pub copilot: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoReview {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default)]
    pub patterns: ScrapePatterns,
}

impl Default for AutoReview {
    fn default() -> Self {
        AutoReview {
            enabled: true,
            poll_interval_secs: 5,
            patterns: ScrapePatterns::default(),
        }
    }
}
```

Add the field to `Config` (after `pub agents: Agents,`):

```rust
    #[serde(default)]
    pub auto_review: AutoReview,
```

Add `auto_review: AutoReview::default(),` to the struct literal returned by `impl Default for Config`.

Add these methods inside `impl Config` (after `worktree_dir`):

```rust
    /// Scrape idle-substrings for `agent`. Claude uses launch-injected hooks
    /// instead of scraping, so it has none.
    pub fn auto_review_patterns(&self, agent: Agent) -> &[String] {
        match agent {
            Agent::Codex => &self.auto_review.patterns.codex,
            Agent::Copilot => &self.auto_review.patterns.copilot,
            Agent::Claude => &[],
        }
    }

    /// Detection cadence; clamped to at least 1s so it can never busy-loop.
    pub fn poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.auto_review.poll_interval_secs.max(1))
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib config 2>&1 | tail -20`
Expected: PASS (all config tests, including the three new ones).

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): AutoReview settings (enabled, interval, scrape patterns)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `SignalLevel` and the pure `decide()` transition function

**Files:**
- Create: `src/detect.rs`
- Modify: `src/main.rs` (declare module)

- [ ] **Step 1: Create the module with types and a failing test**

Create `src/detect.rs`:

```rust
use crate::models::Status;

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
}
```

In `src/main.rs`, add `mod detect;` to the module list (keep alphabetical order: after `mod db;`, before `mod engine;`).

- [ ] **Step 2: Run the test to verify it fails, then compiles**

Run: `cargo test --lib detect 2>&1 | tail -20`
Expected: the test compiles and PASSES (it asserts the baseline rule). If it does not compile, fix before proceeding.

- [ ] **Step 3: Add the exhaustive transition tests**

Append to the `tests` module in `src/detect.rs`:

```rust
    #[test]
    fn finished_in_progress_moves_to_review() {
        assert_eq!(
            decide(Some(SignalLevel::Active), SignalLevel::Idle, Status::InProgress, false),
            Some(Status::Review)
        );
    }

    #[test]
    fn resumed_auto_reviewed_card_moves_back() {
        assert_eq!(
            decide(Some(SignalLevel::Idle), SignalLevel::Active, Status::Review, true),
            Some(Status::InProgress)
        );
    }

    #[test]
    fn never_drags_manually_placed_review_card() {
        // Same Idle->Active transition, but provenance is false: no move.
        assert_eq!(
            decide(Some(SignalLevel::Idle), SignalLevel::Active, Status::Review, false),
            None
        );
    }

    #[test]
    fn no_move_without_a_transition() {
        assert_eq!(
            decide(Some(SignalLevel::Idle), SignalLevel::Idle, Status::InProgress, false),
            None
        );
        assert_eq!(
            decide(Some(SignalLevel::Active), SignalLevel::Active, Status::Review, true),
            None
        );
    }

    #[test]
    fn unknown_never_moves() {
        assert_eq!(
            decide(Some(SignalLevel::Active), SignalLevel::Unknown, Status::InProgress, false),
            None
        );
    }

    #[test]
    fn idle_while_already_in_review_does_not_move() {
        // Forward rule only fires from In Progress.
        assert_eq!(
            decide(Some(SignalLevel::Active), SignalLevel::Idle, Status::Review, true),
            None
        );
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib detect 2>&1 | tail -20`
Expected: PASS (7 detect tests).

- [ ] **Step 5: Commit**

```bash
git add src/detect.rs src/main.rs
git commit -m "feat(detect): SignalLevel and edge-triggered decide()

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Claude marker-file detector + state dir

**Files:**
- Modify: `src/detect.rs`

- [ ] **Step 1: Write failing tests**

Append to the `tests` module in `src/detect.rs`:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib detect 2>&1 | tail -20`
Expected: compile error — `marker_path` / `marker_level` not found.

- [ ] **Step 3: Implement marker helpers**

Add to the top of `src/detect.rs` (after the `use crate::models::Status;` line):

```rust
use directories::ProjectDirs;
use std::path::{Path, PathBuf};
```

Add these functions (above the `tests` module):

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib detect 2>&1 | tail -20`
Expected: PASS (9 detect tests).

- [ ] **Step 5: Commit**

```bash
git add src/detect.rs
git commit -m "feat(detect): claude marker-file detector and state dir

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Scrape detector (stability + substring)

**Files:**
- Modify: `src/detect.rs`

- [ ] **Step 1: Write failing tests**

Append to the `tests` module in `src/detect.rs`:

```rust
    #[test]
    fn scrape_idle_requires_match_and_stability() {
        let pats = vec!["waiting for input".to_string()];
        let mut h: Option<u64> = None;
        let screen = "...\nwaiting for input\n";
        // First sight of a matching screen: not yet stable => Active.
        assert_eq!(scrape_level(Some(screen), &pats, &mut h), SignalLevel::Active);
        // Unchanged + still matching => Idle.
        assert_eq!(scrape_level(Some(screen), &pats, &mut h), SignalLevel::Idle);
    }

    #[test]
    fn scrape_changed_screen_is_active() {
        let pats = vec!["waiting".to_string()];
        let mut h: Option<u64> = None;
        assert_eq!(scrape_level(Some("waiting a"), &pats, &mut h), SignalLevel::Active);
        assert_eq!(scrape_level(Some("waiting b"), &pats, &mut h), SignalLevel::Active);
    }

    #[test]
    fn scrape_no_match_is_active() {
        let pats = vec!["waiting".to_string()];
        let mut h: Option<u64> = None;
        assert_eq!(scrape_level(Some("nvim"), &pats, &mut h), SignalLevel::Active);
        assert_eq!(scrape_level(Some("nvim"), &pats, &mut h), SignalLevel::Active);
    }

    #[test]
    fn scrape_empty_patterns_never_idle() {
        let pats: Vec<String> = vec![];
        let mut h: Option<u64> = None;
        assert_eq!(scrape_level(Some("anything"), &pats, &mut h), SignalLevel::Active);
        assert_eq!(scrape_level(Some("anything"), &pats, &mut h), SignalLevel::Active);
    }

    #[test]
    fn scrape_failed_dump_is_unknown() {
        let pats = vec!["x".to_string()];
        let mut h: Option<u64> = None;
        assert_eq!(scrape_level(None, &pats, &mut h), SignalLevel::Unknown);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib detect 2>&1 | tail -20`
Expected: compile error — `scrape_level` not found.

- [ ] **Step 3: Implement scrape detector**

Add this `use` to the top of `src/detect.rs`:

```rust
use std::hash::{Hash, Hasher};
```

Add this function (above the `tests` module):

```rust
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

    let matches = !idle_substrings.is_empty()
        && idle_substrings.iter().any(|p| screen.contains(p.as_str()));
    if matches && stable {
        SignalLevel::Idle
    } else {
        SignalLevel::Active
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib detect 2>&1 | tail -20`
Expected: PASS (14 detect tests).

- [ ] **Step 5: Commit**

```bash
git add src/detect.rs
git commit -m "feat(detect): scrape detector with stability + substring match

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Claude `--settings` JSON + argv injection

**Files:**
- Modify: `src/detect.rs`

- [ ] **Step 1: Write failing tests**

Append to the `tests` module in `src/detect.rs`:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib detect 2>&1 | tail -20`
Expected: compile error — `claude_settings_json` / `json_escape` / `inject_claude_settings` not found.

- [ ] **Step 3: Implement JSON + injection**

Add these functions to `src/detect.rs` (above the `tests` module):

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib detect 2>&1 | tail -20`
Expected: PASS (18 detect tests).

- [ ] **Step 5: Verify the generated JSON actually parses (manual, one-off)**

Run:
```bash
cat > /tmp/check_settings.rs <<'EOF'
// scratch: print the JSON so we can eyeball / pipe to a validator
EOF
cargo test --lib detect::tests::settings_json_wires_all_four_hooks -- --nocapture 2>&1 | tail -5
```
Then sanity-check by hand that the format string has balanced braces. (Full live verification happens in Task 9's manual step.)

- [ ] **Step 6: Commit**

```bash
git add src/detect.rs
git commit -m "feat(detect): claude --settings hook JSON + argv injection

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: `zellij::dump_screen`

**Files:**
- Modify: `src/zellij.rs`

- [ ] **Step 1: Add the function (no unit test — it shells out to zellij)**

Add to `src/zellij.rs` (after `attach_session`):

```rust
/// Capture the focused pane of a (possibly background) session. Returns `None`
/// if zellij isn't reachable or the dump fails, so callers treat it as "no
/// information". `dump-screen` writes to a file, which we read then delete.
pub fn dump_screen(session: &str) -> Option<String> {
    let tmp = std::env::temp_dir().join(format!("kamaji-dump-{session}.txt"));
    let status = Command::new("zellij")
        .args(["--session", session, "action", "dump-screen"])
        .arg(&tmp)
        .status()
        .ok()?;
    let result = if status.success() {
        std::fs::read_to_string(&tmp).ok()
    } else {
        None
    };
    let _ = std::fs::remove_file(&tmp);
    result
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build 2>&1 | tail -20`
Expected: builds (a `dead_code` warning for `dump_screen` is fine until Task 8 wires it).

- [ ] **Step 3: Commit**

```bash
git add src/zellij.rs
git commit -m "feat(zellij): dump_screen to capture a background session's pane

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Engine detection state + `detect_tick_with` (apply seam) + provenance clear

**Files:**
- Modify: `src/engine.rs`

- [ ] **Step 1: Add state fields and the apply seam**

At the top of `src/engine.rs`, extend imports:

```rust
use std::collections::{HashMap, HashSet};
```
and change the models import to include `Agent`:
```rust
use crate::models::{Agent, Status, Ticket};
```
and add:
```rust
use crate::detect::{self, SignalLevel};
```

Add fields to `pub struct Engine`:

```rust
    /// Last observed signal level per ticket id (in-memory; re-baselined on restart).
    pub last_level: HashMap<i64, SignalLevel>,
    /// Tickets kamaji auto-moved to Review (provenance gate for the move back).
    pub auto_review_ids: HashSet<i64>,
    /// Per-ticket scrape screen hash for the stability guard.
    pub scrape_hash: HashMap<i64, Option<u64>>,
    /// Where per-session idle markers live.
    pub state_dir: std::path::PathBuf,
```

Update `Engine::new` to initialize them:

```rust
    pub fn new(db: Db, config: Config, app: App) -> Self {
        Engine {
            db,
            config,
            app,
            last_level: HashMap::new(),
            auto_review_ids: HashSet::new(),
            scrape_hash: HashMap::new(),
            state_dir: detect::default_state_dir(),
        }
    }
```

Add the apply seam and a small state-forget helper inside `impl Engine` (after `reconcile`):

```rust
    /// Forget all in-memory detection state for a ticket (on teardown/vanish).
    fn forget_ticket_state(&mut self, id: i64) {
        self.last_level.remove(&id);
        self.auto_review_ids.remove(&id);
        self.scrape_hash.remove(&id);
    }

    /// Apply move decisions given already-gathered signal levels. Split out from
    /// the IO so it can be unit-tested with crafted levels.
    fn detect_tick_with(&mut self, levels: &HashMap<i64, SignalLevel>) -> Result<()> {
        let mut changed = false;
        for (&id, &level) in levels {
            // Copy out the status so we don't hold an app borrow across the db write.
            let Some(status) = self.app.tickets.iter().find(|t| t.id == id).map(|t| t.status)
            else {
                continue;
            };
            let last = self.last_level.get(&id).copied();
            let was_auto = self.auto_review_ids.contains(&id);
            if let Some(target) = detect::decide(last, level, status, was_auto) {
                self.db.set_ticket_status(id, target)?;
                match target {
                    Status::Review => {
                        self.auto_review_ids.insert(id);
                        self.app.status_message = Some(format!("#{id} → Review (agent idle)"));
                    }
                    Status::InProgress => {
                        self.auto_review_ids.remove(&id);
                        self.app.status_message =
                            Some(format!("#{id} → In Progress (agent active)"));
                    }
                    _ => {}
                }
                changed = true;
            }
            if level != SignalLevel::Unknown {
                self.last_level.insert(id, level);
            }
        }
        if changed {
            self.reload()?;
        }
        Ok(())
    }
```

Clear provenance on manual move: at the very start of `apply_move`, add:

```rust
        self.auto_review_ids.remove(&ticket.id);
```

- [ ] **Step 2: Write failing round-trip tests**

Add to the `tests` module in `src/engine.rs` (after the existing tests):

```rust
    use crate::detect::SignalLevel;
    use std::collections::HashMap;

    /// Helper: an in-progress ticket with a recorded session, returns its id.
    fn in_progress_ticket(e: &mut Engine) -> i64 {
        let t = e
            .db
            .create_ticket(e.app.project.id, "t", "", None, Agent::Claude)
            .unwrap();
        e.db
            .set_ticket_session(t.id, "kamaji-x", "/wt", "kamaji-x")
            .unwrap();
        e.db.set_ticket_status(t.id, Status::InProgress).unwrap();
        e.reload().unwrap();
        t.id
    }

    fn levels(id: i64, level: SignalLevel) -> HashMap<i64, SignalLevel> {
        let mut m = HashMap::new();
        m.insert(id, level);
        m
    }

    #[test]
    fn idle_after_active_moves_in_progress_to_review() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let id = in_progress_ticket(&mut e);
        // Baseline Active, then Idle => Review.
        e.detect_tick_with(&levels(id, SignalLevel::Active)).unwrap();
        e.detect_tick_with(&levels(id, SignalLevel::Idle)).unwrap();
        assert_eq!(e.db.get_ticket(id).unwrap().unwrap().status, Status::Review);
        assert!(e.auto_review_ids.contains(&id));
    }

    #[test]
    fn resumed_auto_reviewed_card_returns_to_in_progress() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let id = in_progress_ticket(&mut e);
        e.detect_tick_with(&levels(id, SignalLevel::Active)).unwrap();
        e.detect_tick_with(&levels(id, SignalLevel::Idle)).unwrap(); // -> Review (auto)
        e.detect_tick_with(&levels(id, SignalLevel::Active)).unwrap(); // -> In Progress
        assert_eq!(
            e.db.get_ticket(id).unwrap().unwrap().status,
            Status::InProgress
        );
        assert!(!e.auto_review_ids.contains(&id));
    }

    #[test]
    fn manual_drag_back_is_not_re_moved() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let id = in_progress_ticket(&mut e);
        e.detect_tick_with(&levels(id, SignalLevel::Active)).unwrap();
        e.detect_tick_with(&levels(id, SignalLevel::Idle)).unwrap(); // -> Review
        // User drags it back to In Progress (manual move clears provenance).
        e.move_ticket(id, Status::InProgress).unwrap();
        // Agent is still idle; no transition => must NOT bounce back to Review.
        e.detect_tick_with(&levels(id, SignalLevel::Idle)).unwrap();
        assert_eq!(
            e.db.get_ticket(id).unwrap().unwrap().status,
            Status::InProgress
        );
    }

    #[test]
    fn never_drags_manually_placed_review_card() {
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        let id = in_progress_ticket(&mut e);
        // User hand-places it in Review while the agent is active.
        e.move_ticket(id, Status::Review).unwrap();
        e.detect_tick_with(&levels(id, SignalLevel::Idle)).unwrap(); // baseline
        e.detect_tick_with(&levels(id, SignalLevel::Active)).unwrap(); // resume
        // Not auto-reviewed => stays in Review.
        assert_eq!(e.db.get_ticket(id).unwrap().unwrap().status, Status::Review);
    }
```

> Note: `move_ticket` is a private method of `Engine`; these tests live in the same module so they may call it.

- [ ] **Step 3: Run the tests to verify they fail, then pass**

Run: `cargo test --lib engine 2>&1 | tail -30`
Expected: the four new tests PASS and all existing engine tests still PASS. If `apply_move`'s new provenance line or the field initializers don't compile, fix them.

- [ ] **Step 4: Commit**

```bash
git add src/engine.rs
git commit -m "feat(engine): detection state + detect_tick_with apply seam

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Engine `gather_levels` + `detect_tick` (wire real detectors)

**Files:**
- Modify: `src/engine.rs`

- [ ] **Step 1: Write a failing test for the Claude marker path end-to-end**

Add to the `tests` module in `src/engine.rs`:

```rust
    #[test]
    fn detect_tick_reads_claude_marker_and_moves_to_review() {
        let tmp = tempfile::tempdir().unwrap();
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.state_dir = tmp.path().to_path_buf();
        std::fs::create_dir_all(&e.state_dir).unwrap();

        let t = e
            .db
            .create_ticket(e.app.project.id, "t", "", None, Agent::Claude)
            .unwrap();
        e.db
            .set_ticket_session(t.id, "kamaji-sess", "/wt", "kamaji-sess")
            .unwrap();
        e.db.set_ticket_status(t.id, Status::InProgress).unwrap();
        e.reload().unwrap();

        // No marker yet => Active baseline; no move.
        e.detect_tick().unwrap();
        assert_eq!(e.db.get_ticket(t.id).unwrap().unwrap().status, Status::InProgress);

        // Agent's Stop hook would create the marker => Idle => Review.
        std::fs::write(crate::detect::marker_path(&e.state_dir, "kamaji-sess"), "").unwrap();
        e.detect_tick().unwrap();
        assert_eq!(e.db.get_ticket(t.id).unwrap().unwrap().status, Status::Review);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib engine::tests::detect_tick_reads_claude_marker 2>&1 | tail -20`
Expected: compile error — `detect_tick` not found.

- [ ] **Step 3: Implement `gather_levels` and `detect_tick`**

Add to `impl Engine` (after `detect_tick_with`):

```rust
    /// Read the current signal level for every live, in-progress/review ticket.
    fn gather_levels(&mut self) -> HashMap<i64, SignalLevel> {
        // Snapshot first so we don't borrow `app` while mutating scrape state.
        let live: Vec<(i64, Agent, String)> = self
            .app
            .tickets
            .iter()
            .filter(|t| matches!(t.status, Status::InProgress | Status::Review))
            .filter_map(|t| t.session_name.clone().map(|s| (t.id, t.agent, s)))
            .collect();

        let mut out = HashMap::new();
        for (id, agent, session) in live {
            let level = match agent {
                Agent::Claude => {
                    detect::marker_level(&detect::marker_path(&self.state_dir, &session))
                }
                Agent::Codex | Agent::Copilot => {
                    let patterns: Vec<String> = self.config.auto_review_patterns(agent).to_vec();
                    if patterns.is_empty() {
                        continue; // detector disabled for this agent
                    }
                    let screen = zellij::dump_screen(&session);
                    let hash = self.scrape_hash.entry(id).or_insert(None);
                    detect::scrape_level(screen.as_deref(), &patterns, hash)
                }
            };
            out.insert(id, level);
        }
        out
    }

    /// One detection pass: gather levels, then apply move decisions.
    pub fn detect_tick(&mut self) -> Result<()> {
        let levels = self.gather_levels();
        self.detect_tick_with(&levels)
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --lib engine 2>&1 | tail -30`
Expected: PASS (all engine tests including the new marker one).

- [ ] **Step 5: Commit**

```bash
git add src/engine.rs
git commit -m "feat(engine): gather_levels + detect_tick wiring

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Marker reset + argv injection in `start_session`; marker cleanup

**Files:**
- Modify: `src/engine.rs`

- [ ] **Step 1: Write failing tests**

Add to the `tests` module in `src/engine.rs`:

```rust
    #[test]
    fn cleanup_removes_marker_and_state() {
        let tmp = tempfile::tempdir().unwrap();
        let mut e = engine_with_project(std::path::PathBuf::from("/tmp/none"));
        e.state_dir = tmp.path().to_path_buf();
        std::fs::create_dir_all(&e.state_dir).unwrap();

        let t = e
            .db
            .create_ticket(e.app.project.id, "t", "", None, Agent::Claude)
            .unwrap();
        e.db
            .set_ticket_session(t.id, "kamaji-sess", "/wt", "kamaji-sess")
            .unwrap();
        e.reload().unwrap();
        let marker = crate::detect::marker_path(&e.state_dir, "kamaji-sess");
        std::fs::write(&marker, "").unwrap();
        e.auto_review_ids.insert(t.id);
        e.last_level.insert(t.id, SignalLevel::Idle);

        e.cleanup_ticket(t.id).unwrap();

        assert!(!marker.exists());
        assert!(!e.auto_review_ids.contains(&t.id));
        assert!(!e.last_level.contains_key(&t.id));
    }
```

Also extend the existing `start_session_creates_worktree_and_effect` test: right after `e.config.worktree_base = ...`, add a line pointing state_dir at the tempdir so the test never touches the real data dir, and assert the injected layout carries `--settings`:

```rust
        e.state_dir = dir.path().join("state");
```
and after the `Effect::RunSession { .. }` match arm asserts, add (still inside the test, after `assert!(layout_path.exists());`):
```rust
                let layout = std::fs::read_to_string(&layout_path).unwrap();
                assert!(layout.contains("--settings"), "claude layout must inject --settings: {layout}");
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib engine 2>&1 | tail -30`
Expected: `cleanup_removes_marker_and_state` fails (marker not removed) and the `--settings` assertion fails (not yet injected).

- [ ] **Step 3: Inject settings in `start_session`**

In `start_session`, replace the block:

```rust
        let argv = agent::build_command(
            self.config.commands_for(ticket.agent),
            ticket.initial_prompt.as_deref(),
        );
        let kdl = layout::render_layout(&worktree.to_string_lossy(), &argv);
```

with:

```rust
        let argv = agent::build_command(
            self.config.commands_for(ticket.agent),
            ticket.initial_prompt.as_deref(),
        );
        // For Claude, inject hook settings that maintain the idle marker, and
        // clear any stale marker so the session baselines as Active.
        let argv = if self.config.auto_review.enabled && ticket.agent == Agent::Claude {
            let marker = detect::marker_path(&self.state_dir, &name);
            let _ = std::fs::create_dir_all(&self.state_dir);
            let _ = std::fs::remove_file(&marker);
            detect::inject_claude_settings(argv, &marker.to_string_lossy())
        } else {
            argv
        };
        let kdl = layout::render_layout(&worktree.to_string_lossy(), &argv);
```

- [ ] **Step 4: Remove markers + state on cleanup and reconcile**

In `cleanup_ticket`, change the session-termination block:

```rust
            if let Some(name) = &t.session_name {
                zellij::terminate_session(name);
            }
```
to:
```rust
            if let Some(name) = &t.session_name {
                zellij::terminate_session(name);
                let _ = std::fs::remove_file(detect::marker_path(&self.state_dir, name));
            }
```
and at the end of `cleanup_ticket`, right after `self.db.clear_ticket_session(ticket_id)?;`, add:
```rust
            self.forget_ticket_state(ticket_id);
```

In `reconcile`, change the stale collection to keep session names and remove their markers/state. Replace:

```rust
        let stale: Vec<i64> = self
            .app
            .tickets
            .iter()
            .filter(|t| {
                t.session_name
                    .as_deref()
                    .is_some_and(|n| !zellij::session_in_list(&list, n))
            })
            .map(|t| t.id)
            .collect();
        for id in stale {
            self.db.clear_ticket_session(id)?;
        }
```
with:
```rust
        let stale: Vec<(i64, String)> = self
            .app
            .tickets
            .iter()
            .filter_map(|t| {
                t.session_name
                    .as_deref()
                    .filter(|n| !zellij::session_in_list(&list, n))
                    .map(|n| (t.id, n.to_string()))
            })
            .collect();
        for (id, name) in stale {
            self.db.clear_ticket_session(id)?;
            let _ = std::fs::remove_file(detect::marker_path(&self.state_dir, &name));
            self.forget_ticket_state(id);
        }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib engine 2>&1 | tail -30`
Expected: PASS (all engine tests, including the new cleanup test and the extended start_session test).

- [ ] **Step 6: Commit**

```bash
git add src/engine.rs
git commit -m "feat(engine): inject claude hook settings + clean up markers/state

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: Wire the detection cadence into the board loop

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Add the tick to `run_board`**

In `src/main.rs`, add to the imports near the top:

```rust
use std::time::Instant;
```

In `run_board`, replace the loop opening:

```rust
fn run_board(terminal: &mut DefaultTerminal, engine: &mut Engine) -> Result<bool> {
    loop {
        terminal.draw(|frame| ui::render(frame, &engine.app))?;
```
with:
```rust
fn run_board(terminal: &mut DefaultTerminal, engine: &mut Engine) -> Result<bool> {
    let mut last_tick = Instant::now();
    loop {
        if engine.config.auto_review.enabled && last_tick.elapsed() >= engine.config.poll_interval()
        {
            engine.detect_tick()?;
            last_tick = Instant::now();
        }
        terminal.draw(|frame| ui::render(frame, &engine.app))?;
```

(The existing `event::poll(Duration::from_millis(200))` already paces the loop, so the tick is checked at most every ~200 ms and runs at the configured interval.)

- [ ] **Step 2: Verify build + full test suite + lint**

Run:
```bash
cargo build 2>&1 | tail -5
cargo test 2>&1 | tail -15
cargo clippy 2>&1 | tail -20
cargo fmt
```
Expected: build clean, all tests PASS, no clippy errors (warnings acceptable but prefer none), fmt produces no diff worth committing beyond formatting.

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat(main): run detection tick at the configured poll interval

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 11: Manual end-to-end verification + finish

**Files:** none (verification + ship)

- [ ] **Step 1: Verify the Claude hook round-trip live**

This exercises the real CLI/zellij path the unit tests can't. In a project whose root is a git repo:

1. `cargo run` → pick/create the project → create a Claude ticket with an initial prompt → `m` move it to In Progress (you'll attach to the agent).
2. Let the agent finish its turn, then detach (`Ctrl+o d`).
3. Confirm the marker exists: `ls ~/.local/share/kamaji/state/` shows `<session>.idle`.
4. Within ~5s the board shows the card move to **Review** with a status message.
5. `a` to attach, send a new instruction; detach. The marker is removed on submit/tool-use; within ~5s the card returns to **In Progress**.
6. Drag the card back to In Progress manually while idle → it must NOT bounce back to Review.

If the move does not happen, debug with `claude --debug hooks` in a scratch session to confirm the hook fires, and verify `--settings` is present in the generated layout under `$TMPDIR/kamaji-layouts/<session>.kdl`.

- [ ] **Step 2: Verify the scrape path invocation (optional, codex/copilot)**

Confirm the dump command targets a background session on your zellij version:
```bash
zellij --session <some-running-session> action dump-screen /tmp/k.txt && head /tmp/k.txt
```
If your zellij needs different syntax, fix `zellij::dump_screen` accordingly and re-run `cargo test`. (Scrape ships with empty patterns, so this does not block the Claude feature.)

- [ ] **Step 3: Confirm the spec's requirements are all met**

Re-read the spec's §1 behavior list and confirm each is observable: forward move, backward move (round-trip), no-fight-after-manual-drag, never-drag-manual, configurable interval, works while detached. Note any gaps as follow-up issues per AGENTS.md (do not expand scope here).

- [ ] **Step 4: Open the PR and enable auto-merge**

```bash
git push -u origin issue-1-auto-move-review
gh pr create --fill --base main
gh pr merge --squash --auto --delete-branch
```

- [ ] **Step 5: After merge, sync the slay task and clean up**

```bash
# find the task id by the GitHub issue number, then close it
slay tasks list --project kamaji --json \
  | jq -r '.[] | select(.externalProvider=="github" and .externalId=="1") | .id'
slay tasks done <task-id> --close

git worktree remove ../kamaji-worktrees/issue-1-auto-move-review
git branch -d issue-1-auto-move-review
```

---

## Self-Review

**Spec coverage:**
- §1 behavior (edge-triggered, provenance, under-flag) → Task 2 `decide()`, Task 7 provenance + apply seam. ✓
- §2.1 Claude hook marker → Task 3 (marker), Task 5 (settings/inject), Task 9 (injection at launch). ✓
- §2.2 scrape fallback → Task 4 (`scrape_level`), Task 6 (`dump_screen`), Task 8 (wiring). ✓
- §3 state machine + scenarios → Task 2 tests + Task 7 round-trip tests. ✓
- §4 polling integration → Task 10. ✓
- §5 launch injection + marker lifecycle → Task 9. ✓
- §6 config → Task 1. ✓
- §7 modules/types/tests → all tasks; `detect.rs` created across Tasks 2–5. ✓
- §8 non-goals respected (no multi-pane, no regex, no persistence). ✓

**Placeholder scan:** No TBD/TODO; every code step shows full code; commands have expected output. ✓

**Type consistency:** `SignalLevel{Idle,Active,Unknown}`, `decide(Option<SignalLevel>, SignalLevel, Status, bool) -> Option<Status>`, `marker_path(&Path,&str)->PathBuf`, `marker_level(&Path)->SignalLevel`, `scrape_level(Option<&str>,&[String],&mut Option<u64>)->SignalLevel`, `claude_settings_json(&str)->String`, `inject_claude_settings(Vec<String>,&str)->Vec<String>`, `dump_screen(&str)->Option<String>`, `Engine::{detect_tick, detect_tick_with, gather_levels, forget_ticket_state}` — names used consistently across Tasks 2–10. ✓
