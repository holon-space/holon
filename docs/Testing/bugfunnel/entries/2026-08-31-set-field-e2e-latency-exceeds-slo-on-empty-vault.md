---
id: 2026-08-31-set-field-e2e-latency-exceeds-slo-on-empty-vault
date: 2026-08-31
gap: ORACLE
secondary: ENVIRONMENT
status: PARTIAL
summary: >-
  Typing into a block measured p50 216ms / p95 356ms interaction-to-visible on a
  20-block vault against a 200ms p95 SLO, and no gate fails on it. A/B against
  pre-wave main shows the number is PRE-EXISTING and is 95% queue wait: writes
  arrive ~4x faster than the pipeline drains them, and `stage="e2e"` bills every
  interaction for the ones queued ahead of it.

  PARTIAL (2026-08-31): the ORACLE half is FIXED — the SLO is now two gated
  rungs (service-time p95, throughput floor) scored by one type that the
  runtime oracle and the land gate share, so a breach fails a build instead of
  painting a banner. The ENVIRONMENT half — the ~19ms per-write drain itself —
  stays OPEN; see Remedy.
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

## Latency A/B (lane `latency-ab`, 2026-08-31)

Reproduced on `main fcfe50fb` and on pre-wave `main cc3439f0ab25` — the rev
before sidebar-dismiss / web-rebind / linked-refs / actionbar / pin landed.
Debug `cargo build -p holon-gpui --features pbt` on both, fresh sandbox, empty
vault, one app alive at a time, same machine, same script. n=32 per arm per
replicate, two replicates per tree.

| tree | arm | n | p50 | p95 | max | mean |
|---|---|---|---|---|---|---|
| main fcfe50fb r1 | burst | 32 | 237.0 | 443.8 | 467.0 | 237.4 |
| main fcfe50fb r2 | burst | 32 | 230.5 | 419.2 | 441.0 | 227.8 |
| base cc3439f0 r1 | burst | 32 | 238.5 | 435.8 | 460.0 | 238.4 |
| base cc3439f0 r2 | burst | 32 | 240.0 | 434.2 | 458.0 | 239.1 |
| main fcfe50fb r1 | paced | 32 | 11.0 | 12.0 | 13.0 | 10.8 |
| main fcfe50fb r2 | paced | 32 | 11.0 | 12.9 | 15.0 | 10.9 |
| base cc3439f0 r1 | paced | 32 | 10.0 | 13.4 | 21.0 | 10.6 |
| base cc3439f0 r2 | paced | 32 | 10.0 | 12.0 | 19.0 | 10.6 |

Numbers taken verbatim from `latency-ab/lane-logs/arms-main-fcfe50fb-r1.json`,
`arms-main-fcfe50fb-r2.json`, `arms-base-cc3439f0-r1.json`,
`arms-base-cc3439f0-r2.json`; app logs `/tmp/latency-ab-8730/app.log` (main) and
`/tmp/latency-ab-8731/app.log` (base); span decomposition
`latency-ab/lane-logs/span-analysis.log`; per-write work counts
`latency-ab/lane-logs/per-write-work.log`. Reproduce with `scripts/latency/`.

The two arms differ only in how fast writes enter the pipeline: **burst** is one
`type_text` of 32 characters (dispatches ~5ms apart), **paced** is 32 separate
`type_text` calls with `await_quiescence` between each (one interaction in
flight at a time).

## Root cause

**PRE-EXISTING, and an arrival-rate artifact rather than a per-interaction
latency.** The wave did not move it — burst p50 237.0 / 230.5ms on main against
238.5 / 240.0ms on the pre-wave base, paced p50 11 / 11 against 10 / 10. The
base tree is marginally slower.

`stage="e2e"` is a dispatch→row-in-mirror wall clock. Recovering each sample's
dispatch instant as `delivery_ts - ms` from the same log shows why the number is
large:

```
=== main-fcfe50fb-r2
  arrivals spread over   : 149 ms  (mean interval 4.8 ms)
  deliveries spread over : 578 ms  (mean interval 18.7 ms  = drain rate)
  burst p50 e2e          : 237 ms
  service floor (paced)  :  11 ms
  queue wait             : 226 ms  (95.4% of the burst p50)
=== base-cc3439f0-r2
  arrivals 5.0 ms · deliveries 19.1 ms · p50 246 · floor 10
  queue wait             : 236 ms  (95.9% of the burst p50)
```

Writes arrive ~3.9x faster than the pipeline drains them, so the queue grows
monotonically and each interaction is billed for everything ahead of it. The
reported series is a queue-depth ramp, not a latency distribution: 12, 26, 40,
53 … 441ms here, and **the dogfood log has the identical shape** — 12, 36, 82,
101 … 371ms, arrivals ~3.3ms apart, deliveries ~21ms apart. It stopped at 371ms
only because the string ran out at 19 characters.

**The localized span is queue wait inside the `live_data` CDC delivery actor:
95.4% (main) / 95.9% (base) of the burst p50.** The drain rate — ~18.7ms per
write, i.e. a capacity of roughly 53 writes/s — is the real throughput number,
and it is currently unnamed and ungated. A human at 10 char/s never reaches it;
MCP-driven exploration reaches it every time.

Why the writes arrived that fast: every keystroke in every run logged
`[interaction-pump] WINDOW-INACTIVE`, as the dogfood run did. With the window
OS-inactive the input pump is not frame-paced, so 32 keystrokes enter in 149ms.
With the app window frontmost the same 32-character `type_text` takes 940ms to
arrive (~29ms/char) and e2e stays flat at p50 15.0 / p95 16.4 / max 19.0
(`lane-logs/measure-main.json`); activating another app and repeating gives p50
89.0 / p95 100.3 (`lane-logs/measure-main-inactive.json`). Window state is the
dominant covariate.

The residual ~11ms service floor sits in exactly one un-instrumented gap, from
`backend.execute_operation` to `live_data.apply_batch` — Loro commit,
LoroProjection resample, Turso write, IVM matview maintenance and CDC emit all
happen inside it and emit nothing:

```
12:55:52.875676  interaction.dispatch{…set_field}:backend.execute_operation{…}
12:55:52.876036  interaction.dispatch{…set_field}:backend.execute_operation{…}
        (11.3 ms with no event of any kind)
12:55:52.887335  live_data.apply_batch{source="block" seq=707}  stage="e2e" ms=11
12:55:52.887339  live_data.apply_batch{…}                       stage="rows" ms=0
```

The named `projection` stage never fires at all, and `rows` is 0.0ms across 160
events per tree. Naming that gap is still worth doing — but at 11ms it is not
what breaches the SLO.

What sets the ~18.7ms drain is the post-delivery tail, which runs per keystroke
and is identical on both trees (per 64 writes): ~95 `subscribe_cdc('watch_view_…')`
matview subscriptions, ~74 `org.on_block_changed` file write-backs, 70
`render_entity` completions. The org write-back is on the write path but starts
~9ms AFTER `stage="e2e"` closes, so it caps throughput, not latency.

Disclosed: this is a `cargo build -p holon-gpui --features pbt` debug binary,
which inflates absolute numbers. The ~8ms baseline in the skill was taken on a
debug build too, so the comparison holds; a release re-measure should still be
part of the fix. The A/B is unaffected — both trees were built debug on the same
machine, base with the `nightly-2026-07-17` it pins, main with `nightly-2026-08-16`.

Classification unchanged (`gap: ORACLE`, `secondary: ENVIRONMENT`) — the
mechanism refines it rather than contradicting it. Worth noting for whoever
re-triages: the ENVIRONMENT half (backgrounded window, MCP-paced input) is what
produced the specific number, and the ORACLE half is now a sharper claim than
"no gate fires" — see below.

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

so the running app can breach its own SLO and every automated check still
reports green. The budget invariant does not fire where it would be caught.

The A/B sharpens what that gate would have to score. Gating `stage="e2e"` as it
stands does not measure the tree: under saturation it measures queue depth, so
such a gate would fail whenever an agent types faster than a human can, and stay
green on a genuinely slow pipeline that happens to be driven slowly. The same
objection applies to the in-window `latency-slo` oracle, which read 301–354ms off
a queue and painted it as five per-interaction violations.

**Ruled by Martin, D50.a (2026-08-31):** the latency SLO gate is **service
time** — one interaction in flight, p95 < 200ms — plus a **separate throughput
floor** on the drain rate (≥ N writes/s, N still to be proposed; ~53/s measured
here). Service time today is 10–11ms p50, well inside that. Building those two
rungs is the open work, not the shape of them.

## Remedy

### 1-2. Instrumentation — PARTIALLY DONE

The `e2e` event now carries `in_flight` (queue depth at dispatch) and `backlog`
(queue depth after delivery), which is remedy item 2: any consumer can tell a
queued measurement from a quiet one. `crates/holon-api/src/latency_e2e.rs`.

Item 1 — naming the unattributed `backend.execute_operation` →
`live_data.apply_batch` gap and making the `projection` stage fire — is NOT
done. Still open, and still worth doing: at ~11ms it does not breach the SLO,
but it is the only un-named span on the write path.

### 3. The two D50.a rungs — DONE (the ORACLE half)

`crates/holon-integration-tests/tests/latency_slo_gate.rs`, three tests:

| rung | what it scores |
|---|---|
| `latency_slo_rung_service_time_p95` | paced drive, p95 of samples alone in the pipeline for their whole life, against the 200ms SLO — GATED |
| `latency_slo_rung_throughput_floor` | 150-write burst through the production fire-and-forget door, deliveries/second across saturated intervals — REPORT-ONLY on the rate |
| `a_slowed_pipeline_moves_the_service_statistic` | wiring: arms a per-row delay in the real CDC apply path, asserts the service statistic moves (p50 22-45ms clean → 112-125ms slowed) |

The VERDICT-FLIP half of falsification lives in `holon_api::latency_slo`'s unit
tests (`service_rung_fails_on_a_slow_paced_pipeline`,
`throughput_rung_fails_a_slow_drain`, plus pins for both false-red estimators
this lane shipped and lost). It is not asserted at integration level after
measurement: across injected per-row delays of 60/250/300ms and drives of
30/40/80 writes, any injection strong enough to flip a verdict also destroys the
sample population the verdict needs (service samples fell to n=0/6/19/28 against
a floor of 30; the burst delivered 0 of 150 writes at EVERY delay tried, 60ms
and 250ms alike). A flip assertion there would report INCONCLUSIVE on an
unmodified tree. The shipped injection is 250ms/row on the paced arm only.

Both rungs score `holon_api::latency_slo::SloWindow`, and the runtime
`latency-slo` oracle was rewritten to score the SAME type — so the banner and
the gate cannot report different numbers. The oracle is now edge-triggered on a
rung turning red rather than firing per event; the five banners this entry
records off one queue ramp can no longer be produced (pinned by
`a_queue_ramp_paints_no_service_violation` in `crates/holon-oracles/src/latency.rs`).

`just latency-slo-gate` is step 9 of `just landing-gate`.

Measured on an unmodified tree, headless debug: service p95
25/27/30/34/38/45/47/48/53/58/63/68/69ms against the 200ms budget, plus one
admitted run at 183ms — so the contention covariate does not capture every
source of load the service rung is sensitive to, and a red there is worth one
re-run on a confirmed idle host before it is believed. Both rungs refuse to
score a run whose mean boot `matview_ddl` exceeds 30ms, reusing the covariate
and cut `just latency-gate` already uses; a refused run panics with INVALID
rather than passing, so "too busy to judge" can never read as green.

**The throughput rung is REPORT-ONLY on its rate.** On admitted hosts an
unmodified tree measured 42.8 / 64.4 / 460.3 / 460.7 / 809.6 / 1021.4 writes/s
with the current estimator — a 24x spread that no floor can straddle. (Earlier 7.7-27.0/s figures
came from a since-replaced estimator and are void.) The rate is printed every run and cannot fail
a build; what still fails is a burst that saturated nothing, sampled nothing, or
lost more than its stated share of writes. Same treatment
`docs/Testing/latency-ceilings.txt` gives its SplitBlock rungs. Promotion needs
the spread attributed and five agreeing admitted runs — condition stated at
`THROUGHPUT_FLOOR_WRITES_PER_SEC`.

**Burst write loss (disclosed, harness artifact).** 150 dispatched writes yield
~62 deliveries; the fire-and-forget door returns `Ok` regardless, so the rung
now counts landed/dispatched, prints the loss on every run, and fails above a
budget. Cause is a Loro store whose backing tempdir is dropped immediately after
construction (`crates/holon-integration-tests/src/pbt/composed/builder.rs:660-663`,
whose comment asserts the loro-only config "never persists"), so `save_all` →
`write_atomic` (`crates/holon-loro/src/loro_document.rs:332`) hits ENOENT under a
burst. `write_atomic` propagates the error correctly and the paced rung — same
wiring, same blocks — never trips it. Fixture lifetime bug, not a product
defect: NO bugfunnel entry filed, recorded here because it bounds what the
throughput number means.

**Verifier's ruling on promotion (2026-08-31):** the throughput floor must NOT
be promoted to a gate until this ENOENT artifact is fixed, no matter how many
quiet-host runs agree. A lost write still consumes denominator wall time while
contributing no delivery, so every rate measured today is depressed by an
amount that varies with the loss — a floor calibrated on those numbers would
encode the artifact, not the pipeline.

**Known blind spot in the BANNER (not the gate).** Rung 1 scores only samples
that were alone in the pipeline for their whole life. A pipeline slow enough to
be worth catching is slow enough to build a queue, and a queued sample is
excluded — so in production the service rung trends toward `Unjudged`/`Pass`
exactly when things are worst. Measured: 30 fast quiet samples plus 10 genuine
900ms queued ones score `p95=10ms Pass`, all ten dropped. For the GATE this is
harmless (the harness settles between transitions, so pacing holds by
construction and the exclusion removes only contamination). For the oracle it is
a real under-report, and closing it needs a queue-aware service estimator — not
a looser filter. Follow-up, not fixed here.

**Where the fault injector is compiled.** `holon-api`'s `slo-fault-injection`
feature is enabled only through `holon-integration-tests/test-infra`, which is a
DEFAULT feature of that crate — so anything building it compiles the injector,
including the `diag_harness` example (`justfile:537`, `just heap-profile`). That
is a diagnostic harness, not a shipped frontend; no frontend or release binary
enables the feature, and the sleep is `#[cfg]`-compiled out of them entirely.

**Coverage gap closed alongside this.** The oracle's THROUGHPUT banner branch
had no test — all six existing ones exercised the service branch — so a
regression that stopped the drain-rate half raising would have gone unnoticed.
`a_slow_drain_paints_a_throughput_banner` now drives that arm.

**A third consumer is still on the refuted estimator.** `just soak`
(`justfile:518`) judges the same 200ms SLO via
`scripts/measure_latency.py --fail-over-p95 200`, i.e. raw `stage="e2e"` `ms`
under load — the measurement this entry proves is queue depth, not latency.
Flagged rather than fixed: the soak drives a deliberately saturated 5-10k-block
workload, so pointing it at `SloWindow` means deciding what a service-time
statistic even means there, which is its own ruling.

### The ENVIRONMENT half — STILL OPEN

The ~18.7ms drain (roughly 53 writes/s in the app) is unchanged; nothing here
made a write cheaper. What changed is that it is now NAMED and gated, so a
regression against it fails a build. Reducing it is follow-up work:

* Coalesce the org write-back — ~74 `org.on_block_changed` file writes per 64
  writes, starting ~9ms AFTER `stage="e2e"` closes, so it caps throughput.
* Instrument the `execute_operation` → `apply_batch` gap (remedy item 1 above).
* Re-measure in a release build; every number in this entry is debug.
