import { test, expect } from "@playwright/test";

// "Zero worker ERRORs" smoke test.
//
// The dioxus-web frontend runs all Holon crates (ReactiveEngine, parsers,
// storage) inside a dedicated Web Worker (holon-worker, wasi-threads), with the
// Dioxus UI on the main thread talking to it over JSON RPC. A silent worker crash
// has shipped undetected before (worker op-registry was EMPTY because it wired only
// EventInfraModule => every content write was lost with no visible error). This
// gate turns that class of regression red: if the worker (or the page) emits any
// ERROR-level console line during boot, the test fails and prints them.
//
// Chromium delivers Web Worker `console.*` output through the page `console` event,
// so a single page.on("console") listener captures both main-thread and worker
// errors. Uncaught exceptions come through `pageerror`.
test("dioxus-web boots with zero ERROR-level console lines", async ({ page }) => {
  const errors = [];
  page.on("console", (msg) => {
    if (msg.type() === "error") errors.push(`[console.error] ${msg.text()}`);
  });
  page.on("pageerror", (err) => errors.push(`[pageerror] ${err.message}`));

  await page.goto("/");

  // Give the worker time to fetch + instantiate its wasm and push the first
  // snapshot to the UI. The dev-profile wasm is large; boot is not instant.
  await page.waitForTimeout(20_000);

  // Sanity: the Dioxus app actually mounted something (not a blank/404 page).
  const bodyText = (await page.locator("body").innerText()).trim();
  expect(bodyText.length, "page body rendered no content").toBeGreaterThan(0);

  expect(
    errors,
    `dioxus-web produced ERROR-level console output:\n${errors.join("\n")}`,
  ).toEqual([]);
});
