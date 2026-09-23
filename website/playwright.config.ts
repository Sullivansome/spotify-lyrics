import { defineConfig, devices } from '@playwright/test';
export default defineConfig({
  testDir: './tests', fullyParallel: true, forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0, workers: 2, reporter: 'list',
  use: { baseURL: 'http://127.0.0.1:4328', trace: 'retain-on-failure' },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: { command: 'npm run preview -- --port 4328 --ignore-lock', url: 'http://127.0.0.1:4328', reuseExistingServer: false },
});
