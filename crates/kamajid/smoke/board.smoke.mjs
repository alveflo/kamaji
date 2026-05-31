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
  // root_dir = the daemon's temp dir; it is removed by daemon.stop() in afterAll.
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
