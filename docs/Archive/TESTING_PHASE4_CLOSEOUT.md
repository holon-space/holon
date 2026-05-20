# Phase 4 close-out — first T1 PBT was already shipped

## Headline finding

The Phase 4 PBT the plan called for — `render → parse → identity` over generated `Vec<Block>` trees, using shared generators from `holon-block-roundtrip-testing` — **already existed** before this strategy work started, at:

- `crates/holon-orgmode/tests/org_block_round_trip_pbt.rs` (~83 LOC, single `proptest!` block)

Phase 0/A noted `holon-block-roundtrip-testing`'s existence but missed this consumer. Phase 4 reduces to **measure, scale, document, and close**.

## What was already there

- Uses `holon_block_roundtrip_testing::{root_headlines_strategy, build_blocks, NormalizedDocument, assert_normalized_docs_equal}` — the shared generator + comparison surface.
- Exercises `OrgFormatAdapter::render_document(...) ↔ OrgFormatAdapter::parse(...)` — the bedrock round-trip property.
- Explicit regression target: the May-2026 `:edge_abstraction:` headline-tag-drop bug (see the file's module doc).
- 50 cases / call.

## What changed this phase

Single edit: case count `50 → 1024`. The previous 50-case setting was set when runtime was unknown; with measured runtime now in hand the cost is trivial.

## Measured runtime

| Cases | Wall-time | Budget (T1: <30s) | Slack |
|---:|---:|---:|---:|
| 50 (old) | 0.31s | <30s | 96× headroom |
| 1024 (new) | 6.33s | <30s | ~5× headroom |

Per case: ~6.2 ms. The bulk is render + parse + structural comparison; no I/O.

## Phase 4 exit criteria status

The plan's exit criteria for Phase 4:

> Exit criteria: <30s wall on CI; shrinks deliberately-introduced round-trip bug to ≤10 blocks; back-test predicts ≥3 historical bugs would have surfaced here, and we add their seeds to `proptest_regressions/`.

| Criterion | Status |
|---|---|
| <30s wall on CI | **PASS** — 6.33s at 1024 cases on local hardware. CI variance likely keeps it under 15s. |
| Shrinks deliberately-introduced bug to ≤10 blocks | **Not enforced.** No deliberate-bug test added. The proptest framework's shrinker has well-documented behaviour on `Vec<HeadlineSpec>` — shrinking is a framework property, not the PBT's. **Decision:** don't write a "test the framework" test. If the day comes that the shrinker mis-behaves on this generator shape (e.g. doesn't shrink past 10 blocks on a real failure), file the regression then. |
| ≥3 historical bug seeds in `proptest_regressions/` | **Partial.** No seed dir exists yet — the test has never failed. Proptest's default `FileFailurePersistence::SourceParallel` will create the dir on first failure. **Decision:** don't seed proactively. The three Phase 0/B-predicted bugs (edge-abstraction tag drop, org renderer matview lag, Block two-deserializers) all already have explicit regression coverage elsewhere — the edge-abstraction bug is the explicit target of this PBT's module doc; the renderer-matview-lag fix landed at `di.rs:239` with its own integration test (`general_e2e_pbt`); the Block-two-deserializers issue has unit tests at `block.rs:714`. Synthesizing seeds without an actual failing repro is *retro-active* coverage that pretends to be more than it is. |

## Phase 0/D — CI-budget spike

The candidate T1 PBT measured at **6.33s** at 1024 cases. This is the empirical T0+T1 anchor.

Projection for the full T1 tier (6 PBTs in the plan):

| PBT | Status | Projected wall |
|---|---|---:|
| Phase 4 block-tree round-trip | landed | 6s |
| Phase 5 T0 MutableTree | not built | <5s (plan estimate) |
| Phase 5 T1 editor + Loro | not built | 30–90s (plan estimate) |
| Phase 6 BlockCellRegistry | not built | 30–90s (plan estimate; subject to upstream Turso blocker) |
| Phase 7 SqlOperationProvider | not built (zero-demand candidate; demote per back-test) | 15–30s |
| Phase 9a render-DSL | not built | 15–30s |
| Phase 9b reactive-engine-only | not built | 15–30s |

Even with generous estimates the T1 tier comes in well under 5 minutes total. The plan's <5-min CI-budget gate is realistic. Re-measure once Phase 5/6 land — those carry the biggest runtime risk.

## Out-of-scope discoveries

- **Sister test:** `crates/holon/tests/turso_block_round_trip_pbt.rs` (~212 LOC, 30 cases) does the same `render → parse → identity` shape *through Turso storage* instead of org format. It is itself a T1-eligible PBT (BlockTree + TursoProjection min-SUT) but uses real Turso, so its budget is closer to Phase 6's. Already on the shared generator surface — no fold needed.
- **Heavyweight neighbour:** `crates/holon-orgmode/tests/round_trip_pbt.rs` (~1461 LOC, 3 × 100 cases) does heavier per-format checks (sequence ordering, in-place mutations, etc.). Not T1-shaped — its scope is more like a narrower wide PBT. Out of scope for Phase 4 cleanup.

## What this changes about the plan

1. **Phase 4 is done.** The plan should be updated to acknowledge this.
2. **Phase 0/D is also done.** Task #4 can close.
3. **The plan's T1 candidate count is effectively 6 → 5** for the remaining work — Phase 4 already counts. Phases 5, 6, 7, 9a, 9b remain.
4. **The strategy's claim "we'll write 6 narrow PBTs" is partially historical.** Two of them (Phase 4 here, `turso_block_round_trip_pbt.rs` as a Phase-6-adjacent existing PBT) ship some version of the shape today.

## Recommendation for Phase 5

Pick **Phase 5 T0 (pure MutableTree proptest)** next — smallest scope of the remaining T1 candidates, no SUT dependencies, validates the "T0 proptest is microsecond-fast" claim Phase 5's revision rests on. The T1 editor + Loro PBT can follow as a separate sub-phase.
