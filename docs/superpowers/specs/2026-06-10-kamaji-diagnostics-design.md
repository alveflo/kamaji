# kamaji diagnostics — `kamaji doctor` + persistent daemon logs

**Date:** 2026-06-10
**Status:** Approved design, ready for implementation planning.

## Problem

macOS users report two failures that we cannot currently root-cause:

1. Creating a task with "run in background" enabled fails with a generic
   "server error" in the TUI.
2. From the web interface, attaching to a zellij session shows a crashed
   iframe page.

Both failures are almost certainly produced **inside the daemon** — the daemon
spawns `zellij` (for `attach --create-background`) and the `zellij web`
subprocess that the proxy forwards to. When either spawn fails, the daemon logs
the real cause via `tracing::error!` in `ApiError::Internal`
(`crates/kamajid/src/error.rs`) and returns a generic 500 / the proxy returns a
502.

**The blocker:** the daemon that a normal user runs is auto-spawned by the TUI
via `spawn_detached` (`crates/kamaji/src/daemon.rs:227-231`), which redirects
**both stdout and stderr to `/dev/null`**. The `/tmp/kamajid.log` file only
exists when the daemon is started via `make start`, which no end user does. So
the actionable detail behind "server error" is discarded at the source, and the
user can only report the generic message.

The leading hypothesis (unverified — we develop on Linux) is that the
auto-spawned daemon on macOS inherits a minimal PATH without Homebrew
(`/opt/homebrew/bin`, `/usr/local/bin`), so `Command::new("zellij")` fails for
both code paths. Secondary candidates: macOS temp-dir (`/var/folders/...`)
write failures for the layout/web-config files, and IPv4/IPv6 (`127.0.0.1` vs
`::1`) reachability of the `zellij web` port.

We are **not** fixing the bugs in this work. We are building the diagnostics
that let a remote macOS user produce the evidence needed to root-cause them.

## Goals

- The daemon's logs persist to a known location regardless of how it was
  started (TUI auto-spawn, `make start`, systemd).
- A single `kamaji doctor` command produces a shareable report that surfaces
  the most likely culprits — crucially, **the daemon's own view** of its PATH
  and zellij resolution, not just the TUI's.
- Works even when the daemon is down (local checks still run).

## Non-goals

- Fixing the macOS bugs (separate follow-up, informed by the evidence this
  produces).
- Changing `spawn_detached`'s `/dev/null` redirect (unnecessary once the daemon
  writes its own log file).
- Log shipping, telemetry, or any network egress.

## Architecture

Three pieces, with the gathering logic shared in core so the CLI and the daemon
agree on what a check means.

### 1. `kamaji_core::diagnostics` (new module)

Pure, serializable gathering primitives that any process can call. No daemon
I/O, no rendering. Each check returns a structured result carrying a verdict
and a human-readable hint.

```rust
pub enum Verdict { Ok, Warn, Fail }

pub struct Check {
    pub name: String,        // e.g. "zellij on PATH"
    pub verdict: Verdict,
    pub detail: String,      // e.g. resolved path / version / error
    pub hint: Option<String> // shown only on Warn/Fail
}
```

Primitives (each returns one or more `Check`s, all `serde::Serialize`):

- **Binary resolution:** resolve `zellij` and `git` on the *current process's*
  PATH; run `<bin> --version` and capture output. Fail with a hint when not
  found.
- **Temp-dir writability:** `std::env::temp_dir()` plus the two subdirs kamaji
  uses (`kamaji-layouts`, `kamaji-zellij`) — attempt create_dir_all + write +
  remove of a probe file; report the resolved path and any error.
- **XDG dirs:** resolve config/data/cache/runtime dirs (via
  `kamaji_core::paths`); report path, existence, and writability.
- **zellij config:** resolved config file path and whether it is readable.
- **Env allowlist:** capture only `PATH, HOME, SHELL, TMPDIR` and any
  `XDG_*` / `ZELLIJ*` vars. **Never** a full environment dump.

A top-level `gather_local() -> LocalReport` aggregates these into a serializable
struct.

### 2. `kamajid` `GET /diagnostics`

A new route that calls `kamaji_core::diagnostics::gather_local()` **from inside
the daemon process** (so PATH, temp-dir, and env reflect the *daemon's*
environment — the source of truth for both bugs) and augments it with
daemon-only live state:

- daemon version, uptime, pid, bound address;
- a **real zellij spawn probe** as the daemon sees it (resolution + `--version`
  via `kamaji_core::zellij::command()`, which mirrors how sessions are actually
  launched, including the `ZELLIJ*` env scrubbing);
- `zellij web` subprocess state + whether port :8082 is reachable;
- reverse-proxy bind state (port :8756);
- `zellij list-sessions` output;
- counts: projects, tickets.

Returns a `serde_json` body (`DaemonReport`) that embeds the `LocalReport`.
Errors gathering any single field degrade to a `Fail` check rather than failing
the whole endpoint.

### 3. `kamaji doctor` (new CLI subcommand)

In `crates/kamaji/src/cli.rs` (the existing hand-rolled parser) + a handler in
`crates/kamaji/src/main.rs`. It:

1. Runs `kamaji_core::diagnostics::gather_local()` locally (the TUI's
   environment).
2. Attempts `GET /diagnostics` against the daemon (discovered via the existing
   pidfile/addrfile logic; respects `--daemon`). On failure, records "daemon
   unreachable" and still prints the local section.
3. Reads the last ~50 lines of the daemon log file (known path, see below).
4. Merges and renders the report, **flagging mismatches** — most importantly
   "zellij is on *your* PATH but the daemon cannot find it", which is the
   smoking gun for the macOS reports.

## Persistent daemon logging

The daemon owns its log destination via a `tracing` file layer
(`tracing-appender`, daily-rolling, keep last ~5 files) writing to
`<cache_dir>/kamaji/kamajid.log`, **in addition to** the existing console
layer. `init_tracing` in `crates/kamajid/src/main.rs` gains the file layer; the
non-blocking writer guard is returned from `init_tracing` and held in `main`
for the process lifetime.

This makes logs consistent no matter how the daemon was started, and gives
`kamaji doctor` a known path to tail. The resolved log path is exposed through
`kamaji_core::paths` (or a small helper) so both the daemon and `kamaji doctor`
agree on it.

## Output & format

`kamaji doctor` prints a human-readable report by default:

- Sectioned: **Versions**, **Local environment**, **Daemon**, **zellij**,
  **Recent daemon logs**.
- Each check on its own line with a `[ok]` / `[warn]` / `[fail]` marker and a
  one-line hint on failure.
- A short summary verdict at the bottom (e.g. "2 problems found").

A `--json` flag emits the merged structured report for pasting into issues.

Env reporting is allowlisted (above) — never a full env dump — to avoid leaking
secrets.

## Error handling

- No single failed check aborts the report; every gather step degrades to a
  `Fail`/`Warn` `Check` with the error in `detail`.
- A down daemon is an expected state, not an error: the daemon section renders
  "unreachable" and local checks still print.
- The daemon log file is best-effort: if absent/unreadable, the "Recent daemon
  logs" section says so.

## Testing

- **Core `diagnostics`:** unit tests — writability checks against tempdirs,
  version-string parsing against canned output, verdict/hint logic, env
  allowlist filtering.
- **Daemon endpoint:** route test asserting 200 + body shape, using the
  existing `ZellijWeb::fake()` test seam.
- **`kamaji doctor` renderer:** tested against constructed report structs —
  ok/warn/fail rendering, the daemon-unreachable path, and the PATH-mismatch
  flag.
- No new end-to-end tests required.

## Module boundaries summary

| Unit | Responsibility | Depends on |
|------|----------------|------------|
| `kamaji_core::diagnostics` | Gather + serialize local checks; verdict model | `paths`, `zellij`, std |
| `kamajid` `GET /diagnostics` | Daemon-side live state + embed local report | core `diagnostics`, `state`, `zellij_web` |
| `kamaji doctor` (cli + handler + renderer) | Merge local + daemon, tail logs, render | core `diagnostics`, daemon client |
| daemon file-log layer | Persist daemon logs to known path | `tracing-appender`, `paths` |
