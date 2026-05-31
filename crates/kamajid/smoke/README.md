# Board browser smoke test

A headless-browser ([Playwright](https://playwright.dev) + Chromium) smoke test
for the kamajid board. It boots a throwaway `kamajid`, seeds a ticket in every
column over the HTTP API, and drives the real browser through the interactive
flows the server-side tests can't see: live SSE updates, delete, move, the
create/edit modal, and validation. It exists because PR #90 fixed a class of
bugs where the board was completely inert in-browser while all 330+ server-side
tests still passed — no test loaded a browser.

## Run it locally

From this directory:

```sh
cargo build -p kamajid                       # produces target/debug/kamajid
npm ci                                        # or `npm install` the first time
npx playwright install chromium               # one-time browser download
npm test
```

The harness finds the binary at `../../../target/debug/kamajid`. Point it
elsewhere (e.g. a release build) with `KAMAJID_BIN`:

```sh
KAMAJID_BIN=/abs/path/to/kamajid npm test
```

Each run uses a fresh temp dir for `XDG_DATA_HOME`/`XDG_CONFIG_HOME`/
`XDG_RUNTIME_DIR`, so it never touches your real kamaji data, and binds an
ephemeral localhost port.

## What it asserts

- `/ui/events` SSE is live on load (an out-of-band API create appears with no reload)
- Delete removes a card live; Move relocates a card across columns live
- "+ Ticket" opens the modal; Save creates the ticket, closes the modal, and the
  new card appears; Cancel and Escape close the modal
- A whitespace-only title (server returns 400) keeps the modal open
- Zero console / page errors during the run

## Proving it catches regressions

The point is to fail when a #90 fix is reverted. To confirm, temporarily change
the SSE hook in `crates/kamajid/src/views/page.rs` from the RC.6 colon/`data-init`
form back to the inert hyphen form:

```rust
body data-on-load="@get('/ui/events')" {   // was: data-init="@get('/ui/events')"
```

Then `cargo build -p kamajid && npm test` — the "SSE is live" step fails because
the EventSource never opens. Revert the change to go green again. The same holds
for reverting a card binding from `data-on:click` to `data-on-click`.

## CI

The `Browser smoke` job in `.github/workflows/ci.yml` runs this on every PR. It
is a separate, non-required job: a red smoke is visible on the PR but does not
block merges of unrelated work.
