# Default agent picker (issue #46)

## Problem

kamaji defaults new tickets to Claude Code. The agent **resolution** chain is
already complete — explicit `--agent` → project `default_agent` → global
`Config.default_agent` (see `engine.rs` create handler and `cli.rs`
`run_create_ticket`). The `Config.default_agent` field already exists and
round-trips through `config.toml`.

The gap: there is **no in-app way to set** the global default agent. A user must
hand-edit `~/.config/kamaji/config.toml`. Issue #46 asks for a global setting
the user can actually change.

## Approach

Mirror the existing **ThemePicker** modal, which is the established pattern for
"set a persisted global setting from the board": a list modal, opened by a
hotkey, that writes the choice back to `config.toml` on Enter.

Differences from ThemePicker: selecting an agent changes nothing visible on the
board, so there is **no live preview** and **no `original` to restore**. The
modal only needs the highlighted index.

## Components

- **`Modal::AgentPicker { selected: usize }`** (`app.rs`) — `selected` indexes
  into `Agent::all()`.
- **Open handler** (`engine.rs`, board key) — bind `a`. Initialize `selected` to
  the index of the current global default (`config.default_agent()`), not the
  project override (this sets the *global* setting).
- **Key handling** (`engine.rs`, modal match):
  - `↑/↓` / `k`/`j` — move `selected`, clamped to `[0, Agent::all().len()-1]`.
  - `Enter` — set `config.default_agent = Agent::all()[selected].as_str()`,
    `save_to(config_path, config)`. On success set an info message
    (`default agent: <label>`); on failure set an error and leave config
    unchanged (do not partially apply).
  - `Esc` / other — close without saving.
- **Renderer** `render_agent_picker(frame, theme, selected)` (`ui/modals.rs`) —
  list `Agent::all()` labels with the `▸` marker + accent highlight, footer
  `↑/↓ select · ↵ save · Esc cancel`. Dispatched from `ui/mod.rs`.
- **Help** (`ui/modals.rs` `render_help`) — add `a  set default agent`.

## Testing (TDD)

- `Agent`: an `index_of(&str)` / position helper if needed (or reuse
  `Agent::all().iter().position`).
- Engine: pressing `a` opens `AgentPicker` initialized to the current default;
  `↓` then `Enter` persists the new default to a temp `config_path` and the
  reloaded config reflects it; `Esc` leaves config unchanged. Mirror the
  existing ThemePicker engine tests.
- Renderer: smoke test that `render_agent_picker` draws the themed border, like
  the existing modal render tests.

## Out of scope

- CLI command to set the default (TUI picker is the discoverable surface;
  matches the theme precedent).
- Per-project default agent editing (already settable at project creation;
  separate concern).
