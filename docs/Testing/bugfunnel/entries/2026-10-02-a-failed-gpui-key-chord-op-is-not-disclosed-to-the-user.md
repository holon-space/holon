---
id: 2026-10-02-a-failed-gpui-key-chord-op-is-not-disclosed-to-the-user
date: 2026-10-02
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  A GPUI KeyChord that executes an operation dispatches outside the dispatch journal and the
  op-failure sink, so a failed chord op shows no toast and the headless drain cannot see it in flight.
---

## Bug

Found by the fresh-context verifier of admission Inc 0a (2026-10-02, `adm0a-verify/verify.md`,
finding F1, which refuted the claim "no un-journaled dispatch path"; lane state
`lane-logs/adm0a2-state.md` section "ROUND 3e", item c). This entry covers the GPUI part. The two
reactive.rs spawns of the same finding (reap, create-page-and-navigate) are fixed in the same lane
and are reachable from the keystone; this part is not.

## Root cause

`on_key_down` in `frontends/gpui/src/lib.rs` (call at `:1594`) handles `InputAction::ExecuteOperation`
by calling `holon_frontend::operations::dispatch_operation` (`crates/holon-frontend/src/operations.rs:111`).
That function spawns on the spawner, records no journal entry, and on Err only calls
`error_tracker().record_error()` and `tracing::error!` (`operations.rs:150-154`): no
`surface_op_failure`, so no sink and no toast. `services.dispatch_intent` (journal plus sink) is
in scope at that point. The function is also called from three waterui builders
(`frontends/waterui/src/render/builders/{selectable,source_block,editable_text}.rs`), which have the same gap.

## Missing piece

The chord path exists only in the GPUI window handler, which the headless keystone never runs. The
windowed GPUI PBT does not inject a failing op under a chord, and the drain check
(`drain_dispatches` in `crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs:184`) counts
only journaled dispatches.

## Remedy

OPEN. Recommended fix: the chord handler calls `services.dispatch_intent(OperationIntent::new(..))`
and `operations::dispatch_operation` is deleted after the waterui callers move too (or those
callers are listed as out of scope by Martin). Rung: a windowed GPUI PBT or test that presses a
chord whose op fails and asserts one sink message; the shared `dispatch_intent` path is already
covered headless once the chord uses it.
