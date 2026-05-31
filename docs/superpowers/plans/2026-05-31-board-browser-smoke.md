# Board Browser Smoke Test Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a headless-browser smoke test that boots `kamajid` against a throwaway DB, seeds tickets across statuses, and asserts the board's real in-browser behavior — so a future regression of the #90 Datastar-wiring fixes is caught.

**Architecture:** A Playwright (`@playwright/test`) harness committed under `crates/kamajid/smoke/`. A small `harness.mjs` module boots the prebuilt `kamajid` binary with an isolated `XDG_*` env on a free port, seeds via the HTTP API, and tears down. `board.smoke.mjs` drives Chromium through delete / move / create / cancel / escape / validation flows. A new non-required `Browser smoke` CI job builds the binary and runs the spec on PRs.

**Tech Stack:** Node (ESM), `@playwright/test` + Chromium, the existing `kamajid` axum daemon and its JSON API.

---

## File Structure

- Create: `crates/kamajid/smoke/package.json` — Node project, pins `@playwright/test`, `test` script.
- Create: `crates/kamajid/smoke/package-lock.json` — committed lockfile (CI uses `npm ci`).
- Create: `crates/kamajid/smoke/playwright.config.mjs` — single chromium project, serial, list reporter.
- Create: `crates/kamajid/smoke/.gitignore` — `node_modules/`, `test-results/`, `playwright-report/`.
- Create: `crates/kamajid/smoke/harness.mjs` — boot/seed/teardown helpers (one responsibility: get a running, seeded daemon).
- Create: `crates/kamajid/smoke/board.smoke.mjs` — the spec: drives the browser and asserts behavior.
- Create: `crates/kamajid/smoke/README.md` — how to run locally + the regression-proof recipe.
- Modify: `.github/workflows/ci.yml` — add the `smoke` job.

**Key facts the harness relies on (verified against the code):**
- `kamajid serve --bind 127.0.0.1:<port>` binds the daemon; `GET /healthz` returns 200 when ready.
- Data dir is `$XDG_DATA_HOME/kamaji/kamaji.db`; config under `$XDG_CONFIG_HOME`. Isolating both (plus `XDG_RUNTIME_DIR`) makes the run throwaway.
- Seed API: `POST /projects` `{name, root_dir}` → `{id,...}`; `POST /tickets` `{project_id, title, agent:"claude"}` → `{id,...}` (lands in Todo); `POST /tickets/:id/move` `{target:"in_progress"|"review"|"done"}`.
- Board selectors: columns `#col-todo` `#col-in_progress` `#col-review` `#col-done`; cards `#card-<id>`; new-ticket button `button.new-ticket`; modal mount `#modal` containing `<dialog id="ticket-dialog">`; title input `#f-title`; Save is `button[type=submit]`, Cancel is the `button` labelled "Cancel".
- The board page **server-renders** existing cards, so "a card is present" does NOT prove SSE opened. Live updates (delete removal, move relocation, create-appears, and an explicit out-of-band create) are what prove `data-init`/`/ui/events` is wired.
- Delete and Done buttons call `window.confirm`; the spec must auto-accept dialogs.
- The title input is `required`, so a truly empty submit is blocked client-side. To hit the server's `.trim()` 400, submit a **whitespace-only** title.

---

### Task 1: Scaffold the smoke project

**Files:**
- Create: `crates/kamajid/smoke/package.json`
- Create: `crates/kamajid/smoke/playwright.config.mjs`
- Create: `crates/kamajid/smoke/.gitignore`
- Create: `crates/kamajid/smoke/package-lock.json` (generated)

- [ ] **Step 1: Write `package.json`**

```json
{
  "name": "kamajid-board-smoke",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "description": "Headless-browser smoke test for the kamajid board (issue #91).",
  "scripts": {
    "test": "playwright test"
  },
  "devDependencies": {
    "@playwright/test": "^1.49.0"
  }
}
```

- [ ] **Step 2: Write `playwright.config.mjs`**

```js
import { defineConfig, devices } from '@playwright/test';

// One serial chromium project. The smoke mutates server state as it goes, so it
// must not run in parallel. List reporter keeps CI output readable.
export default defineConfig({
  testDir: '.',
  testMatch: '**/*.smoke.mjs',
  timeout: 30_000,
  expect: { timeout: 7_500 },
  fullyParallel: false,
  workers: 1,
  reporter: 'list',
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
});
```

- [ ] **Step 3: Write `.gitignore`**

```gitignore
node_modules/
test-results/
playwright-report/
```

- [ ] **Step 4: Install deps (generates the lockfile) and the browser**

Run (from `crates/kamajid/smoke/`):
```bash
npm install
npx playwright install chromium
```
Expected: `npm install` creates `node_modules/` and `package-lock.json`; the browser download succeeds.

- [ ] **Step 5: Verify Playwright is wired**

Run: `npx playwright test --list`
Expected: exits 0, reports `Total: 0 tests in 0 files` (no spec yet) — proves the toolchain runs.

- [ ] **Step 6: Commit**

```bash
git add crates/kamajid/smoke/package.json crates/kamajid/smoke/package-lock.json \
        crates/kamajid/smoke/playwright.config.mjs crates/kamajid/smoke/.gitignore
git commit -m "test(smoke): scaffold Playwright project for the board smoke (#91)"
```

---

### Task 2: Boot + seed harness, and a smoke that loads the board

**Files:**
- Create: `crates/kamajid/smoke/harness.mjs`
- Create: `crates/kamajid/smoke/board.smoke.mjs`

- [ ] **Step 1: Write `harness.mjs`**

```js
// Boot/seed/teardown for the board smoke. `startDaemon()` spawns the prebuilt
// kamajid binary with an isolated XDG_* env on a free port and waits until
// /healthz is green; `seed()` creates one ticket in every column over the HTTP
// API. The smoke spec owns the browser — this module only owns the server.
import { spawn } from 'node:child_process';
import net from 'node:net';
import os from 'node:os';
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));

// Default to the workspace debug build; CI overrides via KAMAJID_BIN.
function binPath() {
  return process.env.KAMAJID_BIN || path.resolve(HERE, '../../../target/debug/kamajid');
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Ask the OS for a free TCP port, then release it for the daemon to claim.
function freePort() {
  return new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.unref();
    srv.on('error', reject);
    srv.listen(0, '127.0.0.1', () => {
      const { port } = srv.address();
      srv.close(() => resolve(port));
    });
  });
}

export async function startDaemon() {
  const bin = binPath();
  try {
    await fs.access(bin);
  } catch {
    throw new Error(`kamajid binary not found at ${bin}. Run \`cargo build -p kamajid\` (or set KAMAJID_BIN).`);
  }

  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'kamaji-smoke-'));
  const port = await freePort();
  const base = `http://127.0.0.1:${port}`;

  const child = spawn(bin, ['serve', '--bind', `127.0.0.1:${port}`], {
    env: {
      ...process.env,
      XDG_DATA_HOME: dir,
      XDG_CONFIG_HOME: dir,
      XDG_RUNTIME_DIR: dir,
      KAMAJID_LOG: 'warn',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const logs = [];
  child.stdout.on('data', (b) => logs.push(b.toString()));
  child.stderr.on('data', (b) => logs.push(b.toString()));

  const stop = async () => {
    child.kill('SIGTERM');
    await fs.rm(dir, { recursive: true, force: true });
  };

  // Wait for readiness, surfacing the daemon's own logs on failure.
  for (let i = 0; i < 100; i++) {
    if (child.exitCode !== null) {
      await stop();
      throw new Error(`kamajid exited early (code ${child.exitCode}):\n${logs.join('')}`);
    }
    try {
      const r = await fetch(`${base}/healthz`);
      if (r.ok) return { base, dir, stop, logs: () => logs.join('') };
    } catch {
      // not up yet
    }
    await sleep(100);
  }
  await stop();
  throw new Error(`kamajid not ready after 10s:\n${logs.join('')}`);
}

async function postJson(url, body) {
  const r = await fetch(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!r.ok) throw new Error(`POST ${url} -> ${r.status}: ${await r.text()}`);
  return r.json();
}

// One project + one ticket in each of the four columns. Returns the project id
// and a map of column -> ticket id.
export async function seed(base, rootDir) {
  const project = await postJson(`${base}/projects`, { name: 'smoke', root_dir: rootDir });
  const ids = {};
  for (const col of ['todo', 'in_progress', 'review', 'done']) {
    const t = await postJson(`${base}/tickets`, {
      project_id: project.id,
      title: `seed ${col}`,
      agent: 'claude',
    });
    ids[col] = t.id;
  }
  for (const target of ['in_progress', 'review', 'done']) {
    await postJson(`${base}/tickets/${ids[target]}/move`, { target });
  }
  return { projectId: project.id, ids };
}
```

- [ ] **Step 2: Write `board.smoke.mjs` — boot, seed, assert initial render**

```js
// End-to-end smoke for the browser board. Boots a throwaway kamajid, seeds a
// card in every column, then drives Chromium through the interactive flows.
// Each flow is a test.step so failures point at the exact interaction.
import { test, expect } from '@playwright/test';
import { startDaemon, seed } from './harness.mjs';

let daemon; // { base, dir, stop }
let seeded; // { projectId, ids }
const consoleErrors = [];
const pageErrors = [];

test.beforeAll(async () => {
  daemon = await startDaemon();
  seeded = await seed(daemon.base, daemon.dir);
});

test.afterAll(async () => {
  if (daemon) await daemon.stop();
});

test('board is interactive end-to-end', async ({ page }) => {
  page.on('console', (m) => {
    if (m.type() === 'error') consoleErrors.push(m.text());
  });
  page.on('pageerror', (e) => pageErrors.push(e.message));

  await test.step('board loads with a card in every column', async () => {
    await page.goto(daemon.base);
    for (const col of ['todo', 'in_progress', 'review', 'done']) {
      await expect(page.locator(`#col-${col} #card-${seeded.ids[col]}`)).toBeVisible();
    }
  });
});
```

- [ ] **Step 3: Build the binary and run the smoke**

Run (from `crates/kamajid/smoke/`):
```bash
cargo build -p kamajid && npm test
```
Expected: PASS — `board is interactive end-to-end` passes its one step; the daemon is torn down cleanly.

- [ ] **Step 4: Commit**

```bash
git add crates/kamajid/smoke/harness.mjs crates/kamajid/smoke/board.smoke.mjs
git commit -m "test(smoke): boot+seed harness and initial board-render assertion (#91)"
```

---

### Task 3: SSE-open + delete + move live flows

These three flows all depend on the `/ui/events` SSE stream being open (the #90 break). The explicit SSE-open step issues an **out-of-band** create over the API (no button) and asserts the card appears live — proof the EventSource is connected, independent of the server-rendered initial HTML.

**Files:**
- Modify: `crates/kamajid/smoke/board.smoke.mjs` (add steps inside the existing test)

- [ ] **Step 1: Add the SSE-open, delete, and move steps**

Insert these steps after the "board loads" step, inside the same `test(...)` body:

```js
  await test.step('SSE is live: an out-of-band create appears without reload', async () => {
    // Create a ticket directly via the API; if /ui/events is open it patches
    // #col-todo live and the card shows up with no navigation.
    const r = await fetch(`${daemon.base}/tickets`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ project_id: seeded.projectId, title: 'live via SSE', agent: 'claude' }),
    });
    expect(r.ok).toBeTruthy();
    await expect(page.locator('#col-todo').getByText('live via SSE')).toBeVisible();
  });

  await test.step('Delete removes a card live', async () => {
    page.once('dialog', (d) => d.accept()); // window.confirm in the Delete handler
    const card = page.locator(`#card-${seeded.ids.todo}`);
    await card.getByRole('button', { name: 'Delete' }).click();
    await expect(card).toHaveCount(0);
  });

  await test.step('Move relocates a card across columns live', async () => {
    const card = page.locator(`#card-${seeded.ids.in_progress}`);
    await card.getByRole('button', { name: 'Move' }).click();
    await expect(page.locator(`#col-review #card-${seeded.ids.in_progress}`)).toBeVisible();
  });
```

- [ ] **Step 2: Run the smoke**

Run: `npm test`
Expected: PASS — all four steps pass (board loads, SSE-open, delete, move).

- [ ] **Step 3: Commit**

```bash
git add crates/kamajid/smoke/board.smoke.mjs
git commit -m "test(smoke): assert SSE-open, live delete, and live move (#91)"
```

---

### Task 4: Modal create / cancel / escape / validation + no-errors

**Files:**
- Modify: `crates/kamajid/smoke/board.smoke.mjs` (add steps inside the existing test)

- [ ] **Step 1: Add the modal and validation steps, then the final no-errors assertion**

Append these steps after the "Move" step, inside the same `test(...)` body:

```js
  await test.step('+ Ticket opens the modal; Save creates the card and closes it', async () => {
    await page.locator('button.new-ticket').click();
    await expect(page.locator('#modal #ticket-dialog')).toBeVisible();
    await page.locator('#f-title').fill('created via modal');
    await page.locator('#ticket-dialog button[type="submit"]').click();
    // Modal closes (mount emptied) and the new card arrives in todo over SSE.
    await expect(page.locator('#modal #ticket-dialog')).toHaveCount(0);
    await expect(page.locator('#col-todo').getByText('created via modal')).toBeVisible();
  });

  await test.step('Cancel closes the modal', async () => {
    await page.locator('button.new-ticket').click();
    await expect(page.locator('#modal #ticket-dialog')).toBeVisible();
    await page.locator('#ticket-dialog').getByRole('button', { name: 'Cancel' }).click();
    await expect(page.locator('#modal #ticket-dialog')).toHaveCount(0);
  });

  await test.step('Escape closes the modal', async () => {
    await page.locator('button.new-ticket').click();
    await expect(page.locator('#modal #ticket-dialog')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.locator('#modal #ticket-dialog')).toHaveCount(0);
  });

  await test.step('A whitespace-only title (server 400) keeps the modal open', async () => {
    await page.locator('button.new-ticket').click();
    await expect(page.locator('#modal #ticket-dialog')).toBeVisible();
    // `required` is satisfied by spaces, but the server trims and returns 400,
    // so the success-close never runs and the dialog stays.
    await page.locator('#f-title').fill('   ');
    await page.locator('#ticket-dialog button[type="submit"]').click();
    await expect(page.locator('#modal #ticket-dialog')).toBeVisible();
    await expect(page.locator('#col-todo').getByText('created via modal')).toHaveCount(1); // unchanged
  });

  await test.step('no console errors or page errors occurred', async () => {
    expect(consoleErrors, `console errors:\n${consoleErrors.join('\n')}`).toEqual([]);
    expect(pageErrors, `page errors:\n${pageErrors.join('\n')}`).toEqual([]);
  });
```

- [ ] **Step 2: Run the smoke**

Run: `npm test`
Expected: PASS — every step passes, including the no-errors assertion.

Note: if a *benign* console error from a third-party source appears, tighten the `console` filter in `board.smoke.mjs` to ignore that specific known-benign message (document why inline). Do NOT broaden it to swallow real errors.

- [ ] **Step 3: Commit**

```bash
git add crates/kamajid/smoke/board.smoke.mjs
git commit -m "test(smoke): modal create/cancel/escape, 400 validation, no-errors (#91)"
```

---

### Task 5: Regression proof — verify the smoke actually fails

This satisfies the acceptance criterion: "Removing any one of the #90 fixes makes the smoke fail." It is a manual verification — no code is committed.

**Files:**
- Temporarily edit (then revert): `crates/kamajid/src/views/page.rs`

- [ ] **Step 1: Revert the `data-init` SSE hook to the inert form**

In `crates/kamajid/src/views/page.rs`, change the body attribute from `data-init` to the (ignored) hyphen form:

```rust
// from:
body data-init="@get('/ui/events')" {
// to (temporarily, to prove the smoke catches it):
body data-on-load="@get('/ui/events')" {
```

- [ ] **Step 2: Rebuild and run the smoke**

Run (from `crates/kamajid/smoke/`): `cargo build -p kamajid && npm test`
Expected: **FAIL** — the "SSE is live" step times out waiting for `live via SSE` (the EventSource never opens), proving the smoke catches a reverted #90 fix.

- [ ] **Step 3: Restore the fix**

Run: `git checkout crates/kamajid/src/views/page.rs`
Then rebuild and confirm green: `cargo build -p kamajid && npm test` → PASS.

- [ ] **Step 4: No commit**

This task changes no committed files (the edit was reverted). Record the observed FAIL→PASS in the PR description.

---

### Task 6: Wire the non-required CI job

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Append the `smoke` job**

Add this job under `jobs:` in `.github/workflows/ci.yml` (sibling of `test`, `windows`, `shellcheck`):

```yaml
  smoke:
    name: Browser smoke
    runs-on: ubuntu-latest
    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Cache Rust build
        uses: Swatinem/rust-cache@v2

      - name: Build kamajid
        run: cargo build -p kamajid

      - name: Setup Node
        uses: actions/setup-node@v4
        with:
          node-version: 20

      - name: Install smoke deps
        working-directory: crates/kamajid/smoke
        run: npm ci

      - name: Install Chromium
        working-directory: crates/kamajid/smoke
        run: npx playwright install --with-deps chromium

      - name: Run board smoke
        working-directory: crates/kamajid/smoke
        env:
          KAMAJID_BIN: ${{ github.workspace }}/target/debug/kamajid
        run: npm test
```

- [ ] **Step 2: Validate the workflow YAML parses**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('ok')"`
Expected: prints `ok` (no YAML syntax error).

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add non-required Browser smoke job (#91)"
```

---

### Task 7: Document how to run it

**Files:**
- Create: `crates/kamajid/smoke/README.md`

- [ ] **Step 1: Write `README.md`**

````markdown
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
````

- [ ] **Step 2: Commit**

```bash
git add crates/kamajid/smoke/README.md
git commit -m "docs(smoke): how to run the board smoke and prove regressions (#91)"
```

---

## Self-Review

**Spec coverage:**
- Playwright harness under `crates/kamajid/smoke/`, seed across statuses, isolation via `XDG_*` → Tasks 1–2. ✓
- Flows: SSE-open → Task 3; delete + move → Task 3; +Ticket/Save, Cancel, Escape → Task 4; whitespace-400 keeps modal open → Task 4; zero console/page errors → Task 4. ✓
- Non-required CI job → Task 6. ✓
- Documented how to run locally → Task 7. ✓
- Regression proof (reverting a #90 fix fails the smoke) → Task 5 (verified) + documented in Task 7. ✓

**Placeholder scan:** No TBD/TODO; every code/command step shows full content. ✓

**Type/selector consistency:** `harness.mjs` exports `startDaemon()` → `{ base, dir, stop, logs }` and `seed(base, rootDir)` → `{ projectId, ids }`; the spec consumes exactly those. Selectors (`#col-*`, `#card-<id>`, `button.new-ticket`, `#modal #ticket-dialog`, `#f-title`, `button[type="submit"]`, "Cancel"/"Delete"/"Move" roles) match the views in `page.rs`/`board.rs`/`card.rs`/`modal.rs`. ✓
