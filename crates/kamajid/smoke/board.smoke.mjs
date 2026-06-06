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
    if (m.type() !== 'error') return;
    // Ignore the browser's generic network-level "Failed to load resource: 400"
    // message that Chromium emits when the whitespace-title step intentionally
    // receives a 400 from POST /tickets.  The fetch() itself is handled in JS
    // (modal stays open), so this is not a real application error.
    if (m.text().startsWith('Failed to load resource: the server responded with a status of 400')) return;
    consoleErrors.push(m.text());
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
    await card.hover(); // actions reveal on hover (dense card)
    await card.getByRole('button', { name: 'Delete' }).click();
    await expect(card).toHaveCount(0);
  });

  await test.step('Drag-and-drop relocates a card across columns live', async () => {
    // The Move button is gone — column moves happen by dragging the card onto
    // another column's body. Native HTML5 DnD isn't driven by Playwright's mouse
    // events, so dispatch the drag sequence with a shared DataTransfer. The drop
    // fires POST /tickets/:id/move and the authoritative move arrives over SSE.
    const id = seeded.ids.in_progress;
    await page.evaluate((cardId) => {
      const card = document.getElementById(`card-${cardId}`);
      const body = document.querySelector('#col-review .col-body');
      const dt = new DataTransfer();
      const fire = (el, type) =>
        el.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: dt }));
      fire(card, 'dragstart');
      fire(body, 'dragover');
      fire(body, 'drop');
      fire(card, 'dragend');
    }, id);
    await expect(page.locator(`#col-review #card-${id}`)).toBeVisible();
  });

  await test.step('+ Ticket opens the modal as a centered overlay over a dimmed board', async () => {
    await page.locator('button.new-ticket').click();
    const dialog = page.locator('#modal #ticket-dialog');
    await expect(dialog).toBeVisible();
    // Regression for #95: the dialog is morphed in (not showModal()), so it must
    // be pinned as a fixed, centered overlay — not left in normal flow below the
    // board where it computed position:absolute, top:1241px (off-screen).
    const layout = await dialog.evaluate((el) => {
      const cs = getComputedStyle(el);
      const r = el.getBoundingClientRect();
      const back = getComputedStyle(document.getElementById('modal'), '::before');
      return {
        position: cs.position,
        top: r.top,
        bottom: r.bottom,
        viewportH: window.innerHeight,
        backdrop: back.content, // '""' once the ::before backdrop applies, 'none' otherwise
      };
    });
    expect(layout.position).toBe('fixed');
    expect(layout.top).toBeGreaterThanOrEqual(0);
    expect(layout.bottom).toBeLessThanOrEqual(layout.viewportH);
    expect(layout.backdrop).not.toBe('none'); // dimmed/blurred board behind the form
  });

  await test.step('+ Ticket: Save creates the card and closes the modal', async () => {
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
});
