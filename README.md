# kamaji

A Kanban board that orchestrates AI coding agents (Claude Code, Codex, Copilot)
as [zellij](https://zellij.dev) sessions. Each ticket gets its own isolated git
worktree; moving a ticket to **In Progress** creates the worktree, launches the
agent inside a dedicated zellij session, and drops you straight into it. Detach
and the session keeps running in the background.

kamaji has two front-ends over one shared daemon (`kamajid`):

- a **terminal UI** (`kamaji`) — a [ratatui](https://ratatui.rs) board you drive
  from the keyboard, attaching to agent sessions in your terminal; and
- a **browser board** served by the daemon at **http://127.0.0.1:8755**, with
  agent terminals embedded live in the page.

Both talk to the same `kamajid` daemon, so the board, tickets, and sessions stay
in sync no matter which one you use.

```
┌ Todo ─────────┐┌ In Progress ──┐┌ Review ───────┐┌ Done ─────────┐
│ ○ #3 Add login││ ● #1 Refactor ││ ● #5 Flaky    ││ ○ #2 Bump deps│
│ ○ #4 Dark mode││ ● #6 API docs ││               ││               │
│               ││               ││               ││               │
└───────────────┘└───────────────┘└───────────────┘└───────────────┘
 project: acme-api  [↵]attach [s]main [e]dit [c]reate [m]ove [d]elete [/]search [t]heme [p]roject [?]help [q]uit
```

## Architecture

kamaji is a Cargo workspace of three crates plus a long-lived daemon:

| Crate | Kind | Role |
|---|---|---|
| `kamaji-core` | library | Shared domain: SQLite store, config, XDG paths, zellij helpers. Used by both the client and the daemon. |
| `kamajid` | daemon (bin + lib) | The brain. Owns the SQLite database, ticket lifecycle, and background session starts. Serves the browser board and an HTTP/SSE API. |
| `kamaji` | TUI / CLI client | The ratatui board and the `kamaji ticket create` command. A thin client that talks to `kamajid` over HTTP + SSE. |

**The daemon (`kamajid`)** is the single source of truth. It listens on three
ports:

| Port | What it serves |
|---|---|
| **8755** | The browser Kanban board (maud-rendered HTML) **and** the HTTP/SSE API both front-ends use. |
| **8756** | The terminal proxy — an authenticating reverse proxy in front of `zellij web`, which the browser board embeds as a per-ticket terminal `<iframe>`. |
| **8082** | Upstream `zellij web`, spawned and managed by the daemon. Not meant to be hit directly; the proxy on 8756 fronts it. |

**Transport.** State changes stream from the daemon to both front-ends over
**Server-Sent Events**. The browser board is a [Datastar](https://data-star.dev)
hypermedia app — a vendored `datastar.js` module subscribes to an SSE stream
(`/ui/events`) and the daemon pushes HTML fragments to patch the board live (no
client-side framework, no build step). The TUI consumes the parallel `/events`
stream and re-renders in the terminal.

**You don't start the daemon by hand.** Both `kamaji` (the TUI) and
`kamaji ticket create` auto-spawn a detached `kamajid` if one isn't already
healthy (pidfile + `/healthz` probe, race-safe), then connect to it. `make start`
is provided for running the daemon standalone (e.g. when you only want the
browser board).

## Install

**Linux / macOS** (x86_64 / aarch64):

```sh
curl -fsSL https://raw.githubusercontent.com/alveflo/kamaji/main/install.sh | sh
```

This downloads a prebuilt binary for your platform to `~/.local/bin`. Override
the location with `KAMAJI_INSTALL_DIR`:

```sh
curl -fsSL https://raw.githubusercontent.com/alveflo/kamaji/main/install.sh | KAMAJI_INSTALL_DIR=/usr/local/bin sh
```

**Windows** (x86_64, PowerShell):

```powershell
irm https://raw.githubusercontent.com/alveflo/kamaji/main/install.ps1 | iex
```

This installs `kamaji.exe` to `%LOCALAPPDATA%\Programs\kamaji` and adds it to
your user PATH. Override the location with the `KAMAJI_INSTALL_DIR` environment
variable. kamaji drives `zellij.exe`, so you'll also need
[zellij for Windows](https://zellij.dev/documentation/installation.html) on
`PATH`.

kamaji checks for new releases on launch. When one is available the status bar
shows `New version vX.Y.Z available — press u to update`; press `u` to download
and replace the binary in place, then restart.

## Features

- Four-column board: **Todo → In Progress → Review → Done**
- Two front-ends over one daemon: a terminal UI **and** a browser board at
  http://127.0.0.1:8755, kept in sync live over Server-Sent Events
- Agent terminals embedded directly in the browser board (via a `zellij web`
  proxy)
- One git worktree per ticket — agents never step on each other
- zellij session per ticket; detach and re-attach at any time
- Supports Claude Code, Codex, and Copilot via configurable command templates
- Optional per-ticket initial prompt seeds the agent on first launch
- SQLite persistence owned by the daemon; single global database
- Built-in colorschemes — Catppuccin, Tokyo Night, Gruvbox, Nord, plus a
  terminal-default mode — switchable live in-app with `t`

## Requirements

- **Rust** toolchain (for building from source)
- **[zellij](https://zellij.dev)** ≥ 0.43 on `$PATH` — also provides `zellij web`,
  which the daemon uses to stream terminals into the browser board
- At least one agent CLI on `$PATH`: `claude`, `codex`, or `copilot`
- **git** on `$PATH`
- Project roots must be git repositories
- A modern browser, if you want to use the browser board (the terminal UI needs
  none)

## Build and run

This is a Cargo workspace (`kamaji-core`, `kamaji`, `kamajid`).

```bash
# Build everything (debug)
cargo build

# Build the release binaries
cargo build --release
# → target/release/kamaji  (TUI / CLI client)
# → target/release/kamajid (daemon)

# Run the TUI directly (dev). It auto-spawns a kamajid daemon if one isn't
# already running, then connects to it.
cargo run -p kamaji
```

### Running the daemon standalone

You normally never do this — the TUI and CLI auto-spawn the daemon. But to run
just the browser board (no terminal UI), a `Makefile` wraps the daemon's
lifecycle:

```bash
make start     # build kamajid and launch it in the background
make status    # is the daemon responding? (probes :8755/healthz)
make logs      # follow the daemon log (/tmp/kamajid.log)
make restart   # rebuild + relaunch (use after pulling new code)
make stop      # stop the running daemon
```

With the daemon up, open **http://127.0.0.1:8755** in a browser. Equivalently:

```bash
cargo run -p kamajid -- serve            # default bind 127.0.0.1:8755
cargo run -p kamajid -- serve --bind 127.0.0.1:9000
```

The daemon binds the terminal proxy on the board port **+ 1** (so 8756 by
default) and manages an upstream `zellij web` on **:8082**.

### Tests

```bash
cargo test                    # whole workspace
cargo fmt --all               # format
cargo clippy --all-targets    # lint
```

## Global state

kamaji uses XDG base directories (honoring `$XDG_DATA_HOME` and
`$XDG_CONFIG_HOME`):

| Purpose         | Default path                                          |
|-----------------|-------------------------------------------------------|
| SQLite database | `~/.local/share/kamaji/kamaji.db`                     |
| Configuration   | `~/.config/kamaji/config.toml`                        |
| Daemon pid/addr | `$XDG_RUNTIME_DIR/kamaji/kamajid.{pid,addr}` (falls back to `$XDG_CACHE_HOME`, then `~/.cache`) |

The config file is written with defaults on first run if it does not exist. The
daemon's pidfile and address file let the TUI/CLI find (or avoid double-spawning)
a running `kamajid`.

## Configuration

`~/.config/kamaji/config.toml`:

```toml
default_agent = "claude"
worktree_base = "{root}/../kamaji-worktrees"
base_branch = "auto"
zellij_bar = "auto"
theme = "catppuccin"

# Daemon settings (all optional; shown with their defaults). The terminal proxy
# binds the board port + 1, so a bind of 127.0.0.1:8755 puts the proxy on :8756.
[daemon]
bind = "127.0.0.1:8755"
log_format = "human"   # "human" or "json"
log_level = "info"
web_theme = "auto"     # "auto" | "match" | a zellij theme name

[agents.claude]
with_prompt = ["claude", "{prompt}"]
no_prompt = ["claude"]
resume = ["claude", "--continue"]

[agents.codex]
with_prompt = ["codex", "{prompt}"]
no_prompt = ["codex"]
resume = ["codex", "resume", "--last"]

[agents.copilot]
with_prompt = ["copilot", "-i", "{prompt}"]
no_prompt = ["copilot", "-i"]
resume = ["copilot", "--continue"]
```

**Key settings:**

| Setting | Description |
|---|---|
| `default_agent` | Pre-fills the agent field when creating a ticket. |
| `worktree_base` | Where worktrees are created. `{root}` expands to the project's root directory. Default places them alongside (not inside) the main working tree. |
| `base_branch` | Branch new ticket branches are created from. `auto` detects the repo's default branch (`origin/HEAD`), falling back to the current `HEAD`. |
| `zellij_bar` | Bar style for spawned sessions. `auto` (default) matches your zellij `default_layout` (`compact` → compact bar, otherwise tab-bar + status-bar). Force a style with `compact`, `default`, or `none` (no bars). |
| `theme` | Colorscheme: `catppuccin` (default), `tokyonight`, `gruvbox`, `nord`, or `default` (uses your terminal's own 16 colors). Switch live from the board with `t` (the choice is saved back here). Unknown names fall back to `catppuccin`. |
| `daemon.bind` | Address the daemon binds the board + HTTP/SSE API on (default `127.0.0.1:8755`). The terminal proxy binds the next port up. |
| `daemon.log_format` | Daemon log format: `human` (default) or `json`. |
| `daemon.web_theme` | Colorscheme for the browser zellij-web sessions. `auto` (default) respects your own zellij config — kamaji injects nothing. `match` pulls sessions toward the board's palette: it forces the built-in `catppuccin-mocha` theme on zellij's chrome **and** applies the board palette to the browser terminal. Any other value is a zellij theme name to force on the chrome. Forcing a theme is session-wide (also recolors the TUI view) and takes effect for sessions created after a daemon restart. |
| `daemon.log_level` | Daemon log filter (default `info`). Overridable at runtime with the `KAMAJID_LOG` env var. |
| `agents.<name>.with_prompt` | Argv array used when the ticket has an initial prompt. `{prompt}` is replaced with the prompt text. |
| `agents.<name>.no_prompt` | Argv array used when no initial prompt is set. |
| `agents.<name>.resume` | Argv array used to resume a session that survived a reboot (see [Persistent sessions](#persistent-sessions)). If omitted, kamaji derives a default from the binary (`<bin> --continue`, or `codex resume --last`). |

Command templates are passed directly as argv (no shell). Add or edit agent
entries to support other CLIs.

## Usage

You can drive the board from the **terminal UI** or the **browser**. Both are
backed by the same daemon, so changes made in one show up live in the other.

### Browser board

1. Make sure a daemon is running. Either launch the TUI once (it spawns one) or
   run `make start`.
2. Open **http://127.0.0.1:8755**.
3. Pick a project in the sidebar, create and move tickets, and click a ticket to
   open its embedded terminal — the running agent's zellij session, streamed
   into the page through the proxy on `:8756`. The board updates live over SSE as
   sessions start and agents work.

### Startup (terminal UI)

On launch `kamaji` shows a project picker. Select an existing project with
`↑`/`↓` and `Enter`, or press `n` to create a new project (name + root
directory). You can return to the picker at any time from the board by
pressing `p`.

### Typical workflow (terminal UI)

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

4. **Re-attach** — select the ticket and press `Enter` to re-enter the session.

5. **Move to Review** — press `m` and navigate to **Review**, then `Enter`. You
   can also let the daemon do it: with `[auto_review]` enabled (the default), a
   ticket moves to Review on its own once its agent goes idle (see
   [Auto-move to Review](#auto-move-to-review)).

6. **Complete** — press `m` and move the ticket to **Done**. kamaji prompts
   whether to clean up: `y` kills the zellij session, removes the worktree, and
   deletes the branch. `n` moves the ticket to Done and leaves everything
   intact.

### Command line

Create a Todo ticket without opening the TUI:

```bash
kamaji ticket create --prompt "Start working on GitHub issue #123"
```

By default, kamaji infers the project from the current directory. This works
inside a registered project root and inside worktrees recorded on existing
tickets, so a running agent can create follow-up tickets from its zellij
session. Use `--project <id-or-name>` when inference is ambiguous.

Useful options:

```bash
kamaji ticket create \
  --project kamaji \
  --title "GitHub issue #123" \
  --description "Optional context shown on the ticket" \
  --agent claude \
  --prompt "Start working on GitHub issue #123"
```

If `--title` is omitted, the ticket title defaults to the first line of the
prompt.

### Notes on session state

- Moving a ticket *backward* (e.g. In Progress → Todo) leaves the worktree and
  session intact so no work is lost.
- A filled circle `●` next to a ticket title means a session has been created
  for it (the session name is recorded on the ticket); an empty circle `○` means
  none has been started yet.

### Persistent sessions

Sessions survive a reboot. kamaji records each ticket's session in its SQLite
database, and zellij serializes its own sessions to disk, so after a restart a
ticket still maps to its (now exited) zellij session and its git worktree is
still on disk.

When you `Enter` (or move to In Progress) a ticket whose zellij session is in
the exited/resurrectable state — which is how sessions appear after the machine
restarts — kamaji recreates the session in the *same worktree* but launches the
agent with its **resume** command (`agents.<name>.resume`, e.g. `claude
--continue`) instead of replaying the original prompt. The agent picks up its
previous conversation rather than starting fresh. Live sessions still attach
unchanged, and a session that is truly gone starts a new conversation.

This relies on zellij's session serialization, which is on by default. If you
have disabled it in your zellij config, exited sessions won't be recoverable.

## Keybindings (terminal UI)

These apply to the `kamaji` terminal UI. The browser board is mouse-driven.

### Board

| Key | Action |
|---|---|
| `↑` / `k` | Select ticket above |
| `↓` / `j` | Select ticket below |
| `←` / `h` | Focus column to the left |
| `→` / `l` | Focus column to the right |
| `Enter` | Attach to the selected ticket's session, or start it (creates the worktree + zellij session and moves the ticket to In Progress) |
| `s` | Open the project's **main session** — a workspace not tied to any ticket. Runs the project's default agent in the project root (no worktree); attaches if it's already running |
| `e` | Edit selected ticket (title and description) |
| `c` | Create ticket (opens form modal) |
| `m` | Move selected ticket (opens move modal; use `←`/`→` to pick column, `Enter` to confirm, `Esc` to cancel) |
| `d` | Delete selected ticket (prompts for confirmation and optional cleanup) |
| `/` | Search / filter tickets by title (`Esc` clears) |
| `t` | Switch theme (opens a picker; `↑`/`↓` preview live, `Enter` saves, `Esc` cancels) |
| `u` | Update kamaji (shown only when a newer release is available) |
| `p` | Switch project (returns to the project picker) |
| `?` | Help overlay |
| `q` | Quit |

### In a zellij session

| Key | Action |
|---|---|
| `Ctrl+o d` | Detach from session (returns to kamaji board) |

## Auto-move to Review

The daemon runs a poll loop that watches in-progress agent sessions and moves a
ticket to **Review** when its agent goes idle (waiting for input). Detection
polls each session's screen and matches per-agent idle patterns; the resulting
move is broadcast over SSE, so both front-ends update without you touching the
board. A later manual move clears the auto-review provenance.

Configure it under `[auto_review]` in `config.toml`:

```toml
[auto_review]
enabled = true            # set false to keep all moves manual
poll_interval_secs = 5

# Extra idle-detection patterns per agent, if the defaults miss your setup.
[auto_review.patterns]
codex = []
copilot = []
```

Per-session activity also surfaces live: a "working" indicator on each ticket,
streamed from the daemon as the agent runs.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the human basics — building, testing,
running the daemon, and the PR flow.

For the AI-coding-agent working agreement (worktrees, issue→task flow), see
[AGENTS.md](AGENTS.md).
