---
id: 2026-08-15-when-browser-denies-platform-preconditions-app
date: 2026-08-15
gap: ENVIRONMENT
secondary: null
status: PARTIAL
summary: >-
  When the browser denies the platform preconditions the app needs, nothing
  told the user which precondition was missing.
source_line: 698
---

## Bug

(task-#38 web lane; found by USER REPORT — Martin: an INCOGNITO tab on live
holon.space shows no data — and NOT REPRODUCED, recorded anyway because the
disclosure gap it exposed is real and fixable) **When the browser denies the
platform preconditions the app needs, nothing told the user which
precondition was missing.** The worker is wasm32-wasip1-threads and needs
`SharedArrayBuffer`, which exists only on a cross-origin-isolated page; on a
static host that isolation is faked by the coi-serviceworker, so any browser
policy blocking service workers removes SAB and the worker dies deep inside
wasm instantiation as an opaque `worker spawn: …`. NOT a silent hang — that
was checked and refuted: `WorkerBridge::spawn` races `WATCH_READY_TIMEOUT_MS
= 10_000` and flips the UI to `BootState::Failed`. The gap was purely that
the message never named the CAUSE.

## Root cause

task-#38 web lane, found by USER REPORT (Martin: an INCOGNITO tab on live
holon.space shows no data) and NOT REPRODUCED — recorded as an escape anyway
because the disclosure gap it exposed is real and fixable: **when the
browser denies the platform preconditions the app needs, nothing told the
user which precondition was missing.** The worker is wasm32-wasip1-threads
and needs `SharedArrayBuffer`, which exists only on a cross-origin-isolated
page; on a static host that isolation is faked by the coi-serviceworker, so
ANY browser policy that blocks service workers (incognito restrictions, an
extension, a per-site setting, enterprise policy) removes SAB and the worker
dies deep inside wasm instantiation as an opaque `worker spawn: …`. NOT a
silent-hang bug — that hypothesis was checked and refuted:
`WorkerBridge::spawn` races a ready-timeout and `WATCH_READY_TIMEOUT_MS =
10_000` flips the UI to `BootState::Failed` with a message, so a stalled
boot does surface as an error banner rather than an empty shell. The gap was
purely that the message never named the CAUSE. REPRODUCTION FAILED ACROSS
FIVE REAL-CHROME CONTEXTS and the negative result is the useful part:
bundled Chromium fresh context, real Chrome (`channel:'chrome'`) fresh
context headless, real Chrome `--incognito` headless, real Chrome
`--incognito` HEADED, real Chrome HEADED — all five identical,
`crossOriginIsolated=true`, `SharedArrayBuffer` present, coi-serviceworker
registered AND controlling, `navigator.storage.getDirectory()` OK, quota
7.6-10.7GB, worker ready in 2.8-5.0s, 75 DOM nodes, full sidebar+Welcome
content (`lane-logs/t38c-incognito.txt`, `t38c-headed.txt`); the deployed
bundle was byte-identical to what Martin was served (`last-modified
2026-08-15 16:33:22 GMT`, `holon-dioxus-web-8c6c145ecff6be38`). LIMIT OF THE
METHOD, disclosed rather than glossed: Playwright always launches Chrome
with a fresh temporary user-data-dir, so `--incognito` is near a no-op for
it — every context above is "a brand-new profile", NOT "an incognito window
inside Martin's long-lived profile", and the variables that differ
(installed extensions and their incognito enablement, per-site settings for
holon.space, clear-on-exit or third-party-cookie policy, enterprise policy)
are exactly the ones unreachable from a driven browser. All three candidate
causes (SW blocked / COI dance failing / quota denial) remain consistent
with his symptom and refuted in every constructible environment, so they are
NOT discriminated — claiming otherwise would be guessing. ENVIRONMENT: the
failing path is browser-policy-dependent platform code that no test
environment instantiates, and no gate loads the app over HTTPS with a
restricted profile at all. PARTIALLY FIXED — the disclosure half only:
`missing_platform_precondition()` now runs BEFORE the worker spawns and
reports the missing precondition by name, pointing at service-worker
blocking for the COI case and at privacy/enterprise policy for the SAB case.
VERIFIED NOT TO FALSE-POSITIVE, which was the actual risk since a wrong
preflight would brick the app for every user: full `trunk build --release` +
local serve + fresh context boots normally (`ready (2801ms)`, 75 nodes, full
content) with the day page still present
(`lane-logs/t38c-preflight-verify.txt`). ROOT CAUSE STILL UNKNOWN and a real
workaround may not exist — without COI there is no SharedArrayBuffer and the
threaded wasm worker cannot run at all; the next diagnostic step is four
values from Martin's failing tab (`crossOriginIsolated`, `typeof
SharedArrayBuffer`, `navigator.serviceWorker.getRegistrations()`, red
console lines), which discriminate all three hypotheses in one paste.)

## Missing piece

The failing path is browser-policy-dependent platform code that no test
environment instantiates, and no gate loads the app over HTTPS with a
restricted profile at all.

## Remedy

PARTIALLY FIXED — the disclosure half only:
`missing_platform_precondition()` now runs BEFORE the worker spawns and
reports the missing precondition by name, pointing at service-worker
blocking for the COI case and at privacy/enterprise policy for the SAB case.
VERIFIED NOT TO FALSE-POSITIVE, the actual risk since a wrong preflight
would brick the app for everyone: full `trunk build --release` + local serve
+ fresh context boots normally (`ready (2801ms)`, 75 nodes, full content,
day page present) — `lane-logs/t38c-preflight-verify.txt`. REPRODUCTION
FAILED ACROSS FIVE REAL-CHROME CONTEXTS, all `crossOriginIsolated=true` with
SAB present and the worker ready in 2.8-5.0s
(`lane-logs/t38c-incognito.txt`, `t38c-headed.txt`), and the deployed bundle
was byte-identical to what Martin was served. LIMIT OF THE METHOD,
disclosed: Playwright always launches with a fresh temporary user-data-dir,
so `--incognito` is near a no-op — every context was "a brand-new profile",
not "an incognito window inside Martin's long-lived profile", and the
differing variables (extensions, per-site settings, clear-on-exit or
third-party-cookie policy, enterprise policy) are exactly the ones
unreachable from a driven browser. ROOT CAUSE STILL UNKNOWN; all three
candidates (SW blocked / COI dance failing / quota denial) remain
undiscriminated. Next step is four values from Martin's failing tab.
