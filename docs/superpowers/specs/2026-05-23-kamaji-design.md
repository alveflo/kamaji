# kamaji — Design Specification

- **Date:** 2026-05-23
- **Status:** Approved (design); implementation plan pending
- **Author:** Victor Alveflo
- **Repo:** `git@github.com:alveflo/kamaji.git`

## 1. Overview

**kamaji** is a terminal UI that orchestrates AI coding agents (Claude Code,
Codex, Copilot) as [zellij](https://zellij.dev) sessions, organized on a
per-project Kanban board with four columns: **Todo → In Progress → Review →
Done**.

Each project is a name plus a root directory. Tickets describe a unit of work
and name the agent that should do it. Moving a ticket into **In Progress**
creates an isolated git worktree, generates a zellij layout that launches the
chosen agent (seeded with an optional initial prompt), and drops the user into
that session. Detaching returns the user to the board; the session keeps
running in the background and can be re-attached at any time.

### Goals
- Fast, keyboard-driven orchestration of multiple concurrent agent sessions.
- True isolation between concurrent tickets via one git worktree per ticket.
- Zero ceremony: create a ticket, move it, and you are working in the agent.

### Non-goals (v1)
- Automatic detection of when an agent "needs input" (auto-move to Review).
  Deferred — see §11. Moves between columns are manual in v1.
- Multi-user / shared boards. Single user, single machine.
- In-column drag reordering of tickets.

## 2. Technology

| Concern        | Choice                                            |
|----------------|---------------------------------------------------|
| Language        | Rust                                              |
| TUI framework   | [ratatui](https://ratatui.rs)                     |
| Persistence     | SQLite (single global DB)                          |
| Terminal mux    | zellij ≥ 0.43 (CLI-driven)                         |
| Agents          | Claude Code, Codex, Copilot (command templates)   |

Rust + ratatui gives a single static binary, tight control over suspending the
TUI and `exec`-ing zellij, and alignment with zellij's own ecosystem.

## 3. Data model

Global SQLite database at `~/.local/share/kamaji/kamaji.db` (XDG data dir;
honor `$XDG_DATA_HOME`). WAL mode enabled.

```sql
CREATE TABLE projects (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL,
    root_dir      TEXT NOT NULL,            -- absolute path; must be a git repo
    default_agent TEXT,                     -- claude | codex | copilot
    created_at    TEXT NOT NULL
);

CREATE TABLE tickets (
    id             INTEGER PRIMARY KEY,
    project_id     INTEGER NOT NULL REFERENCES projects(id),
    title          TEXT NOT NULL,
    description    TEXT NOT NULL DEFAULT '',
    initial_prompt TEXT,                     -- nullable; empty => bare agent
    agent          TEXT NOT NULL,            -- claude | codex | copilot
    status         TEXT NOT NULL DEFAULT 'todo',  -- todo|in_progress|review|done
    position       INTEGER NOT NULL DEFAULT 0,    -- ordering within a column
    session_name   TEXT,                     -- zellij session name once started
    worktree_path  TEXT,                     -- absolute path once started
    branch         TEXT,                     -- git branch once started
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);
```

`session_name`, `worktree_path`, and `branch` are null until the ticket first
enters In Progress.

## 4. Configuration

`~/.config/kamaji/config.toml` (honor `$XDG_CONFIG_HOME`):

```toml
# Global default agent; pre-fills the new-ticket form. Overridable per project.
default_agent = "claude"

# Where per-ticket worktrees are created. {root} = project root_dir.
# Default keeps worktrees OUTSIDE the main working tree.
worktree_base = "{root}/../kamaji-worktrees"

# Base branch to create ticket branches from.
# "auto" => repo default branch (origin/HEAD), falling back to current HEAD.
base_branch = "auto"

[agents.claude]
with_prompt = ["claude", "{prompt}"]
no_prompt   = ["claude"]

[agents.codex]
with_prompt = ["codex", "{prompt}"]
no_prompt   = ["codex"]

[agents.copilot]                 # template only; CLI not yet validated
with_prompt = ["copilot", "{prompt}"]
no_prompt   = ["copilot"]
```

Command templates are stored as argv arrays (no shell). `{prompt}` is replaced
with the ticket's `initial_prompt`; when the prompt is empty the `no_prompt`
form is used.

## 5. Agents

All three agents are first-class. Defaults verified against the installed CLIs:
both `claude [prompt]` and `codex [PROMPT]` accept a positional prompt and start
an interactive session seeded with it. Copilot is shipped as an editable
template and validated when its CLI is available.

The default agent resolves in this order: ticket value → project
`default_agent` → global `default_agent`.

## 6. Worktree & session lifecycle

Naming for ticket *N* with title slug *S* (slug = lowercased title, non
-alphanumerics collapsed to `-`, truncated): `kamaji-<N>-<S>` is used uniformly
for the **branch**, the **worktree directory name**, and the **zellij session
name**.

**On first move Todo → In Progress:**
1. Verify `root_dir` is a git repository. If not, abort the move with a clear
   error (no partial state).
2. `git -C <root_dir> worktree add <worktree_base>/kamaji-<N>-<S> -b kamaji-<N>-<S> <base_branch>`.
3. Generate a zellij KDL layout (see §8) whose pane `cwd` is the new worktree
   and whose command is the resolved agent argv.
4. Persist `session_name`, `worktree_path`, `branch`; set status `in_progress`.
5. Suspend the TUI and `exec zellij --session kamaji-<N>-<S> -n <layout>`
   (creates **and** attaches in one step).
6. When the user detaches (zellij default `Ctrl+o d`), the zellij server keeps
   the session alive; kamaji resumes and refreshes session liveness via
   `zellij list-sessions`.

**Re-entering In Progress / Attach (`a`):** if a session already exists for the
ticket, kamaji attaches (`exec zellij attach <session>`) rather than recreating
it.

**Moving backward** (e.g. In Progress → Todo): leaves the worktree and session
intact so no work is lost. Only Done triggers cleanup.

**On move → Done:** prompt the user. If confirmed: `zellij kill-session`,
`git -C <root_dir> worktree remove <path>`, and delete the branch. If declined,
leave everything intact. (Branch deletion is local; remote/PR handling is the
agent's job, not kamaji's.)

**Delete ticket (`d`):** confirm; if the ticket has a live session/worktree,
offer the same cleanup as Done.

## 7. UI & keybindings

Four columns rendered side by side; the selected ticket is highlighted. A
status bar shows the active project and key hints.

```
┌ Todo ─────────┐┌ In Progress ──┐┌ Review ───────┐┌ Done ─────────┐
│ #3 Add login  ││▶#1 Refactor db││ #5 Flaky test ││ #2 Bump deps  │
│ #4 Dark mode  ││ #6 API docs   ││               ││               │
│               ││               ││               ││               │
└───────────────┘└───────────────┘└───────────────┘└───────────────┘
 project: acme-api · claude  [c]reate [m]ove [a]ttach [o]pen [p]roject [?]help [q]uit
```

| Key        | Action                                                              |
|------------|---------------------------------------------------------------------|
| `↑`/`↓` `j`/`k` | Move ticket selection within the focused column                |
| `←`/`→` `h`/`l` | Move focus between columns                                     |
| `c`        | Create ticket (modal form) → added to Todo                          |
| `m`        | Move-mode for selected ticket: `←`/`→` pick column, `Enter` confirm, `Esc` cancel |
| `a`        | Attach to selected ticket's session (suspend TUI, exec attach)      |
| `o` / `Enter` | Open ticket modal: view/edit Title & Description                 |
| `p`        | Project switcher (select existing or create new)                    |
| `d`        | Delete selected ticket (with confirm + optional cleanup)            |
| `?`        | Help overlay                                                        |
| `Esc`      | Close modal / cancel                                                |
| `q`        | Quit                                                                |

**Create-ticket modal fields:** Title, Description, Initial Prompt (optional),
Agent (pre-filled with the resolved default).

**Project switcher / startup:** on launch, kamaji shows the project list. A
project is created with a name and a root directory. The last-used project is
remembered.

## 8. zellij layout generation

For each session kamaji writes a temporary KDL layout, e.g. for a Claude ticket
with an initial prompt:

```kdl
layout {
    pane command="claude" cwd="/home/victor/dev/kamaji-worktrees/kamaji-1-refactor-db" {
        args "Refactor the database access layer to use a repository pattern"
    }
}
```

With no initial prompt the `args` line is omitted. The exact KDL spelling is
confirmed during implementation against zellij 0.43.

## 9. Core flows (summary)

1. **Start** → pick/create project → board renders from SQLite.
2. **Create** (`c`) → modal → ticket in Todo.
3. **Move to In Progress** (`m`) → worktree + session created → auto-attach →
   detach returns to board.
4. **Attach** (`a`) → re-enter a running session.
5. **Open** (`o`) → edit title/description.
6. **Move to Done** → prompt to clean up worktree + session.

## 10. Error handling & edge cases

- **Root not a git repo:** moving to In Progress aborts with a clear message;
  ticket stays in Todo.
- **Agent CLI missing** (e.g. copilot): session launch fails fast with the
  command that could not be run.
- **Session name already exists:** attach instead of recreate.
- **Worktree already exists** for the branch: reuse it rather than failing.
- **External session death:** on resume, kamaji reconciles ticket state against
  `zellij list-sessions` and marks sessions that no longer exist.
- **Single instance assumed:** SQLite WAL; concurrent kamaji instances are not a
  supported scenario in v1.

## 11. Deferred: auto-move to Review

Detecting that an agent is waiting for user input (to auto-move a ticket to
Review) is **out of scope for v1**. The intended approach — periodically poll
each running session with `zellij action dump-screen` and match per-agent
idle/prompt patterns — is captured in
[issue #1](https://github.com/alveflo/kamaji/issues/1) for later pickup. Until
then, moves to Review are manual.

## 12. Risks & open items

- zellij KDL `command`/`args`/`cwd` exact syntax — confirm during implementation.
- Suspend-TUI / `exec` / resume cycle in ratatui (raw mode teardown and
  restore) needs care; small spike at the start of implementation.
- `base_branch = "auto"` detection (`origin/HEAD`) behavior on repos without a
  remote — fall back to current HEAD.

## 13. Milestones (high-level; detailed plan to follow)

1. Project + ticket CRUD over SQLite, board rendering, keyboard nav.
2. Project switcher and create flow.
3. Worktree creation + zellij layout generation + suspend/exec/resume attach.
4. Move semantics (incl. Done cleanup prompt) and delete.
5. Config file + agent command templates.
6. Polish: help overlay, error surfacing, session reconciliation.
