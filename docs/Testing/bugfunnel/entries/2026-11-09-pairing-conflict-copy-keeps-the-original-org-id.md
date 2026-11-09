---
id: 2026-11-09-pairing-conflict-copy-keeps-the-original-org-id
date: 2026-11-09
gap: ORACLE
status: FIXED
summary: >-
  After a pairing, the receiver's conflict copy of a block was written with the
  original block's org `:ID:`, so the write-back refused the whole file with an
  org id collision and only a log line said so.
---

## Bug
Found in the log of a passing run of the two-instance PBT
`a_rebuilt_receiver_pairs_without_a_second_node_per_id`: the lines "org id
collision" and "write-back render ... refused". No assertion failed.

## Root cause
`conflict_copy` (crates/holon-loro/src/device_pairing_op.rs) cloned the
receiver's block and gave it the id `<id>-before-pairing`, but the clone kept
the `ID` property. The org renderer writes `:ID:` from that property, so the
copy and the original both claimed one id in the file, and the render refused
the file. The refusal reached the write-back loop
(crates/holon-orgmode/src/di.rs) as an error that was only logged.

## Missing piece
- Litmus ORACLE: the PBT drove the case (an edited file on a rebuilt
  receiver), but no property said that every block the receiver holds reaches
  its org files after a pair.

## Remedy
The conflict copy drops the original's `ID` carrier, so it renders under its
own id. Every write-back failure that reaches the write-back loop is disclosed
to the user (a file condition naming the file and the reason) and marked
disclosed. The PBT asserts after each pair that every block of the receiver's
store, except those the owner also keeps off disk, is in the receiver's org
files; a fixed case edits every file.
