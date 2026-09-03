# Verdict: keystone `inv-sql-budget PINNED OpenTabViaModifierClick` at ff7448cc

## VERDICT: PRE-EXISTING (not caused by the wave-8 chain)

Not a registered pass-with-note: docs/Testing/KeystoneKnownReds.md has rows for
PinBlock / DeleteBackward / TypeChars sql_reads, none for OpenTabViaModifierClick.

## Method
- BASE = read-only `git archive 89e2efea` tree at $SCRATCH/base-89e2 (own target/, sccache bypassed).
- TIP  = /Users/martin/Workspaces/pkm/holon/.claude/worktrees/_sw_integ @ ff7448cc (working copy untouched).
- Same invocation both trees, alternating base/tip: HOLON_PERF_BUDGET=1 PROPTEST_MAX_SHRINK_ITERS=0
  PROPTEST_CASES=8 cargo test -p holon-integration-tests --features pbt --test general_e2e_composed_pbt.
- Cases raised from 1 to 8 because keystone-smoke draws OpenTabViaModifierClick rarely
  (1 sample per 20 one-case runs). Deviation: runs ran outside the parallel semaphore -
  it was starved for >1h by lanes holding it with larger -j; both trees ran under the same load.

## Rate table (verbatim from summarize.sh)
```
tip    tip-9 FAILED 0                   0
--- totals
base: runs=      32 opentab-budget-red-runs=       1 any-FAILED=      23 distinct_read_values=[4x22 6x23 1x24 ]
tip: runs=      31 opentab-budget-red-runs=       1 any-FAILED=      19 distinct_read_values=[4x22 10x23 1x24 ]
```

## Reading
- The pin overruns on PURE MAIN: base-28.log reds with the byte-identical assertion
  `OpenTabViaModifierClick.sql_reads: 24 exceeds expected 23 + tolerance 0 = 23 (watches=0, docs=5)`,
  same 4-repeated-text redundancy shape as the tip red in tip-13.log (docs=4).
- Overrun rate is indistinguishable between trees: base 1/11 OpenTab samples, tip 1/15.
- Everything else lands exactly on the pin (22 activate / 23 insert) in BOTH trees.
- The gate's magnitude (26, dedup 14, a THIRD watch_view at 3x) was not re-drawn in 63 runs;
  it is the same family at a larger draw. Magnitude tracks the draw, the assertion does not.
- No chain commit touches the cost model: crates/holon-pbt-core/src/budget.rs is byte-unchanged
  across 89e2efea..ff7448cc, and ff7448cc's harness edits (block_feed / home_by in-flight settle
  stages) add no SQL - they are observation-only atomic flags.

## Environment hazards found (report to orchestrator)
- sccache wedge: 16 sccache clients from one cargo stuck 69 min with ZERO rustc machine-wide;
  cleared after killing that build. Any lane seeing a build with no rustc children is hitting this.
- The holon-build semaphore is over-subscribed: 6 live holders on a -j4 request, starving -j4 callers
  for >1h (lanes using larger -j values raise the effective count).
