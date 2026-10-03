---
id: 2026-10-03-receiver-click-misses-block-and-drops-the-edit
date: 2026-10-03
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  In the two-instance pair, the receiver's click on block:c1 finds no row within 2 s and degrades
  to a bare focus, so the text typed next never lands and the pair converges without it.
---

## Bug

`two_instance_composed_pbt edit_on_receiver_concurrent_with_create_on_owner_converges` fails with
`owner converged WITHOUT the receiver-authored text`
(`crates/holon-integration-tests/tests/two_instance_composed_pbt.rs:843`). Seen 1/1 in the full
core nextest run on main `03078ee3` (night triage `main-reds/full-1.log:4571`) and 1/10 alone at
load 7 (`main-reds/iso/A-7.log:894`). Found by the reds triage of the night of 2026-10-03.

## Root cause

Not found. Every failure carries `WARN click_entity: entity never appeared in the resolved tree ...
entity_id=block:c1 deadline_ms=2000`; no pass (0/9) does. After that WARN the click binds no intent,
so the receiver's `Type` is a no-op. Whether block:c1 reaches the receiver's resolved tree late or
never is not measured.

## Missing piece

The click's degraded path is disclosed only by a WARN; nothing in the pair fails at the click
itself, so the failure surfaces one step later as a convergence teeth assertion.

## Remedy

Open. Next: measure when block:c1 appears in the receiver's resolved tree relative to the 2 s
click deadline.
