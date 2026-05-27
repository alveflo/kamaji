# kamaji — Browser-First Pivot Design

- **Date:** 2026-05-27
- **Status:** Approved (architecture); phased — only Phase 0 is committed in full detail
- **Author:** Victor Alveflo
- **Supersedes (in part):** technology framing of `2026-05-23-kamaji-design.md` (single TUI binary). The domain model, worktree/session lifecycle, and zellij KDL details from that spec still hold; this spec changes only how the UI layers sit on top of them.

## 1. Overview

kamaji is pivoting from a single TUI binary into a **browser-first orchestrator
with a thinner TUI**, both driven by one shared backend. The board (projects +
tickets) and the ability to attach to a ticket's agent session become reachable
from either a web browser or the ratatui TUI; the browser is the first-class
surface where new features land, and the TUI is a deliberately leaner subset.

The pivot was unlocked by **zellij's web client** (0.43+): visiting
`http://127.0.0.1:8082/<session-name>` creates, attaches to, or resurrects a
named zellij session in the browser, with built-in token auth and persistence.
Because kamaji already names every ticket session deterministically
(`kamaji-<N>-<S>`), "attach in the browser" reduces to *a link to that URL* —
kamaji builds no terminal emulator. The terminal-in-browser problem is entirely
zellij's job; kamaji's browser job shrinks to rendering a good-looking Kanban
board over the existing domain logic.

### Goals
- One source of truth: a single backend owns the DB and all git/zellij
  orchestration; browser and TUI are thin clients of it.
- Live updates on every surface: a board change (ticket moved, session went
  idle, session died) pushes to all connected clients.
- Reuse the existing, well-factored Rust domain logic essentially as-is.
- Localhost-first, with cheap seams so a future remote/internet-facing mode is
  configuration rather than re-architecture.

### Non-goals (this pivot)
- Remote / internet-facing access, real auth, TLS, multi-user identity. Designed
  *for* but not *built* now (see §7).
- A bespoke browser terminal emulator. zellij web owns the terminal.
- Feature parity between TUI and browser. The TUI is intentionally the smaller
  surface going forward.

## 2. Decisions (from brainstorming)

| Question | Decision |
|----------|----------|
| Access model | Localhost first; remote/internet-facing possible later |
| Language | **Rust all the way** (Approach A) — no Elixir/Phoenix rewrite |
| TUI ↔ browser relationship | **One shared backend**; both are thin clients |
| Daemon lifecycle | **Auto-spawn on demand**; daemon outlives the client |
| zellij web | **kamaji manages it** (ensures it's running, handles the token) |
| Browser reactivity | **Datastar** (server-rendered HTML + SSE-driven DOM patching); htmx+SSE is the fallback |

The Elixir/Phoenix option was rejected because the two things that motivated it —
terminal-in-browser and a live-updating UI — are now (a) fully handled by zellij
web, and (b) achievable with Datastar+SSE from Rust for the board, which is a
small, well-bounded amount of real-time. A rewrite would discard the hardest-won
code (worktree + zellij CLI orchestration + auto-review detection) and weaken the
TUI, which Elixir serves poorly.

## 3. Architecture

Convert the single binary into a Cargo **workspace** of three crates:

### `kamaji-core` (library)
Pure domain logic — no UI, no transport. This is almost entirely today's code
lifted as-is: `db`, `models`, `git`, `slug`, `config`, `agent`, `layout`,
`zellij`, `zellij_config`, `detect`, plus the orchestration currently in
`engine`/`session`. It owns the SQLite DB and shells out to `git`/`zellij`
exactly as now. Its public API is a set of **commands** (`create_ticket`,
`move_ticket`, `start_session`, `attach_info`, …) and a **query** surface for
reading board state.

### `kamajid` (daemon)
Wraps `kamaji-core` behind an `axum` HTTP server bound to `127.0.0.1`. It owns:
- the single DB connection — making "exactly one writer" true and retiring the
  "concurrent instances unsupported" caveat from the original spec;
- the **auto-review poll loop** (today's `detect` polling of
  `zellij … dump-screen`), now emitting events instead of mutating UI state;
- the managed **`zellij web`** process (ensure-running + token handling);
- an in-memory **event broadcaster** that fans board deltas out to all clients.

### `kamaji` (the binary the user runs)
A thin launcher and TUI host:
- finds a live daemon (pidfile + health ping) or **auto-spawns** one detached,
  waits for health, then connects;
- renders the ratatui TUI as a **daemon client** (commands over HTTP, board
  state from the event stream);
- subcommands (today's `cli.rs`) become daemon API calls; one subcommand opens
  the browser.

**Load-bearing property:** the UI layer no longer touches the DB or zellij
directly. Both TUI and browser go through the daemon. That is what makes "one
source of truth, live on both surfaces" real.

## 4. Runtime topology (localhost)

```
  ┌─────────────┐     HTTP cmds + SSE events     ┌────────────────────────┐
  │ ratatui TUI │ ─────────────────────────────▶ │        kamajid         │
  │  (client)   │ ◀───────────────────────────── │  axum @127.0.0.1:PORT  │
  └─────────────┘                                 │                        │
  ┌─────────────┐     HTTP cmds + SSE events      │  kamaji-core (DB, git, │
  │   Browser   │ ─────────────────────────────▶ │  zellij orchestration) │
  │  (Datastar) │ ◀───────────────────────────── │  auto-review poll loop │
  └──────┬──────┘                                 │  manages `zellij web`  │
         │  attach: iframe/link                   └───────────┬────────────┘
         ▼                                                     │ spawns/ensures
  ┌──────────────────────────┐                                 ▼
  │ zellij web @:8082/<sess>  │ ◀───────────────────  zellij sessions (PTYs)
  └──────────────────────────┘
```

- **Auto-spawn:** the launcher checks for a live daemon; if absent, forks
  `kamajid` detached, waits for health, then connects. The daemon outlives the
  client that started it.
- **Attach is per-client, by design.** The daemon only *creates the session in
  the background* (today's `create_session_background`) and returns its name;
  the attach mechanism lives in the client:
  - **TUI** attaches natively — suspends and `exec zellij attach <session>`
    inline in the terminal (today's mechanism, unchanged).
  - **Browser** attaches via zellij web — links/iframes to
    `http://127.0.0.1:8082/<session-name>`; the daemon ensures `zellij web` is
    up and handles the token.

## 5. Data flow: commands down, events up

- **Commands** are HTTP requests handled by `kamaji-core`, e.g.
  `POST /tickets`, `POST /tickets/:id/move`, `POST /tickets/:id/start`,
  `POST /tickets/:id/done`, `DELETE /tickets/:id`, plus project CRUD and config.
- **Events** are a single **SSE stream** (`GET /events`) broadcasting board
  deltas: `ticket.created`, `ticket.moved`, `ticket.updated`, `ticket.deleted`,
  `session.started`, `session.idle` (from the poll loop → auto-review),
  `session.exited`. Every connected client — the TUI and every browser tab —
  re-renders from these deltas.

This is the "LiveView feel" without LiveView: the server owns state and pushes
deltas; clients are dumb renderers. The reactivity surface is intentionally
small (a handful of event types), which is what makes Approach A viable.

## 6. Browser UI

Server-rendered HTML (`maud` templates) made reactive with **Datastar**: a
~14kb library unifying client signals with SSE-driven DOM patching, which maps
directly onto "push board deltas to all clients." Moving a card is a POST →
the server re-renders the affected columns → the same change arrives at other
tabs via the `/events` SSE patch. (htmx + its SSE extension is the fallback if
Datastar proves awkward.)

The polished look comes from owning real HTML/CSS. The actual visual design work
(layout, theming, interactions) is deferred to Phase 3 and will invoke the
frontend-design skill at that point; this spec does not pin down visuals.

## 7. Remote-future seams (build now, activate later)

Localhost-first, but with cheap hooks so the remote pivot is configuration, not
a rewrite:
- **Bind address and auth are config, not assumptions.** The daemon binds
  `127.0.0.1` today; a future `bind`/`auth` config flips it. The HTTP API routes
  through a single auth middleware layer that is a no-op on localhost.
- **zellij web already does the hard part of remote sessions:** it supports
  `web_server_ip 0.0.0.0`, TLS via `web_server_cert`/`web_server_key`,
  `base_url` for reverse proxies, and login tokens. "Remote sessions" is mostly
  zellij config that kamaji passes through, not new kamaji code.
- **No shared mutable state outside the daemon.** Multi-client is already true
  locally, so multi-user becomes an auth/identity layer on top rather than a
  re-architecture.

## 8. Phased rollout

This is too large for one change. It ships as a sequence of independently
mergeable phases; each ends green on `main` with a working tool. Only **Phase 0**
is committed in full detail here — each later phase gets its own
brainstorm → spec → plan cycle so each spec stays focused enough for a single
implementation plan.

**Phase 0 — Extract `kamaji-core` (pure refactor, zero behavior change).**
Convert the repo to a Cargo workspace; move domain modules into a `kamaji-core`
library; the existing binary depends on it and calls it directly (no daemon
yet). TUI/CLI behavior is identical. This is the safety net: if nothing else
happened, kamaji is unchanged but cleanly layered.
*Verification:* existing test suite passes unchanged; manual smoke of
create/move/attach.

**Phase 1 — Stand up `kamajid` + the API; TUI still in-process.**
Build the axum daemon exposing the command API + `/events` SSE over
`kamaji-core`. Move the auto-review poll loop and `zellij web` management into
it. The TUI keeps working against core directly for now — the daemon runs
alongside but nothing depends on it yet, so the API can be built and tested in
isolation.
*Verification:* API integration tests (boot daemon on an ephemeral port, drive
commands, assert resulting state + SSE deltas); poll loop emits `session.idle`.

**Phase 2 — TUI becomes a daemon client; auto-spawn.**
Flip the TUI to talk to the daemon (commands over HTTP, board state from
`/events`) and add the auto-spawn/health-check launcher. Now there is exactly
one writer to the DB. The CLI subcommands become API calls too.
*Verification:* two TUIs against one daemon stay in sync; killing the daemon and
re-running auto-respawns; attach still execs native zellij.

**Phase 3 — Browser board (the headline).**
maud + Datastar board served by the daemon: render columns; drag/move;
create/edit/delete; live deltas via SSE; and the **attach** button that ensures
`zellij web` and opens `:8082/<session>`. Invokes the frontend-design skill for
the real visual work. TUI and browser are now peers on one backend.
*Verification:* move a card in the browser → TUI updates and vice-versa; attach
opens a live session; auto-review moves a card to Review live on both surfaces.

**Phase 4+ — Browser-first features (future, separate specs).**
The browser becomes where new features land (richer ticket views, session
previews, etc.); the TUI deliberately stays the leaner subset. Each is its own
cycle.

## 9. Testing strategy

- **`kamaji-core`:** keep and extend the current unit tests — they move with the
  code, and Phase 0 must leave them green. Domain logic stays the most-tested
  layer. zellij/git interactions are tested as today (layouts rendered to temp
  dirs with KDL assertions; no real zellij needed).
- **`kamajid`:** integration tests that boot the daemon on an ephemeral port,
  issue commands, and assert both resulting state and the SSE deltas emitted.
- **Browser:** thin by design (server owns logic). Smoke-level: assert rendered
  HTML contains the expected cards/attributes; defer heavier end-to-end until
  the UI stabilizes.
- **CI:** unchanged philosophy (fmt + clippy + test on PRs), now across the
  workspace.

TDD applies to core + daemon; the view layer is verified by rendering
assertions.

## 10. Risks & open items

- **`zellij web` lifecycle & token handoff** — the exact UX of ensuring the
  server is up and getting a browser session authenticated (token in URL vs.
  login page) needs a small spike against zellij 0.43+. Biggest unknown; gates
  Phase 3.
- **Auto-spawn races** — two clients starting simultaneously both forking the
  daemon; needs a pidfile lock with "lose the race, connect to the winner."
  Gates Phase 2.
- **iframe vs. new tab for attach** — whether zellij web sends headers
  (`X-Frame-Options`/CSP `frame-ancestors`) that block iframing; if so, attach
  opens a tab/window instead. Spike confirms; affects Phase 3.
- **maud + Datastar ergonomics** — less batteries-included than LiveView;
  accepted because the reactivity surface here is small.

## 11. What carries over unchanged from the original spec

The data model (§3 of `2026-05-23-kamaji-design.md`), worktree/session
lifecycle (§6), agent command templates (§4–5), and zellij KDL layout generation
(§8) are unchanged. They simply move into `kamaji-core` and are exercised through
the daemon instead of directly from the TUI.
