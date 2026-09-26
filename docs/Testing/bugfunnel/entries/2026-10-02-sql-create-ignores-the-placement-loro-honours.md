---
id: 2026-10-02-sql-create-ignores-the-placement-loro-honours
date: 2026-10-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  On the SQL write authority a `block.create` always appended the new block
  as the last child and ignored `after_block_id`; Loro placed it where the
  params asked. A dense_patch row inserted before its siblings read back last.
---

## Bug
Found by code review in the Inc 6 verifier pass (lane decision Inc 6),
then reproduced by new tests. Same drift class as the creation-slot
SqlOnly-vs-Loro drift:
file:///private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bd16c5ee-2f72-4275-9dd2-8c49b90a243f/scratchpad/sql-loro-drift-design.md

Red, before the fix: `lane-logs/inc6r7b-red-engine.log`
(`a_new_row_at_the_front_reads_back_first` `[sql]`: "new row \"Front\": it
follows Some(\"block:pp0-r1\") among its siblings, not None"; the SQL
generated dense-edit test the same) and `lane-logs/inc6r7b-red-hand.log`
(`dense-create-at-every-place-on-the-sql-authority`).

## Root cause
Each leg decided placement itself. `OrderedBlockCrud`'s `create` arm
(`crates/holon-app/src/ordered_block_crud.rs`) and `SqlBlockOperations::create`
(`crates/holon/src/core/sql_block_operations.rs`) minted a key after the last
sibling and never read the anchor; the Loro leg
(`crates/holon-loro/src/loro_block_operations.rs`) parsed `after_block_id` /
`after` with its own closure.

## Missing piece
No test created a block anywhere but last on the SQL authority: the keystone
had only `AppendChild`, and the dense_patch engine tests ran on Loro only.

## Remedy
One rule, `holon_api::ChildPlacement::of_create`
(`crates/holon-api/src/entity.rs:1071`; absent = last, `Null` = first, an id =
after it, `after_block_id` and `after` must agree). All three legs read it;
the SQL legs mint through `SqlBlockOperations::mint_placed_key`
(`sql_block_operations.rs:218`). Coverage: keystone `CreateChild { place }`,
both hand-authored cases, the engine generated test on both legs. Teeth:
`lane-logs/inc6r7b-teeth.log` (rule sabotaged: both legs red together).
