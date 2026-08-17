---
id: 2026-07-16-executable-prod-gpui-session-own-weave
date: 2026-07-16
gap: ENVIRONMENT
secondary: PERCEPTION
status: OPEN
summary: >-
  `dismiss_advice` is NOT executable in the prod GPUI session: the op the UI's
  own weave op_button dispatches fails "No provider registered for entity:
  'block'" — while listing Named("block") as available (provider set exists,
  none handles the op); the composed-session gate test
  (advice_live_mcp_gate.rs) proves this exact wire form GREEN, so the advice
  OperationProvider is missing from the app wiring; error message is
  self-contradictory
source_line: 821
---

## Bug

`dismiss_advice` is NOT executable in the prod GPUI session: the op the UI's
own weave op_button dispatches fails "No provider registered for entity:
'block'" — while listing Named("block") as available (provider set exists,
none handles the op); the composed-session gate test
(advice_live_mcp_gate.rs) proves this exact wire form GREEN, so the advice
OperationProvider is missing from the app wiring; error message is
self-contradictory

## Missing piece

gate test runs the composed full_headless session, not the gpui embedder
wiring; no assertion "every op a rendered op_button carries is dispatchable
in THIS session"

## Remedy

ROOT-CAUSED + FIXED 2026-07-17 (base integration `0e005fd1`). Root cause:
`dismiss_advice` (ADR 0021/0022) was implemented ONLY on the Loro block CRUD
provider (`holon-loro::LoroBlockOperations`). The desktop GPUI app defaults
to SqlOnly (loro:false, `wiring.rs:168`), where the block CRUD authority is
`SqlOperationProvider` — which never advertised or handled the op, so the
dispatcher's `available_ops` check for `(entity=block, name=dismiss_advice)`
failed with the (misleadingly entity-scoped) "No provider registered for
entity: block" (`operation_dispatcher.rs:650`). The composed gate is green
because `full_headless` wires the Loro provider. FIX
(`crates/holon/src/core/sql_operation_provider.rs`): `SqlOperationProvider`
now advertises + handles `dismiss_advice` (the SqlOnly twin of the Loro op)
— gated on that edge field being registered so only the `block` provider
claims it. Implemented as a SINGLE per-row `INSERT OR IGNORE` against the
junction's `PRIMARY KEY (anchor_id, lesson_id)`, wrapped in
`db_handle.transaction()` (repo rule: block writes are transactional).
Idempotent (repeat dismiss = PK-collision no-op) and inherently
conflict-free — there is no read-then-write, so concurrent dismissals cannot
lose an update; this is strictly BETTER than the Loro provider, whose
whole-array LWW replace over one meta key CAN clobber two concurrent
dismissals of different lessons. (An earlier draft of this fix used a
non-transactional whole-set capture-then-replace via
`edge_field_replace_sql` — a whole-set LWW with a real lost-update window;
corrected after verifier catch, since wrapping only the replace writes in a
transaction would NOT close the window while the `capture_edges` read stayed
outside the batch.) RED-first by construction (the op literally did not
exist on the SqlOnly provider). Env-gap closed by prod-path test
`crates/holon-integration-tests/tests/advice_dismiss_prod_session_wiring.rs`
(`TestEnvironment::start_app` = the DI wiring GPUI resolves; dispatches
`block.dismiss_advice`, asserts Ok + one persisted `advice_suppressed` row +
idempotency) — GREEN
