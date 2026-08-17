---
id: 2026-07-17-two-real-sut-integration-tests-red
date: 2026-07-17
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Two `sql_loro_slice` real-SUT integration tests RED
  (`..._catches_task_state_divergence`,
  `..._task_state_coherent_across_stores`) after
  `inv-task-state-storage-coherence` was re-anchored to the reference (F4):
  the invariant's `Needs` now lists `ref_present: [RefTaskState]` and its body
  reads `ref_.task_state_of(id)`, but the two slice tests still passed
  `&CapMap::new()` as the ref, so selection deselected the invariant — the
  coherence test failed its "must be selected" guard, and the divergence-catch
  test found `failures=[]` because the (deselected) oracle never ran. NOT a
  broken oracle (the `composed/` fixture-level catch test
  `task_state_coherence_catches_sql_loro_divergence` is green) and NOT
  spurious store convergence (dumped state at the assert: SQL=`Some("TODO")`,
  Loro=`Some("DONE")` — genuinely divergent). Pure harness drift: the F4
  interface change updated the fixture tests but not the real-SUT slice tests.
source_line: 1001
---

## Bug

Two `sql_loro_slice` real-SUT integration tests RED
(`..._catches_task_state_divergence`,
`..._task_state_coherent_across_stores`) after
`inv-task-state-storage-coherence` was re-anchored to the reference (F4):
the invariant's `Needs` now lists `ref_present: [RefTaskState]` and its body
reads `ref_.task_state_of(id)`, but the two slice tests still passed
`&CapMap::new()` as the ref, so selection deselected the invariant — the
coherence test failed its "must be selected" guard, and the divergence-catch
test found `failures=[]` because the (deselected) oracle never ran. NOT a
broken oracle (the `composed/` fixture-level catch test
`task_state_coherence_catches_sql_loro_divergence` is green) and NOT
spurious store convergence (dumped state at the assert: SQL=`Some("TODO")`,
Loro=`Some("DONE")` — genuinely divergent). Pure harness drift: the F4
interface change updated the fixture tests but not the real-SUT slice tests.

## Missing piece

when the invariant gained a ref selection-dependency, only the `composed/`
fixture callers were migrated; the `sql_loro_slice` real-SUT callers still
supplied an empty ref CapMap, so the divergence detection silently
deselected instead of running

## Remedy

FIXED — both slice tests now seed a `RefTaskState` via `ref_task_state(...)`
(coherent: canonical TODO/DONE; catch: ref=TODO so Loro's DONE diverges).
Teeth preserved: with SQL≠Loro no ref value can satisfy both, so the catch
fires for any ref; dumped SQL/Loro reads confirm real divergence.
