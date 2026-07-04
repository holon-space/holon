import { defineConfig, devices } from "@playwright/test";

// Boots the dioxus-web dev server (serve.mjs) and runs the worker-smoke test
// against it. Assumes the Trunk bundle (dist/) and the holon-worker wasm have
// ALREADY been built — the CI job (.github/workflows/devex-gates.yml) builds both
// before invoking Playwright, because serve.mjs only serves, it does not build the
// worker wasm. reuseExistingServer lets a local dev run reuse an already-running
// `node serve.mjs`.
export default defineConfig({
  testDir: "./tests",
  timeout: 180_000,
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:8765",
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: "node serve.mjs",
    url: "http://127.0.0.1:8765",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
    stdout: "pipe",
    stderr: "pipe",
  },
});
