---
id: 2026-08-07-takes-803-975ms-interaction-visible-against
date: 2026-08-07
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  `split_block` takes 803–975ms interaction→visible against the 200ms SLO
source_line: 1162
---

## Bug

(overnight dogfood-explorer, throwaway vault, build at 5670921a)
**`split_block` takes 803–975ms interaction→visible against the 200ms SLO**
— p50 803ms, p95 944ms, max 975ms over n=5, on a vault of 31 blocks. The
app's own `holon_oracles` latency-slo fired and banner-disclosed all three
worst samples (`ORACLE VIOLATION: [latency-slo] interaction 'split_block' on
block:66038554-… took 975ms end-to-end (SLO: p95 <200ms)`), so the runtime
oracle EXISTS and works; nothing gates on it. Localized to the post-dispatch
half: `split_block`'s `dispatch` stage measures only p50 45ms / max 52ms,
leaving ~800ms in projection/paint. Explicitly NOT a debug-build artifact —
`set_field` in the same binary and same session measures e2e p50 11ms / p95
29ms over n=179, so two interactions on one build differ by ~70x.

## Root cause

secondary ORACLE: overnight dogfood — `split_block` interaction→visible
measures p50 803ms / p95 944ms / max 975ms (n=5) against the 200ms SLO, at a
31-block vault. The app's OWN `holon_oracles` latency-slo fired and
banner-disclosed it, so the oracle is not missing at runtime; what is
missing is any GATE that blocks on it. Localized: `split_block` DISPATCH is
only p50 45ms / max 52ms, so ~800ms is post-dispatch projection/paint. NOT a
debug-profile artifact — `set_field` in the SAME binary measures e2e p50
11ms / p95 29ms (n=179), a ~70x gap between two interactions on one build.
**ANNOTATED 2026-08-08 (task #10): the 803/944/975 figures are
MEASUREMENT-NOT-ESTABLISHED.** `split_block` is tokenless while
`block_raw.write_seq` is sticky on the row, so on a block that was ever
typed into the split's own CDC row arrived carrying the stale editor token,
matched no pending entry, and closed nothing — the entry expired and was
pruned in silence. The n=5 vs n=179 asymmetry quoted above IS that bias
signature, not sampling luck: only never-typed blocks could yield a sample,
and any sample that did close, closed on a LATER UNRELATED delivery for that
row. Windowed re-measurement at the same rev: e2e p50 36ms, dispatch p50
55ms, interaction→settled ~140ms. Fixed by the anonymous-delivery rule in
`crates/holon-api/src/latency_e2e.rs`; the row stays OPEN for
RE-MEASUREMENT, not for split-path optimisation. Evidence: task-#37 lane
report §3
(`docs/Testing/fixture-logs-2026-08-08/task37-windowed-latency-report.txt`)
+
`docs/Testing/fixture-logs-2026-08-08/latency-correlator-typed-gesture-8-splits.txt`
(the 8-typed-vs-8-untyped split asymmetry, verbatim),
`.../latency-correlator-probe-typed-split-no-e2e.txt`)

## Missing piece

The keystone is headless and measures the composed pipeline, never the GPUI
projection→paint path where the 800ms lives, so no test scale or profile
would surface it; and the runtime latency-slo oracle is disclosure-only — no
gate consumes it. Missing piece = a windowed latency gate that fails the
build on a `split_block` e2e p95 over budget, plus attribution of the ~800ms
post-dispatch stage (the named `projection` stage still never fires, so
`measure_latency.py` cannot break it down).

## Remedy

OPEN 2026-08-07 — diagnosis only, no fix attempted (dogfood is report-only).
Evidence: `/tmp/dogfood-2026-08-07/logs/latency.txt`,
`logs/errors-verbatim.txt`, `shots/05.png`. **ANNOTATED 2026-08-08 (task
#10): the 803/944/975 numbers are MEASUREMENT, NOT ESTABLISHED LATENCY — do
not optimise the split path against them.** The correlator could not close a
`split_block` clock at all on a block that had ever been typed into (sticky
`block_raw.write_seq` on a tokenless op; see the 2026-08-08 correlator row
below), so this sample set is biased by construction — the n=5 vs
`set_field` n=179 gap in the same session is that bias, and every sample
that did close, closed on a later unrelated delivery. Windowed
re-measurement at the same rev: e2e p50 36ms / dispatch p50 55ms /
interaction→settled ~140ms, i.e. no ~800ms post-dispatch cost. Correlator
fixed in `crates/holon-api/src/latency_e2e.rs`; this row stays OPEN pending
RE-MEASUREMENT with the fixed correlator. Evidence: task-#37 lane report §3
(`docs/Testing/fixture-logs-2026-08-08/task37-windowed-latency-report.txt`),
`docs/Testing/fixture-logs-2026-08-08/latency-correlator-typed-gesture-8-splits.txt`
(the 8-typed-vs-8-untyped split asymmetry, verbatim),
`.../latency-correlator-probe-typed-split-no-e2e.txt`.
