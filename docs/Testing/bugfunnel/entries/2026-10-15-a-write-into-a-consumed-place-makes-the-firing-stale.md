---
id: 2026-10-15-a-write-into-a-consumed-place-makes-the-firing-stale
date: 2026-10-15
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  An outside write into a place a firing had already consumed made that firing
  stale, so its outputs were withheld and the primary store token was lost.
---

## Bug

A write fires `ApplyWrite`, which consumes the head of `WriteQueue` and takes
the `Storage` token. A second write is enqueued while the first is in flight,
so `WriteQueue`'s version moves. When the first write settles, the executor
sees a changed input, settles the occurrence `OverChangedInputs` and withholds
its `Storage` output: the primary store token is gone.

Found by lane net-rev2 (holon-space/holon-work#315) as its finding D1, while
building the revision-2 storage net on the real executor. The lane could not
fix the executor (another lane owned `exec.rs`), so it worked around the loss
by declaring a candidate place on `Storage`, which re-installs the token
through `ApplyStorageCandidate`.

## Root cause

`Executor::stale` compared every row of the firing's read trace against the
current marking (`crates/holon-net/src/exec.rs:2359`). The trace holds the
consumed places too, because the bind reads them to build the input. A firing
HOLDS what it consumes — `start` takes the token out of the place
(`exec.rs:2229`) — so a later write lands in an empty slot and only bumps that
slot's version. Nothing the firing relies on changed, yet the version
comparison reported the firing stale. Ruling REV11-running.a: only READ inputs
can be stale.

## Missing piece

No executor test net had a transition that *consumes* a place together with an
outside write into that place while the firing was in flight. `exec_cas.rs`
covered read arcs only, so its PBT could not generate the interaction, and the
reference model in `exec_support/model.rs` carried the same rule, so the
differential check agreed with the defect.

## Remedy

`stale` skips the rows of the attempt's taken tokens, in the executor and in
the reference model. Pinned by `a_consumed_input_is_never_stale` in
`crates/holon-net/tests/exec_cas.rs`, which is red for this reason before the
fix (log `target/exec-tokens-red.log`, lane exec-tokens, #322 E1).
