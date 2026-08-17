---
id: 2026-07-17-keystone-red-after-born-equal-create
date: 2026-07-17
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Keystone RED `inv-history-no-phantom-rows/block_history`: after a born-equal
  create-then-delete the SUT's legitimate append-only `block_history` create
  row for `block:gen-N` was flagged as a false PHANTOM ("N block id(s)
  recorded in block_history are unknown to the reference"). `gen-N` ids are
  minted ONLY by the test transition `CreateBlockUnderFocus`'s born-equal arm
  (`create_block_under_focus.rs:125`, identity id on both oracle and SUT); a
  later `DeleteBackward`-at-cursor-0 join removes the block from the oracle's
  live set. Discovered by the keystone itself during the reseed non-vacuity
  baseline run
source_line: 812
---

## Bug

Keystone RED `inv-history-no-phantom-rows/block_history`: after a born-equal
create-then-delete the SUT's legitimate append-only `block_history` create
row for `block:gen-N` was flagged as a false PHANTOM ("N block id(s)
recorded in block_history are unknown to the reference"). `gen-N` ids are
minted ONLY by the test transition `CreateBlockUnderFocus`'s born-equal arm
(`create_block_under_focus.rs:125`, identity id on both oracle and SUT); a
later `DeleteBackward`-at-cursor-0 join removes the block from the oracle's
live set. Discovered by the keystone itself during the reseed non-vacuity
baseline run

## Missing piece

the oracle's "ever created" anchor (`history_ever_created`,
`harness.rs:261`) is derived solely from the synthetic→real reconcile map,
and `is_composed_minted_synthetic_id` (`harness.rs:96`) recognized only
`block::split-`/`block:ref-doc-`/`block::create-` — NOT `block:gen-`. So
born-equal `gen-N` never entered the reconcile map, hence never the "ever
created" universe; while live it was covered by `all_block_ids`, but the
moment it was deleted the append-only SUT history row lost its reference
anchor and read as phantom. NOT a prod bug — SUT recorded a real create
correctly; the reference universe was structurally incomplete (mirrors the
prior "journals RED" oracle-asymmetry precedent)

## Remedy

FIXED 2026-07-17. Added ` | | id.as_str().starts_with("block:gen-")` to
`is_composed_minted_synthetic_id` (`harness.rs`) so the born-equal self-map
loop (`harness.rs:542`) retains live `gen-N` in the reconcile map (identity,
exactly like `ref-doc-` born-equal doc pages), and thus in
`history_ever_created`, keeping the create row covered after a later
delete/join. Invariants preserved: at create tick `gen-N ∈ after` →
partitions born_equal (not synthetic); already filtered from `real_new`
(`harness.rs:566`) so the `synthetic.len()==real_new.len()` 1:1 assert stays
balanced; `min_op_groups`' `k != v` filter (`harness.rs:268`) excludes the
self-map.
