---
id: 2026-10-01-reaper-deletes-agent-work-on-an-empty-newborn
date: 2026-10-01
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  The untouched-newborn reaper deletes on `content == ""` only, so an agent's
  property set or move on an empty newborn is lost, and a child added under it
  is deleted silently on the SQL leg while the Loro leg refuses with an error
  shown to the human.
---

## Bug
Found by reading, not reproduced: the read-only spikes SP6 (section 4) and SP4
(section 3) in the session scratchpad (`scoped-net/sp6-human-agent.md`,
`scoped-net/sp4-newborn-census.md`). Scenario: the human's caret is in an empty
creation-slot newborn N; an agent acts on N; the human then moves focus.

## Root cause
By reading, not reproduced. `reap_untouched_newborns`
(`crates/holon-frontend/src/reactive.rs:3249`) deletes every registered newborn
except the focused one, with `OpOrigin::Rule`, unless `observed_content` is
non-empty. The licence is `content.is_empty()`
(`crates/holon-frontend/src/creation_slot.rs:103-105`). `observed_content`
(`reactive.rs:3283`) returns `None` when no watcher has seen the row, and then
the delete goes out anyway. Cases:
- property set (`task_state`, `assigned-to`, `claim_task`) or `move_block` on N:
  content stays empty, N and the agent's write are deleted, the agent got
  success (deterministic, no timing).
- child created under N: the Loro leg refuses the non-leaf delete
  (`crates/holon-loro/src/loro_block_operations.rs`) and `surface_op_failure`
  shows an error to the human; the SQL leg purges the subtree and the child
  vanishes silently (`crates/holon/src/core/sql_operation_provider.rs`).
- `set_field(content)` inside the CDC delivery lag: timing race.
The doc comment on `reap_is_licensed_by_store` claims MCP writes also save the
block; that holds for `content` only. Rule-origin deletes never enter the undo
stack, so the loss cannot be undone.

## Missing piece
No keystone transition writes a non-content field, a move, or a child on a
focused newborn from an agent origin before a focus change.

## Remedy
OPEN. The planned materialize design removes the empty newborn and with it the
reaper. Until then add the keystone transition (agent property write on the
focused newborn, then blur) and make it go red before the reaper is deleted.
