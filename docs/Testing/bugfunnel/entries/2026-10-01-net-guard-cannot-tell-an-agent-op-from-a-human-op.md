---
id: 2026-10-01-net-guard-cannot-tell-an-agent-op-from-a-human-op
date: 2026-10-01
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  `NetGuardOp` carries no op origin, so a net guard cannot tell an agent op from
  a human op, and every confirmable refusal an agent meets is confirmed by the
  agent itself.
---

## Bug
Design gap found by reading, not reproduced: read-only spike SP6 (H-f2, section
2.4 and 8, `scoped-net/sp6-human-agent.md` in the session scratchpad). The user
visible consequence: a soft refusal protects nothing when the agent may
re-dispatch with `confirm_break`; and a presence-based guard would also refuse
the human's own op.

## Root cause
By reading, not reproduced. `NetGuardOp` is `{entity_name, op_name, params,
confirmation}` (`crates/holon/src/api/net_guard.rs:138-143`). The dispatcher
has `origin` in scope but passes only `(entity, op, params)` to
`enforce_net_guard` (`crates/holon/src/api/operation_dispatcher.rs:1232`; the
next gate, `enforce_write_tier`, receives `&origin`). Also, `dense_patch`,
`claim_task`, `complete_task` and `add_subtask` build their own params and
cannot pass `confirm_break`, so for them a confirmable refusal is a hard error.

## Missing piece
No keystone transition drives a confirmable refusal from an agent origin, and no
invariant says who may confirm.

## Remedy
OPEN. Add `origin` to `NetGuardOp` and pass it at the call site; decide who may
confirm (the human, for example through a question block) before adding any
policy that depends on origin.
