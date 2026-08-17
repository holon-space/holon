---
id: 2026-08-12-latency-disclosure-compiled-out-release-build
date: 2026-08-12
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  The `stage="e2e_retired"` latency disclosure is compiled out of every
  release build
source_line: 721
---

## Bug

(latency-arch-red lane, task #21; found by another lane's verifier running
`holon-architecture-tests`, a package no landing gate runs; no gated test
produced it) **The `stage="e2e_retired"` latency disclosure is compiled out
of every release build**, so in the build Martin dogfoods a refused op
retires its pending entry in silence — `tracing/release_max_level_info`
(turso `workspace-hack`, feature-unified graph-wide) deletes `debug!`
callsites, and `latency_e2e.rs:292` was the only `holon_latency` event below
INFO; its siblings are INFO/WARN/ERROR. Release latency logs therefore
showed a `dispatch` with no `e2e` sample and no reason.

## Root cause

latency-arch-red lane (task #21), found by ANOTHER LANE'S VERIFIER running a
package the landing gate does not run — no automated test in any gate
produced it: **the `stage="e2e_retired"` disclosure is compiled out of every
release build, so in the build Martin dogfoods a refused op's latency entry
is retired in total silence.** The turso `workspace-hack` enables
`tracing/release_max_level_info`, which feature-unifies across the whole
graph, so a `debug!` callsite does not exist in a release binary.
`crates/holon-api/src/latency_e2e.rs:292` emitted the retirement at `debug!`
while every sibling `holon_latency` event in the file is INFO (`e2e`
sample), WARN (`e2e_expired`, `e2e_delivery_partial`) or ERROR
(`e2e_delivery_unreadable`) — so release-build latency logs show a
`dispatch` with no `e2e` sample and no explanation of where the interaction
went, the "silently degrades to look fine" case the module exists to
prevent. A/B ATTRIBUTION (`git grep -B3 'target: "holon_latency"'` at three
revs): clean at `ec4f064399` (the 2026-07-29 fix that added the arch rule)
and clean at `c045e6dccf^`, offending at `c045e6dccf` (2026-08-08, the
tokenless-op/retire-refused-op correlator fix) — the rule pre-dated the
callsite by ten days, so `c045e6dccf` LANDED THE RED and it sat on `main`
for four days. That commit's own gate list names fmt, holon-api,
keystone-smoke, hand-authored, `cargo check`, bugfunnel-check —
`holon-architecture-tests` is absent, which is the actual escape: the
compensating oracle existed and fired, nothing ran it. FIXED in this lane:
the callsite is `tracing::info!`, matching the `e2e` sample it accounts for
(a refusal is an ordinary outcome, not the anomaly `e2e_expired`'s WARN
marks); default log volume stays held by the `holon_latency` EnvFilter
directive in `holon_frontend::logging`, not by the callsite level. The stale
module-doc line advertising `tracing::debug!` for `stage="e2e"` (the code
has been `info!` all along) is corrected in the same edit.)

## Missing piece

Nothing is missing from the oracle: the arch rule
`latency_events_are_emitted_above_the_release_level_ceiling` was landed
2026-07-29 by `ec4f064399` and detects this exactly. What is missing is the
rule in a GATE — `c045e6dccf` (2026-08-08) introduced the `debug!` callsite
with a gate list that names
fmt/holon-api/keystone-smoke/hand-authored/`cargo check`/bugfunnel-check and
NOT `holon-architecture-tests`, so the red landed and sat on `main` for four
days. A/B: clean at `ec4f064399` and `c045e6dccf^`, offending at
`c045e6dccf`. Remedy = add `holon-architecture-tests` to the prepush/landing
gate composition.

## Remedy

FIXED 2026-08-12 (task #21): callsite raised to `tracing::info!`, matching
the `e2e` sample it accounts for — a refusal is an ordinary outcome, not the
anomaly `e2e_expired`'s WARN marks; log volume stays held by the
`holon_latency` EnvFilter directive in `holon_frontend::logging`. Full
`holon-architecture-tests` package green (5/5). The gate-composition remedy
is NOT done here and stays open.
