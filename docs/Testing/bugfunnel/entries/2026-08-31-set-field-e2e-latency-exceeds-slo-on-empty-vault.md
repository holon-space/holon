---
id: 2026-08-31-set-field-e2e-latency-exceeds-slo-on-empty-vault
date: 2026-08-31
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Typing into a block costs p50 216ms / p95 356ms interaction-to-visible on a
  20-block vault, against a 200ms p95 SLO — and no gate fails on it.
---

## Bug

Found by the `dogfood-explorer` gate typing 19 characters into one block in a
real GPUI window (lane `dogfood-mobile`, port 8720, sandbox vault holding 20
blocks total).

`python3 scripts/measure_latency.py /tmp/dogfood-mobile-sandbox/logs/app.log`
(lane copy: `lane-logs/latency.log`):

```
== PROD END-TO-END  interaction -> visible  (stage=e2e) ==
action           n      p50      p95      max     mean  (ms)
set_field       19    216.0    355.7    371.0    208.1
navigate         2     31.5     38.2     39.0     31.5
```

SLO is p95 interaction→projection-visible < 200ms. `set_field` misses it at
the p50, not just the tail — every keystroke is over budget.

The app's own in-window oracle agreed and said so on screen
(`shots/05-backlink.png`):

```
ORACLE VIOLATIONS (5) — live invariant check failed
[latency-slo] interaction 'set_field' on block:dogfood-link-1 took 301ms
              end-to-end (SLO: p95 <200ms)      … 319ms … 337ms … 354ms
```

Boot also shows `matview_ddl` at max 467ms (n=124, p95 56.8ms), which trips the
same diagnostic during startup.

Scale is not the explanation: 20 blocks, empty vault, fresh boot. The
`dogfood-explorer` skill records the expected figure for exactly this
configuration and build profile as "set_field e2e p95 ~8ms". This is ~44x that
baseline.

## Root cause

Not root-caused from this channel — the measurement is the finding. The
per-stage breakdown accounts for almost none of it: the only non-boot stage
recorded is `rows (CDC batch apply)` at p50/p95/max = 0.0ms over 78 samples,
with `rows per CDC batch: p50=1 max=1`. So the 216-356ms sits between the
interaction and the visible projection in a span the current
`holon_latency` stages do not name.

Disclosed: this is a `cargo build -p holon-gpui --features pbt` debug binary,
which inflates absolute numbers. The ~8ms baseline in the skill was taken on a
debug build too, so the comparison holds; a release re-measure should still be
part of the fix.

## Missing piece

The `latency-slo` oracle exists and fires — it painted the banner and wrote the
WARN lines. What does not exist is a GATE that fails on it. `run_self_checks`
skips the budget family entirely against a live app:

```
inv-settle-budget      skipped  class-3 temporal/budget check: scores a
                                per-tick accounting window a one-shot live
                                sweep does not have
inv-sql-budget         skipped  (same)
inv-complexity-class-trend  skipped  (same)
```

so the running app can breach its own SLO by 78% at the p95 and every
automated check still reports green. That is an ORACLE gap in the strict sense
the triage rules name for latency: the budget invariant does not fire where it
would be caught.

## Remedy

Open. Two parts: instrument the unattributed span between interaction and
visible so the cost has a name, and make the budget invariant score a gate
rather than only paint a banner — the composed keystone already has the
per-tick accounting window the live sweep lacks.
