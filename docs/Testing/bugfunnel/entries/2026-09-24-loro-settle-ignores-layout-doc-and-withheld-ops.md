---
id: 2026-09-24-loro-settle-ignores-layout-doc-and-withheld-ops
date: 2026-09-24
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  The Loro→SQL settle predicate read only the global doc's watermark, so a
  create committed to the layout doc reported settled while its block_raw row
  was still in flight (and a pass that withheld an owed op reported settled too).
---

## Bug
`loro_suite::loro_create_persists_prod_session` failed at its
"under an existing block" case under parallel load: `block.create` succeeded,
`wait_for_loro_quiescence` returned Ok in ~3 s, and `block_raw` had no row.
Found by gate runs, then measured with 16 concurrent copies: 21% (main), 35%
(savefix chain), 39% (full batch); 0/15 serial on a quiet machine. The savefix
chain made it more frequent because `save_all()` now runs inside the pass,
before the sink write, which widens the in-flight window.

## Root cause
`LoroSyncControllerHandle::is_settled_at(current)` was
`last_synced == global oplog_frontiers && pending_is_empty()`. Two holes:

1. The test's "existing block" parent is a random seeded block. When it lives in
   the device-local LAYOUT doc (`LoroBackend` probes the layout doc first), the
   create commits there. The global frontier does not move, and the pass drains
   `layout_pending` when it starts. From then until the sink write lands, the
   predicate reads settled. The layout watermark (`layout_last_synced`) existed
   but no settle detector read it.
2. A full walk that withheld owed ops (ungrounded creates, armed deletes) still
   advanced `last_synced` inside `emit_ops`. The pass returned
   `Incomplete`, but the predicate reported settled.

Evidence (an instrumented copy of the test, 32 runs under 16-way load, which
recorded which doc moved and polled for the row after the settle):
`DIAG under an existing block: parent=block:todoist-view global_moved=false
layout_moved=true rows_at_settle=0` and then `row ARRIVED 11ms after the lying
settle`. 11 of 11 lying settles had `layout_moved=true`. The rows arrived
11–113 ms later. No `withholding … ungrounded` warning appeared in any run. So
this is a lying settle, not data loss.

## Missing piece
- No settle detector covered the layout doc, and no test committed to the layout
  doc and then checked the settle predicate mid-pass.
- The keystone's composed settle (`converge_signals`) used the same predicate.
  But the keystone alphabet never creates a block under a layout-doc parent, so
  the headless composed rung could not produce this interleaving (COVERAGE).
  The keystone did not catch it. The loro-suite projection-harness rung
  (`LoroProjection` + `MemorySink` apply hook) is the rung that pins it
  deterministically.

## Remedy
- `LoroProjection::is_settled_at(global, layout)` and `is_settled()` check both
  watermarks and the pending queues. The handle exposes only `is_settled()`, so
  a caller cannot forget the layout doc.
- The watermarks advance in `advance_watermarks` after the sink write commits,
  and only when the pass withheld nothing. An owed op now keeps the projection
  visibly unsettled until a later pass pays it.
- Pinned by `loro_suite::loro_projection_settle`: two deterministic tests that
  were red before the fix, red with the fix removed, and green with it.
- A/B stress (8 rounds × 16 copies, `loro_create_persists_prod_session`): 42/128
  failed with the fix removed, 0/128 with it (Fisher two-sided p = 8.0e-15).
