---
id: 2026-10-12-db-open-failure-drops-earlier-moves
date: 2026-10-12
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  When the database open failed after it had moved a damaged file aside, the error did not
  name that move, so the user could not find the backup.
---

## Bug
Found by the honeymoon2 boot-ladder census (code audit, lane boot-ladder-gaps). Reproduced
red-first by `a_failed_move_keeps_the_earlier_moves_disclosed` in
`crates/holon-turso/tests/db_open_keeps_a_usable_file.rs`
(`target/boot-ladder-gaps-red-db-open.txt`): the open moved a damaged `-tshm` aside, then
failed with "permission denied" on the write-ahead log, and the error did not name the
`-tshm` backup.

## Root cause
`crates/holon-turso/src/db_open.rs` `open` collected its moves in a local `Vec<OpenChange>`
and returned it only on success. Every `?` after a move (the `recover_damaged` call and the
other error exits) dropped the vector.

## Missing piece
No test made the open fail after a move. Every test asserted the moves of a successful open
only.

## Remedy
`open` returns `Result<(Arc<Database>, OpenOutcome), OpenFailed>`. `OpenFailed` carries the
moves made before the failure next to the error, and its text names each move. The boot
error (`lifecycle.rs`) carries this text in its cause chain.
