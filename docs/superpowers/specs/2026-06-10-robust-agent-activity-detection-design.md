# Robust per-agent activity detection

**Date:** 2026-06-10
**Status:** Approved, ready for planning
**Scope:** Close the reliability gaps in kamaji's agent activity detection by
adopting slayzone's more robust per-agent strategies, while keeping kamaji's
existing marker-file + poll-loop architecture.

## Background

kamaji already detects whether an agent session is *Active* or *Idle* and
auto-moves its ticket between **In Progress** and **Needs attention** (Review).
The mechanism (see `crates/kamaji-core/src/{detect,poll,session}.rs`):

- A per-session **idle marker file** (`<state_dir>/<session>.idle`).
- A daemon **poll loop** (~every 5s) reads the level and applies edge-triggered
  move decisions via `detect::decide`.
- `SignalLevel { Active, Idle, Unknown }` rides the `session.signal` SSE event.

Detection is assigned per agent today:

| Agent | Today | Reliability |
|-------|-------|-------------|
| Claude | Launch-injected `--settings` hooks `touch`/`rm` the marker; poll reads file existence. `instrumented = true`. | Good (hook-driven). |
| Codex | Daemon scrapes `zellij dump-screen` for configured idle substrings + stability guard. | Fragile (TUI redraws; substrings drift). |
| Copilot | Same screen-scrape + substrings. | Fragile (full-screen TUI is high-churn). |

slayzone (`/home/victor/dev/slayzone`) solves the same problem with two
families: **hook-driven** (Claude, Codex) and **timeout-driven** (Copilot). This
spec brings kamaji's Codex and Copilot paths up to that standard. Claude is
already hook-driven and stays as-is.

## Goals

1. **Codex → hook-instrumented.** Use Codex's native hooks (`~/.codex/hooks.json`,
   Codex ≥ 0.129) to maintain the same idle marker file Claude uses. Retire the
   Codex screen-scrape path.
2. **Copilot → screen-change timeout.** Replace fragile idle-substring matching
   with a screen-change timeout: a static screen for N seconds means Idle, any
   change means Active. Retire the Copilot screen-scrape substring path.
3. Keep everything else unchanged: the marker convention, `decide`, the poll
   loop's move logic, SSE events, and the Claude path.

## Non-goals

- No HTTP webhook / state-machine rewrite. kamaji keeps the marker + poll model;
  slayzone's webhook transport is **not** adopted.
- No Codex-version fallback. Codex ≥ 0.129 is **assumed** (per decision). On an
  older Codex the hooks simply won't fire and detection shows `Unknown` (which
  never moves a ticket) — acceptable.
- Claude's richer "awaiting user input vs turn complete" distinction
  (slayzone's `CLAUDE_BLOCKING_TOOLS`) is out of scope; Claude stays binary
  Active/Idle. Can be a future issue.

## Design

### 1. Codex becomes hook-instrumented

**Marker convention (unchanged).** The poll loop reads
`detect::marker_path(state_dir, session)` = `<state_dir>/<session>.idle`, where
`session` is the zellij session name (`slug::ticket_name(...)`). Codex's hooks
must `touch`/`rm` exactly that path.

**No per-invocation config for Codex.** Unlike Claude's `--settings` flag,
Codex hooks live in a *single global file* `~/.codex/hooks.json` shared by every
Codex session. So the per-session marker path cannot be baked per-session.
Instead the hook command derives it at runtime from the zellij session name,
which Codex's hook subprocess inherits from its pane:

- **Active** (`UserPromptSubmit`, `PreToolUse`):
  `sh -c 'case "$ZELLIJ_SESSION_NAME" in kamaji-*) rm -f "<state_dir>/$ZELLIJ_SESSION_NAME.idle";; esac'`
- **Idle** (`Stop`, `PermissionRequest`):
  `sh -c 'case "$ZELLIJ_SESSION_NAME" in kamaji-*) touch "<state_dir>/$ZELLIJ_SESSION_NAME.idle";; esac'`

`<state_dir>` is kamaji's absolute state dir, baked into the command literal
(machine-constant). The `kamaji-*` guard means the global hooks are inert for
the user's own (non-kamaji) Codex sessions — they create/remove nothing. A
non-zellij Codex session has no `$ZELLIJ_SESSION_NAME`, so the guard also skips
it. The event → marker mapping mirrors Claude exactly.

**Installer (`detect::install_codex_hooks`).** Idempotent merge into
`~/.codex/hooks.json`:
1. Read existing JSON (or start from `{}`); tolerate a missing/empty/invalid
   file by treating it as empty (log on invalid, do not clobber blindly — see
   open question O1).
2. For each of the four events, drop any existing array entries marked
   `"_kamajiManaged": true`, then append kamaji's entry (also marked
   `_kamajiManaged: true`). Preserve all user-defined hooks and any other keys.
3. Write back only if the content changed (avoid needless writes / mtime churn).

The exact `hooks.json` entry shape (object with `type: "command"`, `command`,
and `matcher` for tool events) is modeled on slayzone's
`codex-hook-installer.ts` and **must be verified against current Codex docs**
during implementation (see O2).

**Wiring (`session::prepare_with_argv`).** Extend instrumentation to Codex:
```text
instrumented = auto_review.enabled && agent ∈ {Claude, Codex}
if instrumented:
    marker = marker_path(state_dir, name); ensure state_dir; remove marker  // start "active"
    match agent:
        Claude => argv = inject_claude_settings(argv, marker)   // unchanged
        Codex  => install_codex_hooks(state_dir)?               // global, idempotent; argv unchanged
```
Codex sessions are then created with `instrumented = true` and the poll loop's
marker path applies to them automatically.

### 2. Copilot screen-change timeout

kamaji's daemon cannot see keystrokes or raw PTY output (zellij owns the PTY);
it only runs `zellij dump-screen` each poll. So the kamaji-native equivalent of
slayzone's silence timer is **screen-change based**:

New `detect::screen_change_level(screen, state, idle_after_unchanged)`:
- `screen == None` (dump failed) → `Unknown` (never moves a ticket; existing rule).
- `screen` hash **differs** from last → reset unchanged counter, store hash → `Active`.
- `screen` hash **equals** last → increment unchanged counter; `Idle` once it
  reaches `idle_after_unchanged`, else `Active`.

State per ticket: `{ last_hash: Option<u64>, unchanged_count: u32 }` (replaces
the current `scrape_hash` map in `PollLoop`).

**Quantization.** The daemon samples every `poll_interval_secs` (default 5), so
the timeout is expressed in unchanged polls:
`idle_after_unchanged = max(1, ceil(copilot_idle_secs / poll_interval_secs))`.
With the defaults (`copilot_idle_secs = 8`, poll 5s) that is 2 unchanged polls
(~10s of a static screen → Idle). Rationale: a working Copilot TUI animates
(spinner/redraw) so its screen changes between polls; a finished or
awaiting-input TUI is static.

### 3. `gather_levels` after the change

```text
if instrumented:                      // Claude or Codex
    marker_level(marker_path(state_dir, session))
else if agent == Copilot && auto_review.enabled:
    screen_change_level(dump_screen(session), screen_state[id], idle_after_unchanged)
else:
    Unknown                           // auto_review off, or un-instrumented Claude/Codex
```
The exited-session short-circuit (`zellij::session_exited` → `Unknown`) is
unchanged.

## Data model & config

- `tickets.instrumented` is now `true` for Codex too. **No schema change** — the
  column already exists; only the value written changes.
- **Config:**
  - Remove `auto_review.patterns` (`ScrapePatterns`) and
    `Config::auto_review_patterns`. Old config files keep their now-unused
    `[auto_review.patterns]` table harmlessly — serde ignores unknown fields, so
    existing configs still load.
  - Add `auto_review.copilot_idle_secs: u64` (`#[serde(default)]`, default 8).
  - Add `Config` helper to compute `idle_after_unchanged` from
    `copilot_idle_secs` and `poll_interval_secs`.
- `PollLoop.scrape_hash: HashMap<i64, Option<u64>>` →
  `screen_state: HashMap<i64, ScreenChangeState>`; `forget_ticket` clears it.

## Components & boundaries

| Unit | Responsibility |
|------|----------------|
| `detect::screen_change_level` + `ScreenChangeState` | Pure screen-change → `SignalLevel`. Unit-tested in isolation. |
| `detect::install_codex_hooks` + `codex_hooks_json` | Build & idempotently merge the global Codex hooks file. JSON-building unit-tested; merge tested against fixtures. |
| `detect::marker_level` / `marker_path` / `inject_claude_settings` | Unchanged. |
| `session::prepare_with_argv` | Decides instrumentation per agent; the only place that wires Claude `--settings` vs Codex hooks install. |
| `poll::PollLoop::gather_levels` | Picks the detector per agent; holds per-ticket screen state. |
| `config` | Owns `copilot_idle_secs` and the polls-to-idle computation. |

## Error handling

- `install_codex_hooks` failure: surface as the session-prepare error (Codex
  instrumentation is required for detection). It writes a user file, so failures
  (permissions, unreadable existing JSON) should be reported, not silently
  swallowed. See O1 for the invalid-existing-file policy.
- Screen dump failure stays `Unknown` (no move), as today.
- Removing `auto_review_patterns` must not break config load for existing files
  (verified: serde ignores the leftover `patterns` table).

## Testing

Unit (`kamaji-core`):
- `screen_change_level`: changed→Active; unchanged-below-threshold→Active;
  unchanged-at-threshold→Idle; `None`→Unknown; threshold of 1.
- `codex_hooks_json`: wires all four events; commands contain the `kamaji-*`
  guard, the baked state dir, and `$ZELLIJ_SESSION_NAME`; valid JSON.
- `install_codex_hooks` merge: fresh file; preserves a pre-existing user hook;
  idempotent re-run (no duplicate kamaji entries); replaces a stale
  kamaji-managed entry; tolerates empty file.
- `Config`: `copilot_idle_secs` default (8) and load when absent; polls-to-idle
  computation (e.g. 8/5→2, 5/5→1, 3/5→1); a config carrying a legacy
  `[auto_review.patterns]` table still loads.
- `session`: a Codex ticket prepares as `instrumented = true`.
- `decide` and the Claude marker tests stay green unchanged.

Manual / acceptance (after `make restart`):
- Codex ticket: start it, confirm it sits in In Progress while working and
  auto-moves to Needs attention when it stops; confirm `~/.codex/hooks.json`
  contains the kamaji-managed entries and any prior user hooks survive.
- Copilot ticket: confirm Active while it works, Idle (→ Needs attention) after
  it goes quiet for ~10s.

## Open questions (resolve during implementation)

- **O1 — invalid existing `~/.codex/hooks.json`:** if the file exists but is not
  valid JSON, do we (a) abort with an error, or (b) back it up and replace? Lean
  (a) abort + clear message — never destroy a user file kamaji can't parse.
- **O2 — exact Codex hooks.json schema:** confirm event names
  (`UserPromptSubmit`, `PreToolUse`, `Stop`, `PermissionRequest`), the hook entry
  object shape, and whether `matcher` is required for `PreToolUse`, against
  current Codex docs before finalizing `codex_hooks_json`. slayzone's
  `codex-hook-installer.ts` is the working reference.
- **O3 — install timing:** install in `prepare_with_argv` per Codex session
  (chosen — single shared path for TUI + daemon + CLI, idempotent) vs once at
  daemon startup. Confirm no concurrency hazard from two near-simultaneous Codex
  starts writing the file (serialize the read-modify-write if needed).
