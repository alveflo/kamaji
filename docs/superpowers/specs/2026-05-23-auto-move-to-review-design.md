# Auto-move tickets to Review when an agent needs input — Design

- **Date:** 2026-05-23
- **Status:** Approved (design); implementation plan pending
- **Author:** Victor Alveflo
- **Tracks:** [issue #1](https://github.com/alveflo/kamaji/issues/1)
- **Builds on:** `docs/superpowers/specs/2026-05-23-kamaji-design.md` §11 (deferred)

## 1. Overview

kamaji renders tickets on a Kanban board (Todo → In Progress → Review → Done)
and runs each In-Progress ticket's agent in a background zellij session. Today
all column moves are manual. This feature auto-moves a live ticket between
**In Progress** and **Review** based on whether its agent is idle / waiting for
user input, so the user can tell at a glance which sessions need attention
without attaching to each one.

### Behavior

- **In Progress → Review** when the agent goes idle / needs input.
- **Review → In Progress** when the agent resumes work (round-trip).
- **Edge-triggered, not level-triggered.** kamaji acts only on *transitions*
  (`Active→Idle`, `Idle→Active`), never on a steady state. A steady state never
  re-fires, so manually dragging a card does not fight the heuristic.
- **Provenance gate on the backward move.** A card auto-moves Review →
  In Progress *only if kamaji itself* moved it to Review. Any manual move clears
  that provenance, so a card the user placed in Review by hand is never dragged
  out.
- **Tolerant of false positives.** Prefers under-flagging to spurious moves
  (per the issue). Unknown/unreadable signals never trigger a move.

## 2. Signal sources — per-agent `Detector`

Each agent maps to a detector reporting a `SignalLevel`:

```rust
enum SignalLevel { Idle, Active, Unknown }
```

`Unknown` (e.g. the screen couldn't be dumped, or the marker couldn't be read)
is treated as "no information" — it never produces a transition and never
updates the baseline (see §3).

### 2.1 Claude — launch-injected hook marker (no scraping)

The most reliable signal for Claude is its own hook lifecycle, not screen
scraping. At session start kamaji appends `--settings '<json>'` to the resolved
claude argv. `claude --settings <file-or-json>` is a verified flag that *merges*
additional settings (it does not replace the repo's own `.claude/settings*.json`,
and nothing is written into the user's repo or worktree).

The injected settings define hooks that maintain a per-session **marker file**
at `~/.local/share/kamaji/state/<session_name>.idle` (XDG data dir; honors
`$XDG_DATA_HOME`):

| Hook event        | Action on marker | Meaning              |
|-------------------|------------------|----------------------|
| `Stop`            | create           | turn finished → idle |
| `Notification`    | create           | needs permission / idle-timeout → needs attention |
| `UserPromptSubmit`| remove           | user sent input → active |
| `PreToolUse`      | remove           | agent running a tool → active |

kamaji reads the marker each poll: **present ⇒ `Idle`, absent ⇒ `Active`**. This
works even while kamaji is suspended or attached elsewhere — the hooks fire
inside the zellij session regardless; kamaji catches up on its next poll.

The hook command strings are generated at launch with the absolute marker path
baked in (single-quoted), e.g. `touch '/home/u/.local/share/kamaji/state/kamaji-1-x.idle'`
and `rm -f '<same path>'`.

### 2.2 Codex / Copilot — scrape fallback

For agents without a hook lifecycle kamaji scrapes the session screen:

1. `zellij --session <name> action dump-screen <tmpfile>` (targets the session
   via the zellij server; kamaji need not be attached).
2. Compute the level:
   - `Idle` if the buffer **matches a configured idle substring AND is unchanged
     since the previous poll** (the stability guard rejects a transiently-visible
     prompt while the agent is mid-work).
   - `Active` otherwise.
   - `Unknown` if the dump command fails.

`dump-screen` captures only the **focused** pane. If the user has focused another
pane/tab the detector sees that instead of the agent, yielding `Active`/no-match
→ no move. This is an accepted, deliberate under-flag (the issue prefers a missed
move to a spurious one). In kamaji's generated layout the agent is the focused
pane on launch.

## 3. State machine (core; pure and unit-tested)

Per live ticket kamaji keeps, **in memory**, `last_level: Option<SignalLevel>`
and a shared set `auto_review_ids: HashSet<i64>` (tickets kamaji auto-moved to
Review). The decision is a pure function exhaustively unit-tested without zellij
or claude:

```
fn decide(last: Option<SignalLevel>, current: SignalLevel,
          status: Status, was_auto_reviewed: bool) -> Option<Status>

current == Unknown                              -> None        (ignore; do not update baseline)
last == None                                    -> None        (first sight: baseline only)
(Active -> Idle)  & status == InProgress        -> Some(Review)        + mark provenance
(Idle  -> Active) & status == Review & auto     -> Some(InProgress)    + clear provenance
otherwise                                       -> None
```

Driver per tick, for each ticket with a live `session_name`:

```
current = detector.level(ticket)
if current == Unknown { continue }            // leave last_level untouched
match decide(last_level[id], current, ticket.status, auto_review_ids.contains(id)) {
    Some(Review)     => { set_status(id, Review);     auto_review_ids.insert(id); status_msg }
    Some(InProgress) => { set_status(id, InProgress); auto_review_ids.remove(id); status_msg }
    _ => {}
}
last_level[id] = current
```

- A **manual move** (the `m` Move modal) calls `auto_review_ids.remove(id)` — the
  user now owns the card's placement; the backward auto-move can't drag it.
- In-memory state is re-baselined on restart (no persistence). After a restart
  kamaji observes the current level as the baseline and only acts on subsequent
  transitions — deliberately conservative (prefers under-flagging over a startle
  move on startup).
- On a successful auto-move kamaji sets `app.status_message`, e.g.
  `#3 → Review (agent idle)` / `#3 → In Progress (agent active)`.

### 3.1 Worked scenarios

- **Happy path:** agent finishes → `Active→Idle` while In Progress → Review
  (+provenance). User attaches, replies → `Idle→Active` while Review & auto →
  In Progress (−provenance). Finishes again → Review.
- **No fight after manual drag-back:** auto-moved to Review (Idle). User drags it
  to In Progress; level is still Idle (no transition) → no re-move. Manual move
  also cleared provenance.
- **Never drags a manual Review card:** user hand-places a card in Review while
  its agent is active; provenance is not set. Agent later resumes → `Idle→Active`
  but `was_auto_reviewed == false` → no move.

## 4. Polling integration

A synchronous tick inside `run_board` (`src/main.rs`) — no threads, matching the
existing single-threaded event loop. The loop already calls
`event::poll(Duration::from_millis(200))`. Add a `last_tick: Instant`; when
`last_tick.elapsed() >= poll_interval`, run one `engine.detect_tick()` pass
before the next `terminal.draw`, then reset `last_tick`. A status change is
reflected on the following draw (≤200 ms later).

Cost: marker reads are a `stat`; scrape spawns one subprocess per scrape-agent
session at a multi-second cadence. Negligible. No busy-loop. Detection runs only
while the board loop runs (i.e. while not attached); the Claude hook marker keeps
accumulating state while attached and is reconciled on return.

Disabled (`auto_review.enabled = false`) ⇒ the tick is skipped entirely.

## 5. Launch injection & marker lifecycle

In `Engine::start_session`, when `auto_review.enabled` and the ticket's agent is
`Claude`:

1. Compute the marker path `state_dir/<session_name>.idle` and **remove any stale
   marker** (so the session baselines as `Active`).
2. Build the settings JSON (§2.1) and splice `--settings <json>` into the argv
   immediately after `argv[0]` (a global claude flag; before the positional
   prompt). Injection is keyed on `agent == Claude`.
3. Proceed with layout generation as today.

Marker cleanup: `cleanup_ticket` and `reconcile` remove a ticket's marker file
when its session is torn down or has vanished, alongside the existing
`clear_ticket_session`. A missing marker is never an error.

Scrape agents need no launch changes.

## 6. Configuration

Additions to `~/.config/kamaji/config.toml` (with serde defaults so existing
config files keep loading):

```toml
[auto_review]
enabled = true
poll_interval_secs = 5

[auto_review.patterns]   # scrape idle-substrings; claude ignores these (uses hooks)
codex   = []             # empty list ⇒ that detector is disabled (safe default)
copilot = []
```

- `enabled` default `true`; `poll_interval_secs` default `5`.
- An empty pattern list disables the scrape detector for that agent (it always
  reports `Active`/`Unknown` → never moves), so Codex/Copilot ship inert until
  the user tunes patterns to their installed CLI version. Claude works out of the
  box via hooks.
- All fields use `#[serde(default)]` so older `config.toml` files still parse.

## 7. Modules, types & testing

New `src/detect.rs`:
- `SignalLevel { Idle, Active, Unknown }`.
- `Detector` trait: `fn level(&mut self, ticket: &Ticket) -> SignalLevel`.
- `ClaudeMarkerDetector` (resolves marker path from session name; presence→level).
- `ScrapeDetector { idle_substrings: Vec<String>, last_hash: Option<u64> }`.
- `decide(...)` pure transition function (§3).
- Settings-JSON builder and argv-injection helper (so both are testable without
  launching anything).

`Engine` (`src/engine.rs`):
- Holds `last_level: HashMap<i64, SignalLevel>` and `auto_review_ids: HashSet<i64>`.
- `detect_tick(&mut self) -> Result<()>` runs the §3 driver.
- `start_session` does marker reset + argv injection (§5).
- Manual-move paths clear provenance.

`Config` (`src/config.rs`): new `AutoReview` struct + `patterns` map, all
serde-defaulted.

`main.rs`: the `last_tick` cadence in `run_board`.

**Tests (TDD):**
- `decide()` — every `(last, current, status, provenance)` combination, including
  `Unknown` and `None` baseline.
- `ClaudeMarkerDetector` — marker present/absent/created/removed via temp files.
- `ScrapeDetector` — match+stable ⇒ Idle; match+changed ⇒ Active; no-match ⇒
  Active; empty patterns ⇒ never Idle.
- Settings-JSON builder — exact JSON for a given marker path.
- argv injection — `--settings <json>` spliced after `argv[0]`, only for Claude.
- Engine round-trip — drive `detect_tick` against a fake detector to assert the
  forward move, backward move, no-fight-after-manual-drag, and never-drag-manual
  scenarios end to end.

Actual `zellij action dump-screen` and live claude hook firing are verified
manually (they require running processes); the design keeps every decision unit
behind a pure boundary so CI coverage does not depend on them.

## 8. Non-goals (this iteration)

- Multi-pane / non-focused-pane targeting for scrape agents (focused pane only).
- Regex patterns (substring matching only, to avoid a new dependency).
- Persisting `last_level` / provenance across kamaji restarts.
- Auto-move involving Todo or Done — only the In Progress ↔ Review pair.

## 9. Risks & open items

- Claude hook event names / `--settings` merge semantics — confirmed against the
  installed CLI during the design; re-verify if the CLI is upgraded.
- Codex / Copilot idle substrings are version-sensitive and ship empty; they need
  one-time tuning against the installed CLI (documented in config comments).
- `PreToolUse` fires frequently (one `rm -f` per tool call); cheap and idempotent,
  but noted.
