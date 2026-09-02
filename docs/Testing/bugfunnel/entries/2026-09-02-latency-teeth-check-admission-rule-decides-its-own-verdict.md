---
id: 2026-09-02-latency-teeth-check-admission-rule-decides-its-own-verdict
date: 2026-09-02
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  The latency-SLO gate's teeth check asserts a fixed 90ms median against a
  self-selecting sample population, so a faster or quieter host admits more
  cheap writes, drags the median under the line, and reds the gate on an
  unmodified tree.
---

## Bug

The land battery for the sharing chain failed on
`crates/holon-integration-tests/tests/latency_slo_gate.rs:503`,
`a_slowed_pipeline_moves_the_service_statistic`, with:

> the 250ms per-row injection did NOT move the service statistic: p50 71ms
> (max 164ms) over n=56, under the 90ms this check requires — an unslowed tree
> measures 22-45ms here.

The same test passed on plain main in a land ~2h earlier, and the load average
was 2.2, so it was not a load artifact. An attribution lane A/B'd it: three runs
per tree, interleaved in one machine window, WITHOUT the chain (plain main
`f27c79b7db4d`) against WITH it (chain tip `50f878cc`).

All six runs PASSED, on both trees. The red is not chain-caused.

| Order | Tree | p50 (ms) | max (ms) | n | Result |
|---|---|---|---|---|---|
| 1 | WITHOUT | 123 | 164 | 28 | pass |
| 2 | WITH | 108 | 159 | 29 | pass |
| 3 | WITHOUT | 90 | 173 | 50 | pass, exactly at threshold |
| 4 | WITH | 125 | 151 | 25 | pass |
| 5 | WITHOUT | 126 | 164 | 38 | pass |
| 6 | WITH | 142 | 166 | 25 | pass |

Medians: 123ms without the chain, 125ms with it. Run 3 passed by zero
milliseconds on an unmodified tree.

## Root cause

A sample counts toward the statistic this check asserts on only when the
interaction was service time alone: `in_flight == 1 && backlog == 0`
(`crates/holon-api/src/latency_slo.rs:153`). Under the 250ms per-row injection
most writes overlap and are excluded. What survives is a SELF-SELECTING
population biased toward the cheapest writes, the ones touching fewest injected
rows, because only those both start and finish alone.

Sample count and median are therefore anticorrelated. The six A/B runs plus the
failing land run show it directly:

| n | 25 | 25 | 28 | 29 | 38 | 50 | 56 (land red) |
|---|---|---|---|---|---|---|---|
| p50 (ms) | 142 | 125 | 123 | 108 | 126 | 90 | 71 |

The faster or quieter the host, the more cheap single-row writes complete alone
and are admitted, and the further the median falls below the fixed 90ms line.
The max stayed in a tight 151-173ms band in every run, including those with the
lowest medians, so the injection reached the pipeline every time. The admission
rule, not the injection, decides the verdict.

The doc comment at `crates/holon-integration-tests/tests/latency_slo_gate.rs:459`
calibrates `TEETH_MIN_SLOWED_P50_MS = 90` as "~1.25x below the slowed floor",
citing slowed runs of 112-125ms. This lane measured a slowed floor of 90 on an
unmodified tree and the land battery saw 71, so that calibration no longer holds
on this host.

## Missing piece

No property ties the statistic under assertion to the population it is computed
over. The check reads a median whose meaning changes with host speed, then
compares it to a constant. A wiring check that asks "did the injection reach the
scorer" has an answer independent of host speed, and the current formulation
does not ask that question.

## Remedy

OPEN. No code was changed by the attribution lane; its mandate ended at the
measurement. Remedy candidates, in preference order, all in the admission rule
or the statistic rather than the threshold:

1. **Assert on the max, or a high quantile.** It stayed within 151-173ms across
   every run of both trees while the median swung 71-142ms. It is the statistic
   that actually responds to the injection rather than to the host.
2. **Score the whole armed population, not the service-time subset.** The
   wiring question is whether the delay reached the scorer at all; excluding
   overlapping interactions serves the SLO rungs, not this check.
3. **Normalize by n, or require a sample count band.** If the median is kept,
   the assertion has to account for the population it was computed over,
   because the two move together.

Lowering the 90ms constant is NOT a remedy: it treats the symptom and leaves a
gate whose verdict a quiet machine can flip.

Evidence: A/B lane report and six per-run logs in
`lane-logs/` of the `share-create-routing` workspace
(`lane-report-latency-teeth-ab.md`, `teeth-with-r{1,2,3}.log`), plus
`teeth-without-r{1,2,3}.log` in the `main-baseline` workspace.
