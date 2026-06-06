# AGENTS.md — Working agreement for AI agents on kamaji

This file applies to **all** AI coding agents working in this repository
(Claude Code, Codex, and any other). `CLAUDE.md` is a symlink to this file.

kamaji orchestrates AI agents as zellij sessions on a per-project Kanban board,
reachable from both a web browser and a ratatui TUI over one shared backend.

**To understand the system as it is today, read
[`ARCHITECTURE.md`](ARCHITECTURE.md)** — crate responsibilities, ports, the
command/event flow, daemon lifecycle, and how zellij web + the proxy fit. The
`docs/superpowers/` tree is *historical design history* (see below), not a
description of the current system.

---

## Codebase map

A three-crate Cargo workspace (full detail in [`ARCHITECTURE.md`](ARCHITECTURE.md)):

| Crate | What it is |
|-------|------------|
| `crates/kamaji-core` | Pure domain logic library — SQLite DB, git worktrees, zellij orchestration, agent templates, auto-review detection. No UI, no transport. |
| `crates/kamajid` | The **daemon**: axum HTTP server wrapping `kamaji-core`, SSE event broadcaster, `maud` browser board, managed `zellij web` + reverse proxy. Owns the single DB writer. |
| `crates/kamaji` | The binary the user runs: thin launcher + ratatui TUI client. Auto-spawns the daemon, talks to it over HTTP, renders board state from the event stream. |

**Ports** (all `127.0.0.1`): **8755** board HTTP API + HTML + SSE · **8756**
reverse proxy for zellij web (board port + 1) · **8082** `zellij web` (the
browser terminal subprocess).

**Commands down, events up:** clients POST commands over HTTP; the daemon
mutates state and broadcasts deltas over SSE (`/events` JSON for the TUI,
`/ui/events` Datastar for the browser).

**Make targets** (daemon control — see the `Makefile`):

| Target | Use |
|--------|-----|
| `make restart` | Rebuild + relaunch the daemon — **run this after pulling new code** so the running process picks up changes. |
| `make start` / `make stop` | Start / stop the daemon (board :8755, proxy :8756). |
| `make status` / `make logs` | Is it up (`GET /healthz`)? / follow `/tmp/kamajid.log`. |

The `kamaji` TUI auto-spawns the daemon, so it doesn't need `make start` first;
`make start`/`restart` are for the daemon that serves the **browser** board.

### `docs/superpowers/` is historical design history

`docs/superpowers/specs/` and `docs/superpowers/plans/` are a dated,
append-only record of brainstorms, specs, and implementation plans — *partially
superseded* as the system evolved (notably the 2026-05-27 browser-first pivot).
They explain **why** kamaji is shaped the way it is, but they are **not** a
description of the current system. For current architecture read
[`ARCHITECTURE.md`](ARCHITECTURE.md); reach for `docs/superpowers/` only for
historical context or when working a plan that explicitly references one.

---

## Core workflow

Every piece of work follows the same loop: **isolate → build → ship**.

### 0. Executing an implementation plan: always subagent-driven

When executing a written implementation plan (one of `docs/superpowers/plans/`),
**always use the `superpowers:subagent-driven-development` approach** — a fresh
implementer subagent per task, followed by a spec-compliance review and a
code-quality review before moving on, plus a final whole-branch review before
shipping. This is the repo owner's standing preference: **do not ask which
execution mode to use** — subagent-driven is the default for every plan. (Inline
execution is only for an explicit one-off request.)

### 1. Always work in a git worktree

Never commit on `main` directly, and never work in the primary working tree.
Each task gets its own worktree and branch so multiple tasks can proceed in
parallel without colliding:

```sh
git worktree add ../kamaji-worktrees/<branch> -b <branch> <base>
```

- Branch naming: `issue-<n>-<slug>` when the work tracks a GitHub issue,
  otherwise a short descriptive `<slug>`.
- `<base>` is the repo default branch (`main`) unless told otherwise.
- Do your editing, building, and committing inside that worktree.

### 2. Side quests become GitHub issues

While working a task you will notice other things — bugs, missing tests,
refactors, follow-ups. Decide what to do with each:

- **In scope and trivial** → just fix it inline as part of the current task.
- **Genuinely separate work (a real side quest)** → file a GitHub issue. Do
  **not** derail the current task to do it now.
- **A side quest that blocks the current task** → file the issue *and* treat it
  as a blocker (the current work waits on it).

Do not file issues for trivial in-scope fixes — keep the tracker signal-rich,
not noisy. File an issue with:

```sh
gh issue create --title "<concise title>" --body "<context, why it matters, acceptance criteria>"
```

### 3. A new issue becomes a slay task that auto-starts a Claude session

Whenever a GitHub issue is created (by you, for a side quest), create a matching
slay task. **slay spawns the Claude process** that works the issue — and the
task's **`--description` is the initial prompt** handed to that Claude. So set
the description to a short "start working" instruction pointing at the issue;
the issue itself holds the detail.

```sh
slay tasks create "<issue title>" \
  --project kamaji \
  --description "Start working on GitHub issue #<n>." \
  --external-provider github \
  --external-id "<n>"
```

Conventions (always follow these):
- **Project** is always **kamaji** (`--project kamaji`).
- **Description = the agent prompt.** Keep it to `Start working on GitHub issue
  #<n>.` The spawned Claude reads the issue itself (`gh issue view <n>`) for full
  context — do **not** paste the issue body into the description.
- **`--external-id <n>` + `--external-provider github` dedupe** against the
  issue: re-running never creates a duplicate task. Always pass them.
- **Make the GitHub issue self-contained and actionable** (clear what / why /
  acceptance criteria + pointers to the relevant files), because that issue is
  the spec the spawned Claude works from.

Creating the task does not itself launch Claude — the task is started from slay
(by a human or automation). When started, slay spawns a Claude session with the
prompt above and it begins working the issue automatically, in its own worktree
per the worktree rule (§1). Because `--external-id` dedupes, you never spawn the
same issue twice.

### 4. Ship: PR, then auto-merge when green

When the work for a branch is done:

```sh
gh pr create --fill --base main
gh pr merge --squash --auto --delete-branch
```

- Use **squash merge**.
- `--auto` enables auto-merge: GitHub merges automatically once required status
  checks pass. **If the repo has no CI pipelines/checks, the PR merges
  immediately** — that is the intended behavior.
- After merge, remove the local worktree and branch:

  ```sh
  git worktree remove ../kamaji-worktrees/<branch>
  git branch -d <branch>   # if not already deleted by --delete-branch
  ```

### 5. When the PR merges and the issue closes, mark the slay task done

Squash-merging with `--delete-branch` closes the linked GitHub issue. Whenever
an issue's PR is merged, mark its slay task done so the board stays in sync:

```sh
slay tasks done <task-id> --close
```

If you don't have the task id handy, look it up by the GitHub issue number it
was created with (`--external-provider github --external-id <n>`):

```sh
slay tasks list --project kamaji --json \
  | jq -r '.[] | select(.externalProvider=="github" and .externalId=="<n>") | .id'
```

(Confirm the exact JSON field names with `slay tasks list --json` once; adjust
the filter if they differ.) Then `slay tasks done <id> --close`.

---

## Quick reference

| Situation                          | Do this                                             |
|------------------------------------|-----------------------------------------------------|
| Executing a plan                   | Subagent-driven (fresh subagent + reviews per task); never ask which mode |
| Starting any task                  | New worktree + branch off `main`                    |
| Noticed separate work              | `gh issue create` (only if genuinely out of scope)  |
| Issue created                      | `slay tasks create "<title>" --project kamaji --description "Start working on GitHub issue #<n>." --external-provider github --external-id <n>` (description = the spawned Claude's prompt) |
| Work finished                      | `gh pr create` → `gh pr merge --squash --auto`      |
| Merged / issue closed              | Remove worktree + branch; `slay tasks done <id> --close` |

## Project conventions

- Language: Rust (edition per `Cargo.toml`). Format with `cargo fmt`; lint with
  `cargo clippy`. Run `cargo test` before opening a PR.
- Keep modules small and single-purpose (see the design spec's emphasis on
  isolated, independently testable units).
- Never commit secrets. Never force-push shared branches.
