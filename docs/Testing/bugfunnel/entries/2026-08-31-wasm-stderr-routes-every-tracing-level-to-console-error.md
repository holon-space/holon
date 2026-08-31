---
id: 2026-08-31-wasm-stderr-routes-every-tracing-level-to-console-error
date: 2026-08-31
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Every wasm tracing line, INFO included, arrived as `console.error`, so the
  worker-smoke gate was permanently red and no real error could ever be
  distinguished from boot chatter.
---

## Bug

`npm run test:e2e` in frontends/dioxus-web failed with several hundred
`[console.error] [wasm] … INFO …` lines — ordinary boot logging. The gate
asserts zero ERROR-level console output, so it had no reachable green state and
could never have reported an actual worker error either. Found running the web
PBT surface (lane webpbt); red log `lane-logs/webpbt-playwright-run1.log`.

## Root cause

The wasm writes all `tracing` output to WASI stderr, and the WASI shim's
`printErr` mapped stderr straight to `console.error`
(frontends/holon-worker/web/worker-entry.mjs, web/wasi-worker-with-opfs-stubs.mjs).
The level lives inside the line's text, so the console channel carried no
severity at all.

## Missing piece

The gate's only discriminator is the console channel, and nothing preserved
severity across the WASI boundary — so the invariant it asserts was
unsatisfiable rather than merely failing. A permanently-red gate is
indistinguishable from a dead one: it was carried as a known red instead of
being read.

## Remedy

`frontends/holon-worker/web/wasm-log.mjs` reassembles stderr chunks into lines
before classifying any of them, then routes by the level token the fmt layer
emits. A line with no level continues the record before it; panics and aborts
carry no level, so they are matched explicitly and latch the sink to error so
their payload and backtrace lines follow; anything before the first level stays
an error.

A partial line is held rather than classified for as long as it keeps growing or
has been idle less than `STALE_PARTIAL_MS`, so a fragment cut before its level
token ("…Z ER") survives any number of event-loop ticks. A fragment that stops
growing is released marked `[partial line]`, taking its own level when a whole
token is present, error when it carries a *closed* fatal marker (`Aborted(OOM).`,
a panic line — these lack only their newline and are exactly what a dying worker
emits), and the latched severity otherwise.

**Residual, pinned as a KNOWN LIMIT test:** a stall longer than
`STALE_PARTIAL_MS` landing *inside* the ~5-byte level token still splits that one
record and demotes both halves — the released fragment has no token to promote
it and the remainder continues at the latched level. Accepted: the wasm does not
pause for seconds mid-token, and the same bytes a tick apart route correctly. It
is asserted rather than documented-only so a future fix surfaces as a test
change, not as silence.

**The lock is `frontends/holon-worker/test/wasm-log.test.mjs`** (23 cases,
`npm run test:unit` in `frontends/holon-worker`, wired into the same CI job as
the Playwright smoke). An earlier attempt at this fix was verified only by a
one-off manual probe, and three holes survived it — a `Aborted(OOM)` line after
boot chatter routed to `console.log` (a worker abort would have PASSED the
smoke gate), a panic's payload line inherited the latched INFO, and a level
token split across a buffer flush (`"…Z ER"` + `"ROR …"`) demoted a real ERROR.
All three are cases in the test now: red at
`lane-logs/webpbt-wasmlog-red.log`, green at `lane-logs/webpbt-wasmlog-green.log`.

A second attempt reassembled lines but flushed a partial after one event-loop
tick, which re-created the same demotion whenever a record spanned ticks — a
byte-per-tick stream shredded one record into 45 console lines. The suite could
not see it because its harness fed every chunk in one tick and called a flush by
hand, a path the browser never takes; it now feeds chunks a tick apart and lets
only the sink's own timer flush (red
`lane-logs/webpbt-wasmlog-r3-red.log`, green `-r3-green.log`).

Releasing a stale partial at the latched severity then turned the boot itself
red: the wasm writes `INFO di.create_backend_engine{db_path=`, opens the
database, and finishes the record — the fragment goes stale while the sink is
still latched at error because nothing has established a level yet, so the first
record of the session was reported as the session's first error. Found by the
Playwright gate, not by reasoning (`lane-logs/webpbt-r3-playwright.log`). Hence
the level-adoption rule and a 2s window.

Playwright green: `lane-logs/webpbt-playwright-run3.log` (and run4/run5 after
the worker was rebuilt).
