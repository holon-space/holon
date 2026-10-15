---
id: 2026-10-15-a-put-into-an-occupied-candidate-place-drops-the-waiting-token
date: 2026-10-15
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A second token put into an occupied candidate place silently replaced the
  waiting one, so a store that had opened stayed open with no token in the net
  and never swapped in.
---

## Bug

Turso's background try opens and its token lands in `StorageCandidate` as a
fact. A write that settled over changed inputs returns the primary store token,
which the executor routes to the same candidate place. The put replaced the
waiting Turso token: Turso stayed open, held by no place, and never swapped in.

Found by lane net-rev2 (holon-space/holon-work#315) as its finding E3, through
the `store_serves` invariant of the write property test. The lane could not
change `exec.rs`, so it skipped `store_serves` for every run in which any
store took the candidate route — 644 of 716 steps — and left the scenario as
`#[ignore = "needs #322 (multi-token candidate place)"]`
(`a_returned_write_token_never_overwrites_a_waiting_candidate`,
`crates/holon-net/tests/net_rev2_storage.rs`).

## Root cause

`Tokens::put` keeps at most one token per (place, subject) and replaces the
content it finds (`crates/holon-net/src/exec.rs:694`). Every place was capacity
1, including candidate places, which take every late or upgrading result with
no staleness check. Two independent tokens then compete for one slot and the
later put wins silently. Ruling REV2-cand.c (ADR 0032 §9.6): a candidate place
holds any number of tokens and its apply transition consumes the oldest first.

## Missing piece

No executor test net had two independent producers writing the same place, so
neither the `exec_cas.rs` PBT nor the reference model could generate the
overwrite; and `Tokens::put` had no invariant that a put never drops a token,
so nothing went red when it did.

## Remedy

A place now declares its capacity (single-token or FIFO), the load
refuses a candidate place that is not FIFO, and a put into a FIFO place
appends. That removes the overwrite and the test's run-wide skip, and the
ignored scenario is green.

The gap is closed at the oracle: the reference model holds a token queue per
row, so the differential check compares whole queues and a dropped token
diverges. `a_put_never_drops_a_waiting_candidate`
(`crates/holon-net/tests/exec_cas.rs`) pins it deterministically — two stale
normalizations wait, oldest first.

Ruling REV2-cand.b also asks that a put into an occupied *single-token* place
refuse loudly rather than replace. Which puts that covers is ruled on #322
(322/E3.a2): only a fact from a transition that does **not** read the place. A
put behind the CAS has seen the held token, so its rewrite is intended and stays
allowed (`ProviderTries`, `Mid`), and so is a fact from a transition that
declares a read arc on the place — the read arc is the net's own CAS, visible in
the net text. A data-class place is exempt because its outputs merge in Loro
(NET-fire.a). The anchor is the place's declared capacity, not the read trace.
`Executor::refuse_blind_overwrite` (`crates/holon-net/src/exec.rs`) panics with
the place, the subject and both token contents;
`a_blind_put_into_an_occupied_single_token_place_refuses` and
`a_fact_rewrite_of_a_read_row_is_allowed`
(`crates/holon-net/tests/exec_cas.rs`) pin both sides.

The first refusal to fire was in production, not in a test net: the write-back
net rewrote `OnDisk` and `Base` by a fact with no read arc on them, so
`READ_DISK`, `ACCEPT_REMOVAL` and `REWRITE_FROM_STORE` each gained the read arc
they rely on (`crates/holon-filesystem/src/writeback_net.rs`). That is the honest
form: a disk observation and a store rewrite each replace the state they have
read. `an_edit_between_the_write_and_the_record_is_applied_over_changed_inputs`
and `the_records_own_row_write_is_not_a_foreign_change`
(`crates/holon-filesystem`) are the tests that caught it. They do not pin the
arcs, though — the exemption alone turns them green — so one test per arc fires
the path the arc protects and refuses without it:
`a_read_disk_whose_hash_differs_rewrites_on_disk` and
`the_rewrite_from_store_remedy_rewrites_an_occupied_base`
(`crates/holon-filesystem/src/writeback_net.rs`). The manual remedies had no
test below the keystone suite before them.
