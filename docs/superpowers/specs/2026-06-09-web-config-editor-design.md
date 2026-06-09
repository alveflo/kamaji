# Web config editor — design

**Date:** 2026-06-09
**Status:** approved, ready for implementation plan

## Problem

kamaji's configuration lives in `~/.config/kamaji/config.toml`. Today the only
way to change most of it is to hand-edit the file. The web board can edit a
mere three fields (`theme`, `default_agent`, `worktree_base`) via `PATCH
/config`; everything else — agent command templates, auto-review, daemon
settings — is file-only. There is no in-product way to discover or change them.

This feature gives the **browser board a full structured config editor** behind
a gear icon, and **clarifies the README** so TUI users know what is editable and
where the file lives.

## Scope

In scope:

1. A web config-editor modal (structured form) covering **every** config field,
   including the **three built-in agents'** command templates.
2. A gear icon at the **bottom of the side rail** that opens it.
3. README documentation of the config file location and what is editable where.

Explicitly **out of scope** (deferred to a follow-up issue):

- Arbitrary custom agent *providers* (Ollama and the like). The `Agent` enum
  (`Claude`/`Codex`/`Copilot`) stays a fixed enum; this feature only makes the
  three built-ins' flags/commands editable. Adding string-keyed custom agents is
  a separate, larger refactor across ~23 files.

## Decisions (resolved during brainstorming)

- **Agent model:** edit the three built-ins only; do not generalize to custom
  providers yet.
- **Config UI:** structured form (typed fields/sections), not a raw-TOML editor.
- **Modal scope:** *everything* editable. Fields that only take effect later are
  **labeled**, not auto-applied (no daemon-restart trigger from the form).
- **Argv editing:** agent command templates are edited as **space-separated
  single-line inputs** (familiar "type a command" UX). Documented caveat: a
  single argv token cannot contain a space. Auto-review *patterns* (which can
  contain spaces) use **newline-separated** textareas instead.
- **Write API:** add a new **`PUT /config`** that replaces the whole config;
  leave the existing `PATCH /config` untouched so the TUI's partial edits keep
  working.

## Architecture

Three layers, built bottom-up. The UI layer never touches the DB or disk
directly — it goes through `kamajid` routes, consistent with the rest of the
system.

### Part A — Backend: `PUT /config` (full replace)

In `crates/kamajid/src/routes/config.rs`:

- Keep `get_config` and `patch_config` exactly as they are.
- Add `put_config(State, Json<Config>)`:
  - The body is a complete `Config`. Because the form is rendered from `GET
    /config` it always carries every field; serde defaults backfill any key an
    older client omits.
  - **Validate** before persisting; on failure return `400` (`ApiError::BadRequest`)
    with a human message the modal shows inline:
    - `default_agent` parses as a known `Agent`.
    - For each of the three agents: `with_prompt` and `no_prompt` are non-empty,
      and `with_prompt` contains a `{prompt}` token somewhere in its argv.
    - `daemon.bind` parses as `std::net::SocketAddr`.
    - `daemon.log_format ∈ {"human","json"}`.
    - `auto_review.poll_interval_secs >= 1`.
  - On success: take the config write lock, replace the in-memory `Config`,
    `save_to(config_path(), &cfg)` on a blocking task (mirroring `patch_config`),
    and return the persisted `Config` as JSON.
- Wire `PUT` onto the existing `/config` route in `crates/kamajid/src/lib.rs`
  (the route already has `get(...).patch(...)`; add `.put(...)`).

A small reusable `validate_config(&Config) -> Result<(), String>` helper keeps
the handler readable and is unit-testable in isolation.

### Part B — Web view: `config_form` + gear icon

**New view** `crates/kamajid/src/views/config_form.rs`:

`pub fn config_form(cfg: &Config) -> Markup` — rooted at `#modal` (so Datastar's
`@get` morph-by-id replaces the page's `<div id="modal">` mount), built from the
shared modal chrome classes (`modal-head` / `modal-body` / `modal-foot` /
`field` / `seg` / `btn` / `hint`). Sections:

| Section | Fields |
|---|---|
| **General** | `default_agent` (segmented `.seg` control over `Agent::all()`, like `project_form`), `theme` (`<select>`: `default`, `catppuccin`, `tokyonight`, `gruvbox`, `nord`), `worktree_base` (mono input, `{root}` hint), `base_branch` (input), `zellij_bar` (`<select>`: `auto`/`compact`/`default`/`none`) |
| **Agents** — claude, codex, copilot | per agent: `with_prompt`, `no_prompt`, `resume` — each a single text `input` whose value is the argv joined by a space; hint notes `{prompt}` placeholder |
| **Auto-review** | `enabled` (checkbox), `poll_interval_secs` (number input, `min=1`), `patterns.codex` and `patterns.copilot` (newline-separated `textarea` each) |
| **Daemon** | `bind` (mono input, label "applies after daemon restart"), `log_format` (`<select>` human/json), `log_level` (input), `web_theme` (input, label "applies to sessions created after restart") |

**Submit:** an explicit `fetch('/config', {method:'PUT', headers:{'content-type':'application/json'}, body: …})`, following the `project_form` pattern (read controls via `f.elements['name']`, single-quoted JS only inside `PreEscaped` attributes, RC.6 `data-on:` colon bindings). The JS assembles the `Config` JSON, splitting the space-separated argv inputs into arrays and the newline-separated pattern textareas into arrays, and coercing the number/checkbox controls. On a 2xx it clears `#modal`; on a 4xx it re-renders with the inline error (the handler returns the message). Cancel / ✕ / Escape clear the mount, exactly like the other modals.

> **Theme keys:** the canonical theme registry lives in the TUI crate
> (`kamaji/src/theme.rs`), which `kamajid` does not depend on. The five known
> keys are hard-listed in `config_form` with a comment pointing back to the
> registry. Accepted minor duplication — theme keys change rarely.

**Gear icon** — in `crates/kamajid/src/views/sidebar.rs`: a `rail-settings`
row pinned at the bottom of the rail (alongside the existing `rail-add` row),
`data-on:click="@get('/ui/config')"`, rendering a gear glyph + "Settings" label
in the same tile style as "Add project".

**New route** `GET /ui/config` → handler in `crates/kamajid/src/routes/ui.rs`
that reads the current config and returns `config_form(&cfg)`. Wired in
`lib.rs`.

### Part C — README docs

The `## Configuration` section already tables every field. Augment it (no field
table rewrite):

- A lead-in paragraph: the **browser board has a full GUI editor** — the **gear
  icon at the bottom of the rail** opens a form covering every setting below.
  The **TUI** edits only the theme live (`t`); all other fields are changed by
  editing the file (or via the web editor).
- State the path precisely: `$XDG_CONFIG_HOME/kamaji/config.toml`, default
  `~/.config/kamaji/config.toml` (Windows uses the native config dir).

## Testing

- **`kamajid` integration tests** (`crates/kamajid/tests/`): `PUT /config`
  round-trips a valid config (response + on-disk file reflect it); rejects with
  `400` each invalid case — unknown `default_agent`, empty `with_prompt`/`no_prompt`,
  `with_prompt` missing `{prompt}`, unparseable `daemon.bind`, `poll_interval_secs`
  of 0, bad `log_format`.
- **`validate_config` unit tests** for each rule in isolation.
- **View tests** (`config_form`): rooted at `#modal`; renders shared chrome;
  every field present and pre-filled from a sample `Config` (agent argvs joined
  with spaces, patterns joined with newlines); restart labels present; submit
  wires `fetch('/config',{method:'PUT'`; RC.6 colon bindings, no hyphen form.
- **Sidebar test:** `rail-settings` row present and opens `@get('/ui/config')`.

## Risks / notes

- **Full-replace drift:** if a future config key is added but `config_form` is
  not updated to render it, a `PUT` from the stale form resets that key to its
  serde default. Mitigation: keep the form in sync when adding config keys; the
  form is the single rendering of the schema. `PATCH` (the TUI path) is
  unaffected.
- **Argv space caveat:** space-separated inputs cannot express a token
  containing a space. Acceptable for agent command templates (`claude {prompt}`,
  `codex resume --last`); documented in the field hint. Auto-review patterns,
  which legitimately contain spaces/box-drawing runs, use newline-separated
  textareas instead.
