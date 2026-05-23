# kamaji

A terminal Kanban board that orchestrates AI coding agents (Claude Code, Codex,
Copilot) as [zellij](https://zellij.dev) sessions. Each ticket gets its own
isolated git worktree; moving a ticket to **In Progress** creates the worktree,
launches the agent inside a dedicated zellij session, and drops you straight
into it. Detach and the session keeps running in the background.

```
┌ Todo ─────────┐┌ In Progress ──┐┌ Review ───────┐┌ Done ─────────┐
│ ○ #3 Add login││ ● #1 Refactor ││ ● #5 Flaky    ││ ○ #2 Bump deps│
│ ○ #4 Dark mode││ ● #6 API docs ││               ││               │
│               ││               ││               ││               │
└───────────────┘└───────────────┘└───────────────┘└───────────────┘
 project: acme-api  [c]reate [m]ove [a]ttach [o]pen [d]elete [p]roject [?]help [q]uit
```

## Features

- Four-column board: **Todo → In Progress → Review → Done**
- One git worktree per ticket — agents never step on each other
- zellij session per ticket; detach and re-attach at any time
- Supports Claude Code, Codex, and Copilot via configurable command templates
- Optional per-ticket initial prompt seeds the agent on first launch
- SQLite persistence; single global database

## Requirements

- **Rust** toolchain (for building from source)
- **[zellij](https://zellij.dev)** ≥ 0.43 on `$PATH`
- At least one agent CLI on `$PATH`: `claude`, `codex`, or `copilot`
- **git** on `$PATH`
- Project roots must be git repositories

## Build and run

```bash
# Build release binary
cargo build --release
# Binary is at target/release/kamaji

# Run directly (dev)
cargo run
```

## Global state

kamaji uses XDG base directories (honoring `$XDG_DATA_HOME` and
`$XDG_CONFIG_HOME`):

| Purpose        | Default path                          |
|----------------|---------------------------------------|
| SQLite database | `~/.local/share/kamaji/kamaji.db`    |
| Configuration  | `~/.config/kamaji/config.toml`        |

The config file is written with defaults on first run if it does not exist.

## Configuration

`~/.config/kamaji/config.toml`:

```toml
default_agent = "claude"
worktree_base = "{root}/../kamaji-worktrees"
base_branch = "auto"
zellij_bar = "auto"

[agents.claude]
with_prompt = ["claude", "{prompt}"]
no_prompt = ["claude"]

[agents.codex]
with_prompt = ["codex", "{prompt}"]
no_prompt = ["codex"]

[agents.copilot]
with_prompt = ["copilot", "{prompt}"]
no_prompt = ["copilot"]
```

**Key settings:**

| Setting | Description |
|---|---|
| `default_agent` | Pre-fills the agent field when creating a ticket. |
| `worktree_base` | Where worktrees are created. `{root}` expands to the project's root directory. Default places them alongside (not inside) the main working tree. |
| `base_branch` | Branch new ticket branches are created from. `auto` detects the repo's default branch (`origin/HEAD`), falling back to the current `HEAD`. |
| `zellij_bar` | Bar style for spawned sessions. `auto` (default) matches your zellij `default_layout` (`compact` → compact bar, otherwise tab-bar + status-bar). Force a style with `compact`, `default`, or `none` (no bars). |
| `agents.<name>.with_prompt` | Argv array used when the ticket has an initial prompt. `{prompt}` is replaced with the prompt text. |
| `agents.<name>.no_prompt` | Argv array used when no initial prompt is set. |

Command templates are passed directly as argv (no shell). Add or edit agent
entries to support other CLIs.

## Usage

### Startup

On launch kamaji shows a project picker. Select an existing project with
`↑`/`↓` and `Enter`, or press `n` to create a new project (name + root
directory). You can return to the picker at any time from the board by
pressing `p`.

### Typical workflow

1. **Create a ticket** — press `c`, fill in Title, Description, an optional
   Initial Prompt, and choose the Agent (`←`/`→`). Press `Enter` to save; the
   ticket appears in the Todo column.

2. **Start work** — select the ticket and press `m` to open move mode. Navigate
   to **In Progress** with `→` and press `Enter`.

   On first move to In Progress kamaji:
   - Creates a git worktree at `<worktree_base>/kamaji-<id>-<slug>`
   - Generates a zellij KDL layout that runs the agent (with the initial prompt
     if provided) inside that worktree
   - Launches `zellij` and auto-attaches to the new session

3. **Detach** — press `Ctrl+o d` (zellij default) to detach from the session.
   kamaji resumes and the board is visible again. The agent session keeps running
   in the background.

4. **Re-attach** — select the ticket and press `a` to re-enter the session.

5. **Move to Review** — press `m` and navigate to **Review**, then `Enter`.
   Moves are manual (see note below).

6. **Complete** — press `m` and move the ticket to **Done**. kamaji prompts
   whether to clean up: `y` kills the zellij session, removes the worktree, and
   deletes the branch. `n` moves the ticket to Done and leaves everything
   intact.

### Notes on session state

- Moving a ticket *backward* (e.g. In Progress → Todo) leaves the worktree and
  session intact so no work is lost.
- A filled circle `●` next to a ticket title means a session has been created
  for it (the session name is recorded on the ticket); an empty circle `○` means
  none has been started yet.

## Keybindings

### Board

| Key | Action |
|---|---|
| `↑` / `k` | Select ticket above |
| `↓` / `j` | Select ticket below |
| `←` / `h` | Focus column to the left |
| `→` / `l` | Focus column to the right |
| `c` | Create ticket (opens form modal) |
| `m` | Move selected ticket (opens move modal; use `←`/`→` to pick column, `Enter` to confirm, `Esc` to cancel) |
| `a` | Attach to selected ticket's zellij session |
| `o` / `Enter` | Open / edit selected ticket (title and description) |
| `d` | Delete selected ticket (prompts for confirmation and optional cleanup) |
| `p` | Switch project (returns to the project picker) |
| `?` | Help overlay |
| `q` | Quit |

### In a zellij session

| Key | Action |
|---|---|
| `Ctrl+o d` | Detach from session (returns to kamaji board) |

## Deferred: auto-move to Review

Automatic detection of when an agent is waiting for input (and auto-moving the
ticket to Review) is not yet implemented. Moves between columns are manual.
The planned approach — polling sessions via `zellij action dump-screen` and
matching per-agent idle patterns — is tracked in
[issue #1](https://github.com/alveflo/kamaji/issues/1).

## Contributing

See [AGENTS.md](AGENTS.md) for notes on the codebase and how to work with the
AI coding agents that helped build it.
