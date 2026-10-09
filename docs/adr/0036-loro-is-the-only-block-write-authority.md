# ADR 0036 — Loro is the only block write authority; the projection is the only block-SQL writer

**Status:** Accepted 2026-10-09 (Martin: D-write-path.a; M5 = D-eb-path).
Implementation: the M5 plan (`EditorTextOnLoroText.md`, linked from the
vault topic `write-path-unification-ruling`). Not yet built: see
[Model.md](../Architecture/Model.md) invariants 4 and 18.
**Deciders:** Martin
**Relates to:**
[ADR 0030](0030-birth-atomicity-authority-and-mirror-contract.md) — a birth
fires in one authority; this ADR names that authority for blocks in every
wiring.
[WritePathUnification-Options-2026-07-17.md](../Plans/WritePathUnification-Options-2026-07-17.md)
— the options this ruling closes.
D-link-id.b — every Loro mark writer resolves a link name to a page id
through one shared resolver.

## Problem

Block-write semantics had two implementations: the Loro authority
(`LoroBlockOperations`, `BlockCellRegistry`) and the SQL authority
(`SqlOperationProvider` block CRUD, `SqlBlockOperations`). Symmetry between
them was kept by hand. The bug ledger shows the cost: a classification of
every entry on 2026-10-09 found 42 bugs that cannot occur with one write
path, and 18 of them reproduce with the CRDT layer ON. So the class is "two
writers", not "two modes": the SQL implementation also runs in Full mode
(planners, ingest legs, direct SQL writers), and a write that SQL gets but
Loro does not is overwritten or blanked by the projection later.

Refusing `crdt.enabled = false` alone therefore does not remove the class.
Deleting the second writer does.

## Decision

**D1 — Loro is the only block write authority, in every wiring.** Full and
SqlOnly run the same in-memory Loro document, cell registry, undo and op
code. They differ only in durability: Full persists a Loro snapshot; SqlOnly
persists nothing of Loro and rebuilds the in-memory document from SQL at
boot. Non-block entities (integrations, sidecar tables) keep their SQL
authority.

**D2 — The Loro→SQL projection is the only writer of the block SQL tables**
(`block_raw` and its junctions). `SqlOperationProvider` block CRUD is the
projection's sink and nothing else. Ingest, `place_all`, link-resolution
rewrites and every other direct block-SQL writer route through the Loro
authority. This is part of M5, not a later step: it removes the bypass
writers that D1 alone leaves.

**D3 — An op that decides reads the write authority, never the
projection.** Guards, structural ops, planners and read-backs decide on what
the authority holds (`WriteAuthorityReads`, the authority's block reads).
The projection lags the authority, so a decision on it is a decision on an
old tree. A projection read is advisory only: it may narrow what the UI
offers, never license or refuse a write.

**D4 — The SqlOnly flip waits for its gates.** SqlOnly moves onto the Loro
authority only after (a) the open Loro-leg field losses are fixed, because
SqlOnly escapes them today, (b) every decider reads the authority (D3), and
(c) the projection-latency SLO (p95 interaction→projection-visible < 200 ms,
release) is a landing gate that runs in both wirings, because every write
now pays the projection pass.

## Consequences

- One write path: a block-op feature is implemented once. The archlint
  `sole_block_writer` rule shrinks to the projection sink.
- The keystone's SqlOnly-versus-Full arm stops comparing two writers. It
  then checks only the durability switch (boot from snapshot versus seed from
  SQL).
- SqlOnly users inherit the Loro-path costs: projection lag for readers,
  projection latency per write, and Loro-leg defects. D4 makes those gates,
  not follow-ups.
- Sharing and pairing stay unavailable in SqlOnly (disclosed): its Loro
  history is rebuilt each boot with a fresh peer id.
- The consolidator handover of Model.md invariant 10 becomes "rebuild the
  snapshot from SQL", because both wirings now have the same consolidator.
