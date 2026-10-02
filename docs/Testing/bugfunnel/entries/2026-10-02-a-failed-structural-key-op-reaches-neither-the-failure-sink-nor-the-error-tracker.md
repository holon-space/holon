---
id: 2026-10-02-a-failed-structural-key-op-reaches-neither-the-failure-sink-nor-the-error-tracker
date: 2026-10-02
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  A failed join, split or indent from a key only writes a log line: it reaches neither the
  op-failure sink (no toast) nor the error tracker, in GPUI and in the headless twin.
---

## Bug

Found by the fresh-context verifier of admission Inc 0a (2026-10-02, `adm0a-verify/verify.md`,
finding F4; lane state `lane-logs/adm0a2-state.md` section "ROUND 3e"). The new
`handle_keystroke` doc in the headless editor says a failed key op "surfaces through the op-failure
sink". That is true for char keys and false for structural keys.

## Root cause

Char keys go through `dispatch_intent`, whose Err arm calls `surface_op_failure`
(`crates/holon-frontend/src/reactive.rs:4298`; the function is at `:5398` and does
`error_tracker().record_error()`, a log line and the sink call). Structural keys go through
`dispatch_structural` (`crates/holon-frontend/src/headless_editor_mirror.rs:335`, GPUI twin
`frontends/gpui/src/views/editor_view.rs:1203`) into `dispatch_intent_chain`
(`reactive.rs:5622`), whose Err arm only calls `tracing::error!` (`:5636-5640`). The worker
(`frontends/holon-worker/src/lib.rs:800`) uses the same function. A failed join, split or indent
is therefore visible only to the log-capture layer.

## Missing piece

`inv-no-observed-errors` reads the captured log, so a log line alone satisfies the oracle's "error
is disclosed" idea. No invariant demands that each failed op also reaches the user-visible sink and
the tracker, and no test installs a sink around a failing chain. Earlier entries noticed the same
silence as a side remark
(`2026-08-07-rapid-enter-silently-loses-block-splits`) but none isolated this defect.

## Remedy

OPEN, in the Inc 0a round 3e code lane. Rung: a frontend_suite test that installs a sink with
`set_op_failure_sink`, runs `dispatch_intent_chain` with a `join_block` on a missing id, waits for
`journal.open_chains() == 0`, and asserts exactly one sink message and the tracker count +1
(today 0 and +0). Fix: `dispatch_intent_chain` calls `surface_op_failure` on Err and keeps its
context string, so the known-red pattern anchored on `dispatch_intent_chain: block.join_block
failed` still matches.
