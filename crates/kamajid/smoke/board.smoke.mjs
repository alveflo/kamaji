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
});
