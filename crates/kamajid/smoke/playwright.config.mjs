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
