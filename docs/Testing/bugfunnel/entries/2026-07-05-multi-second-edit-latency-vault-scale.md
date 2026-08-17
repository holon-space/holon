---
id: 2026-07-05-multi-second-edit-latency-vault-scale
date: 2026-07-05
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Multi-second edit latency at vault scale (pass_ms ≈ 11.3 + 0.221×blocks)
source_line: 866
---

## Bug

Multi-second edit latency at vault scale (pass_ms ≈ 11.3 + 0.221×blocks)

## Missing piece

no latency-budget invariant; test scale « vault scale

## Remedy

open; SLO defined 2026-07-07. RE-MEASURED 2026-07-16 (clean soak, CRDT-on,
1500 & 4000 blk): root cause isolated to `LoroProjection::project()`
FULL-DOCUMENT RESEED (`loro_sync_controller.rs:445`) — the landed O(changed)
incremental path falls back to full DFS-snapshot + whole-tree diff on
ordinary interactive edits (empty-pending-moved-frontier, unsettled batch,
orphan-create, oversized). Confirms the formula: full pass p50=855/p95=1128
ms @4000 blk; navigate `focus`=2674 ms; snapshot_ms 13→275 ms (1500→4000).
CDC/Turso-IVM exonerated (`stage=rows` 0-2 ms all scales). `async_io
false→true` REFUTED as a lever — clean same-contention A/B identical (proj
p50 89 vs 85 ms); the apparent first-run win was machine contention. CRDT-ON
ONLY: `LoroModule` gated `if loro_enabled` (`wiring.rs:168`); `loro:false`
SqlOnly desktop unaffected (prod e2e split 26-41 ms @1500 — SLO met). Fix
>M, own workstream: close the four reseed-fallback leaks (ancestor-chain
fetch for orphan-create; fix subscribe_root enqueue race; container-scoped
diff instead of full reseed; scope `TursoBlockQuerySource::snapshot()` to
focus subtree) + keystone invariant "no interactive edit takes mode=full at
N blocks". INC 0 LANDED 2026-07-16 (telemetry + observe-only pin): every
`mode=full` projection event now carries `reason=coldboot\ |
empty_pending_moved_frontier\ | unsettled\ | orphan\ | oversized\ |
sink_fail` (`FullReason` threaded through `emit_ops`,
`loro_sync_controller.rs`); `measure_latency.py` breaks full passes down
per-reason; observe-only keystone oracle `inv-no-steady-reseed-leak`
(`reseed_observer.rs`, tracing-layer on `holon_latency`) attributes
steady-state full reseeds to interactive transitions, flips to enforcing via
`HOLON_PBT_RESEED_ORACLE=enforce`. PER-REASON NON-VACUITY BASELINE (keystone
N, current unfixed tree): ALL FOUR leak reasons UN-OBSERVED — the keystone
aborts at the pre-existing boot-seed journals ingest-data-loss RED
(`inv-blocks-match-ref/block_raw`) BEFORE any interactive transition runs,
so the projection loop never fires `mode=full` at steady state (oracle ran
3× per run, reported `incremental=0 full=0 steady_leaks=0`). Consequence:
none of the four fixes can be keystone-guarded at current N until the
journals RED is lifted; until then they need the wall-clock soak/diag guard
(`HOLON_SOAK_SEED_FILES`) for non-vacuity. **RE-MEASURED 2026-07-17** (the
above baseline is INVALID — it was an artifact of the tick-0 abort, now
lifted by the journals fix + F8 display-placement selection pairing): full
interactive sequences now reach steady state (observed `incremental=1..61`
per sequence). Under `HOLON_PBT_RESEED_ORACLE=enforce HOLON_PBT_FORCE_FULL=1
PROPTEST_CASES=32` (confounding non-reseed reds softened to `warn`), across
89 per-sequence reseed summaries: EVERY `mode=full` pass is `coldboot`
(4–6×/seq, legit boot seed), ALL FOUR leak reasons UN-OBSERVED,
`steady_leaks=0` everywhere, `enforce` did NOT red on the reseed axis. So
the enforce-flip prerequisite is NOT "reasons now fire" — none of the leak
reasons fire at keystone N; flipping enforce would be GREEN but VACUOUS (no
leak to catch). The keystone now actively CONFIRMS the incremental fast path
holds at N (strict improvement over the old "never reached"), but proving it
CATCHES a leak regression still needs the soak/diag guard. Do NOT flip
enforce by default on this basis. (Full table in `reseed_observer.rs`
header.) **SOAK RUNG LANDED 2026-07-17** (`d10768d0`;
`soak_reseed_reproduction` in `general_e2e_composed_pbt.rs`; test-only,
env-gated skip-by-default via `HOLON_SOAK_RESEED_EXPECT=reproduce\ | zero`):
a dedicated large-vault reproduction at 2000 blocks × 3 fixed seeds driving
deterministic `AddPeer→PeerEdit→MergeFromPeer` cycles (93 transitions) STILL
does NOT reproduce ANY of the four leak reasons — `full=0` every seed while
`incremental=10/13/16` (fast path taken on every peer import). Senior
spot-check confirms the negative is TRUSTWORTHY:
`mode=full`/`mode=incremental` share one `holon_latency` emission in
`emit_ops` (`loro_sync_controller.rs:842`, gated `if !ops.is_empty()`), so
live incremental counts + `full=0` = a real negative for the row-71
'ordinary edit leaks to full reseed' scenario (the only uncounted case is a
zero-op full reseed = wasted O(N) snapshot with before==after, which is NOT
this scenario). RE-SCOPE: scale + peer-merge alone does NOT reproduce; the
live-vault leak is concurrency/checkout-race-specific — settle-to-quiescence
serializes what the live app does concurrently (cf.
`crdt-incr-diff-checkout-race`). NEXT PROBE needs a
concurrent-reader-during-import lever, NOT more blocks; OR confirm in the
live CRDT vault whether row-71 is still open after the DiffEvent/incremental
landings. Inc 1-4 CRDT fixes REMAIN UNGUARDED by any automated reproduction
→ do NOT touch `project()` until reproduced. The rung's `zero` mode passes
today = ready post-fix regression guard. SECONDARY FINDINGS (2026-07-17
soak, not yet triaged): (a) 4000-block boot FAILS —
`LoroSyncControllerHandle never resolved` within the 30s poll budget
(full-headless 4000-block org-ingest quiescence > 30s; possible real
scalability signal — rung dropped to 2000); (b) `SutBlockCreate` transition
PANICS at scale — `commit_creation_slot resolves NO creation parent after
3s` (harness robustness / possible large-vault slowness signal).
