import { defineConfig, devices } from '@playwright/test';

// Every worker starts its own server and database (fixtures/server.ts), so workers are
// capped: logins hash passwords, and the server only hashes a few at a time.
export default defineConfig({
  globalSetup: './global-setup.ts',
  outputDir: './test-results/artifacts',
  timeout: 60_000,
  expect: { timeout: 10_000 },
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: process.env.CI ? 2 : 4,
  reporter: process.env.CI
    ? [['github'], ['html', { open: 'never' }]]
    : [['list'], ['html', { open: 'never' }]],
  use: {
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
  },
  projects: [
    {
      name: 'web',
      testDir: './tests/web',
      fullyParallel: true,
      use: { ...devices['Desktop Chrome'] },
    },
    {
      // Everything on the emulator: the app alone (tests/android) and with the web client
      // (tests/cross). One emulator, so one worker.
      name: 'android',
      testDir: './tests',
      testMatch: ['android/**/*.spec.ts', 'cross/**/*.spec.ts'],
      workers: 1,
      timeout: 180_000,
      use: { ...devices['Desktop Chrome'] },
    },
  ],
});
