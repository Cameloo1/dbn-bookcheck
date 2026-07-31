import { defineConfig, devices } from "@playwright/test";

const configuredBaseUrl = process.env.PORTFOLIO_BASE_URL?.replace(/\/+$/u, "");
const port = Number(process.env.PORTFOLIO_E2E_PORT ?? 4173);
const baseURL = configuredBaseUrl ?? `http://localhost:${port}`;

export default defineConfig({
  testDir: ".",
  testMatch: "**/*.e2e.spec.ts",
  outputDir: "../outputs/playwright/test-results",
  fullyParallel: false,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 2 : 0,
  workers: 2,
  reporter: [
    ["line"],
    [
      "html",
      {
        open: "never",
        outputFolder: "../outputs/playwright/html-report",
      },
    ],
  ],
  use: {
    baseURL,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  webServer: configuredBaseUrl
    ? undefined
    : {
        command: `npm run dev -- --host localhost --port ${port}`,
        url: baseURL,
        reuseExistingServer: !process.env.CI,
        timeout: 120_000,
      },
  projects: [
    {
      name: "chromium-desktop",
      use: { ...devices["Desktop Chrome"] },
    },
    {
      name: "firefox-desktop",
      use: { ...devices["Desktop Firefox"] },
    },
    {
      name: "webkit-desktop",
      use: { ...devices["Desktop Safari"] },
    },
    {
      name: "chromium-mobile",
      use: { ...devices["Pixel 7"] },
    },
  ],
});
