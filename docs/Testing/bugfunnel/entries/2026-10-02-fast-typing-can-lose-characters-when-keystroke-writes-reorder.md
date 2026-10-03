---
id: 2026-10-02-fast-typing-can-lose-characters-when-keystroke-writes-reorder
date: 2026-10-02
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  A fast typist in the GPUI editor can lose characters or a just-typed task keyword:
  each keystroke's whole-buffer set_field runs in its own task, and the store accepts
  whichever write lands last, also an older one.
---

## Bug
Found by the headless keystone after its editor twin was made to dispatch
keystrokes fire-and-forget, as GPUI does (admission Inc 0a). Not yet seen by a
person in the live app.

Hand-authored rows `task64-promotion-sqlonly-arm` and
`task64-second-keyword-draw-sqlonly` (SqlOnly leg) went red intermittently:
`inv-task-state-matches-ref` reports `block:promo-sql: expected
task_state=Some("TODO"), actual task_state=Some("")`, and `block_raw` holds
`"T"` / `"TO"` instead of the full typed text. Red logs:
`lane-logs/adm0a3-probe-task64-sql.log`, `lane-logs/adm0a3-probe-task64-2nd.log`
(both rows were green in the full run `lane-logs/adm0a3-ha-after-*.log`).

The same race reproduces on the {Loro + dispatch} leg, the leg GPUI runs in
production (`frontends/gpui/src/di.rs:128` installs no editor-cell registry, so
every keystroke goes through `dispatch_intent`).
`two_instance_composed_pbt edit_on_receiver_concurrent_with_create_on_owner_converges`
fails with `owner converged WITHOUT the receiver-authored text`
(`crates/holon-integration-tests/tests/two_instance_composed_pbt.rs:843`): 1/1 in the
night core run on main `03078ee3`, 1/10 alone at load 7, 6/20 under load 136-215.
Probe run `b2-receiver-click/lane-logs/probe4.log` (temporary order logging, since
reverted) typed `"phone"` on the receiver's `block:c1`:
- the five `set_field` writes start and finish out of order (`probe4.log:119-130`;
  run 15 started 4-1-5-3-2);
- in 20/20 runs the receiver's Loro text equals the write that FINISHED last
  (`b2-receiver-click/lane-logs/analyze4.out`);
- a run fails exactly when the `"c1phone"` write did not finish last (run 2 finished
  1-3-2-5-4 and kept `"c1phon"`).
The click WARN `click_entity: entity never appeared in the resolved tree` occurs
in passing runs too, so it is not the cause.

## Root cause
- Every text change dispatches the VM's whole-buffer `set_field` through
  `services.dispatch_intent` (`frontends/gpui/src/views/editor_view.rs:442`).
- `dispatch_intent` spawns one task per intent
  (`crates/holon-frontend/src/reactive.rs:4275`), so two keystrokes' writes
  have no order between them.
- The store writes `content` and `write_seq` in one unconditional `UPDATE`
  (`crates/holon/src/core/sql_operation_provider.rs:3750-3757`); nothing
  rejects a write whose `write_seq` is older than the row's. `write_seq` is
  read only by the editor to drop stale echoes
  (`EditorViewModel::converge_from_data_sync`,
  `crates/holon-frontend/src/editor_view_model.rs:687`).
- So when the write of `"T"` lands after the write of `"TODO"`, the stored text
  and the parsed task keyword go back to the older buffer.

## Missing piece
The keystone's headless editor awaited each keystroke's dispatch
(`dispatch_intent_sync`), which serialised the writes, so the race could not
occur in the test. The editor now dispatches fire-and-forget
(`crates/holon-frontend/src/headless_editor_mirror.rs`, `vm_commit_edit`).
The two-instance pair reaches the race only by chance (6/20 under load), and its
failure names the convergence, not the lost keystroke.

## Remedy
OPEN. Registered as known red `keystroke-set-field-store-reorder`
(`docs/Testing/KeystoneKnownReds.md`, two patterns: the task-state teeth and the
two-instance teeth); both hand-authored rows are in the `just hand-authored`
default skip list. Owner: admission Inc 1 (the dispatcher admission step orders a
block's local writes, D6.d/D7.a).
