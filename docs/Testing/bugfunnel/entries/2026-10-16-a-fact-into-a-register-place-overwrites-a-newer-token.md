---
id: 2026-10-16-a-fact-into-a-register-place-overwrites-a-newer-token
date: 2026-10-16
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A fact put replaced the held token without comparing versions, so a firing
  that read a stale file state overwrote the newer OnDisk token with its own.
---

## Bug

`ReadDisk` reads the file, hashes it, and puts the hash as a fact into
`OnDisk`. The file is deleted while that read is in flight, the controller
records `OnDisk{hash: absent}`, and the completing firing then writes its own
hash over it: `OnDisk` holds `h2` for a file that no longer exists, so
`CatchUp` writes the store content back to a path the user removed.

Found by the r4 verifier of lane exec-tokens (holon-space/holon-work#322,
finding F3) while reviewing the register semantics of ruling 322/E3.b, not by
a test.

## Root cause

A fact put was unconditional: `Executor::record` put every fact before the
staleness check and the stale branch never rolled one back. `Single` places
refuse such a put loudly (322/E3.a), `Register` places accept it (322/E3.b) —
and "accept" meant "replace", not "replace if the row still holds what the
firing read".

## Missing piece

No test net wrote a register place from a firing whose read of that place had
moved meanwhile. `exec_cas.rs` covered the declaration cases only, and the
write-back tests drove `ReadDisk` without an interleaved outside write, so
neither could produce the lost update. The reference model in
`exec_support/model.rs` carried the same unconditional put, so the
differential check agreed with the defect.

## Remedy

Ruling 322/E3.c: a fact into a single-token row its own firing READ is a
compare-and-set against the token it read. The row having moved, the fact is
withheld, the newer token wins and the occurrence settles
`OverChangedInputs` with the withheld fact as a disclosed route; the
staleness check therefore runs before any fact is put. Pinned by
`a_read_disk_fact_loses_to_the_absent_token_written_while_it_ran`
(`crates/holon-filesystem/src/writeback_net.rs`) and by
`a_fact_into_a_read_row_that_moved_loses_to_the_newer_token`
(`crates/holon-net/tests/exec_cas.rs`), both red for this reason before the
fix (log `target/exec-tokens-r5-red.log`, lane exec-tokens, #322).
