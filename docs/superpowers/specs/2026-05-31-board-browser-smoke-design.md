# Design — headless-browser smoke test for the board (issue #91)

## Problem

PR #90 fixed a class of bugs where the browser board was completely inert —
"nothing happens when I press any buttons" — even though all 330+ server-side
tests passed. The Phase 3 frontend was written against a different Datastar
attribute syntax than the vendored v1.0.0-RC.6 bundle implements: the hyphen
form `data-on-click` is silently ignored (RC.6 needs the colon form
`data-on:click`), and there is no `on-load` event (RC.6 uses `data-init`).

Nothing caught it because **no test loads a browser**. The view tests assert the
rendered HTML *strings*, so they happily asserted the wrong-but-consistent
syntax. PR #90 added Rust-level guards that pin the exact strings (colon syntax,
`data-init`, the `#modal` root, JSON command bodies), but those can't catch a
future Datastar upgrade that changes semantics, or a new integration break. We
need a real end-to-end check that actually drives a browser against the running
daemon.

## Goal

A headless-browser smoke test that boots `kamajid` against a throwaway database,
seeds tickets across statuses, and asserts the real in-browser behavior of the
board. It must fail if any one of the #90 fixes is reverted (e.g. a binding put
back to the hyphen form).

## Decision

The board boots in-process via `kamajid::router` and serves on an ephemeral
port, so any HTTP-driving harness works. Of the three options in the issue
(Playwright in CI / local-only dev tool / Rust `fantoccini`), we chose:

> **Playwright (Node) harness, committed under `crates/kamajid/smoke/`, wired
> into CI as a separate, non-required `Browser smoke` job.**

This gives the most coverage. A red smoke is visible on the PR but — because no
checks are required for merge in this repo today — it does not block unrelated
merges. It adds a JS + Chromium toolchain to CI, scoped to its own job.

Two implementation calls made during design and approved:

- Use `@playwright/test` (the test runner + assertion library) rather than a
  hand-rolled `node` script — built-in waiting, dialog handling, and reporting.
- Seed purely through the HTTP API (`POST /projects`, `POST /tickets`,
  `POST /tickets/:id/move`) rather than poking a DB fixture — no coupling to the
  on-disk schema, exercises the same paths the app uses.

## Architecture

### Layout

```
crates/kamajid/smoke/
  package.json            # pins @playwright/test
  playwright.config.mjs   # single project, chromium, sensible timeouts
  board.smoke.mjs         # the spec: boot → seed → drive → assert → teardown
  README.md               # how to run locally; the regression-proof recipe
  .gitignore              # node_modules/, test-results/
```

### Booting the daemon

The spec spawns the **prebuilt** `kamajid` binary as a child process (it does
not build it — CI and the local README run `cargo build -p kamajid` first):

- **Binary path** from the `KAMAJID_BIN` env var, defaulting to the workspace
  `target/debug/kamajid` (resolved relative to the smoke dir).
- **Isolation** via a fresh temp directory per run, exported as `XDG_DATA_HOME`,
  `XDG_CONFIG_HOME`, and `XDG_RUNTIME_DIR`. `config::load_or_init` writes default
  config there and the SQLite DB lands at `$XDG_DATA_HOME/kamaji/kamaji.db`, so
  the run never touches the developer's real data.
- **Port**: the harness picks a free TCP port (bind a throwaway listener on
  `127.0.0.1:0`, read the assigned port, close it) and passes
  `serve --bind 127.0.0.1:<port>`.
- **Readiness**: poll `GET /healthz` until it returns 200 (bounded retry, then
  fail with the child's captured stderr).

Teardown kills the child process and removes the temp directory, in an
`afterAll`/`finally` so a failed assertion still cleans up.

### Seeding

Over the HTTP API, before any browser work:

1. `POST /projects` with `{ name, root_dir }` (`root_dir` = the temp dir; the
   board never touches it for rendering). Capture the project id.
2. `POST /tickets` four times (all land in Todo), capturing ids.
3. `POST /tickets/:id/move` to spread one card each into **in_progress**,
   **review**, and **done**, leaving one in **todo**.

This yields a board with a card in every column — enough to exercise delete
(todo/done), move (in_progress → review), and create (todo).

### Driving & asserting

A single spec runs the flows in sequence against `http://127.0.0.1:<port>/`:

| Flow | Action | Assertion |
|------|--------|-----------|
| **SSE opens on load** | navigate to `/` | the live `/ui/events` stream patches the board — assert a seeded card is present after the snapshot (proves `data-init` fired and the EventSource opened) |
| **Delete live** | click Delete on the todo card; auto-accept the `confirm()` dialog | `#card-<id>` is removed from the DOM (SSE remove patch) |
| **Move live** | click Move on the in_progress card | the card now lives inside `#col-review` |
| **+ Ticket → Save** | click "+ Ticket"; fill Title; Save | `#ticket-dialog` appears in `#modal`, then closes, and the new card appears in `#col-todo` |
| **Cancel closes** | open modal; click Cancel | `#modal` is emptied (no `<dialog>`) |
| **Escape closes** | open modal; press `Escape` | `#modal` is emptied |
| **Validation 400** | open modal; submit a **whitespace-only** title | modal stays open (the new card never appears) |
| **No errors** | across the whole run | zero `console` error events and zero `pageerror` events |

**Validation nuance:** the title `<input>` carries `required`, so a truly empty
submit is blocked by native browser validation before the server is reached. To
exercise the real server-side 400 (`title must not be empty` after `.trim()`),
the smoke submits a **whitespace-only** title — it satisfies `required` but the
server rejects it, `r.ok` is false, so the success-close JS does not run and the
modal stays open. This is the genuine end-to-end validation path.

**Dialog handling:** Delete (and Done, unused here) go through `window.confirm`.
The spec registers a `page.on('dialog', d => d.accept())` handler so the
confirm resolves truthy and the `fetch(...DELETE)` fires.

**Error capture:** the spec attaches `page.on('console')` (filtering `error`)
and `page.on('pageerror')` listeners at navigation and asserts both stayed empty
at the end. A silently-ignored binding (the #90 bug) produces no console error,
so the behavioral assertions above — not the error capture — are what catch a
regression; the error capture is an extra net for *new* breakage.

### CI job

A new `smoke` job in `.github/workflows/ci.yml`, alongside `test` / `windows` /
`shellcheck`:

```yaml
smoke:
  name: Browser smoke
  runs-on: ubuntu-latest
  steps:
    - checkout
    - install Rust (dtolnay/rust-toolchain@stable)
    - cargo build -p kamajid            # produces target/debug/kamajid
    - actions/setup-node                # Node LTS
    - working-directory: crates/kamajid/smoke
      run: npm ci
    - working-directory: crates/kamajid/smoke
      run: npx playwright install --with-deps chromium
    - working-directory: crates/kamajid/smoke
      run: npm test                     # KAMAJID_BIN -> the built binary
```

It is just another job. Branch protection (not the workflow file) decides what
is "required"; this repo requires nothing for merge today, so the job is
visible-but-non-blocking — exactly the recommended posture.

## Testing & verification

- **Positive:** the smoke passes against a fresh `cargo build -p kamajid`.
- **Regression proof (acceptance criterion):** documented in the README — revert
  any one #90 fix (e.g. change a `data-on:click` back to `data-on-click`, or
  `data-init` back to `data-on-load`) and re-run; the SSE-open / interaction
  assertions fail. This is verified once by hand during implementation.

## Out of scope

- Running the smoke inside `cargo test` (option 3 / `fantoccini`).
- Making the job a required check / wiring branch protection.
- Cross-browser coverage — Chromium only.
- Testing session-spawning flows (Start/Attach/Done-with-cleanup) — those need
  zellij + git and are not part of the inert-board regression surface.

## Acceptance criteria (from the issue)

- A smoke test exercising the flows above exists and passes against a fresh
  build.
- It is documented (how to run locally) and runs on PRs via CI.
- Removing any one of the #90 fixes makes the smoke fail.
