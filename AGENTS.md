# AGENTS.md — Working agreement for AI agents on kamaji

This file applies to **all** AI coding agents working in this repository
(Claude Code, Codex, and any other). `CLAUDE.md` is a symlink to this file.

kamaji is a Rust + ratatui TUI that orchestrates AI agents as zellij sessions on
a per-project Kanban board. See `docs/superpowers/specs/` for the design.

---

## Core workflow

Every piece of work follows the same loop: **isolate → build → ship**.

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

### 3. A new issue spawns a slay ticket and a live Claude session

Whenever a GitHub issue is created (by you, for a side quest), immediately:

1. **Create a matching slay ticket**, deduplicated against the issue so the same
   issue never produces two tickets:

   ```sh
   slay tasks create "<issue title>" \
     --description "<issue body + link to the GitHub issue>" \
     --external-provider github \
     --external-id "<issue number>"
   ```

   `--external-id` makes this idempotent: if a ticket for that issue already
   exists, slay skips creating a duplicate. **Always pass it**, and check for an
   existing ticket/worktree before proceeding so you never spawn work twice.

2. **Create a worktree** for the issue branch (`issue-<n>-<slug>`, see step 1).

3. **Auto-launch a new Claude Code session** in that worktree, seeded with the
   issue context, so the side quest starts immediately:

   ```sh
   cd ../kamaji-worktrees/issue-<n>-<slug>
   claude "Work GitHub issue #<n>: <title>. <body / acceptance criteria>"
   ```

   Guard against runaway fan-out: before launching, confirm (via the
   `--external-id` dedup and the presence of a worktree) that this issue is not
   already being worked. Never re-launch a session for an issue that already has
   one.

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
- Mark the corresponding slay ticket done: `slay tasks done <id>`.

---

## Quick reference

| Situation                          | Do this                                             |
|------------------------------------|-----------------------------------------------------|
| Starting any task                  | New worktree + branch off `main`                    |
| Noticed separate work              | `gh issue create` (only if genuinely out of scope)  |
| Issue created                      | slay ticket (`--external-id`) → worktree → launch Claude |
| Work finished                      | `gh pr create` → `gh pr merge --squash --auto`      |
| Merged                             | Remove worktree + branch, `slay tasks done`         |

## Project conventions

- Language: Rust (edition per `Cargo.toml`). Format with `cargo fmt`; lint with
  `cargo clippy`. Run `cargo test` before opening a PR.
- Keep modules small and single-purpose (see the design spec's emphasis on
  isolated, independently testable units).
- Never commit secrets. Never force-push shared branches.
