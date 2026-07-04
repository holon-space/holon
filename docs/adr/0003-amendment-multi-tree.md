# ADR 0003 Amendment: Per-container LoroTrees + stitching layer (multi-tree)

**Status:** Accepted (2026-07-21)
**Deciders:** Martin
**Amends:** ADR 0003 (all-in-LoroTree architecture) — reopens the single-global-tree decision
**Amended-by pointer:** ADR 0003 §Amendment (top-of-file note)
**Ratified by:** ADR 0028 (Sharing as Policy Overlay over Aligned Containers), §5 / §7

## Why this note exists

ADR 0028 formally reopened ADR 0003 for the multi-container shape (0028 §Amends).
Rather than let two accepted ADRs silently contradict each other, this note
reconciles them: it records precisely *what* of ADR 0003 the sharing overlay
keeps, what it reopens, and the two costs the change accepts. ADR 0003 remains
correct as the *intra-container* substrate decision; only its
*single-global-tree* framing is superseded.

## Reopen, don't silently contradict

ADR 0003 chose **one global LoroTree in a single LoroDoc**, and explicitly
listed `LoroDocumentStore` per-file HashMap as *eliminated* — "replaced by
single global LoroDoc" (ADR 0003:74).

ADR 0028 §5 (S6 ruling) reintroduces the multi-doc shape: **per-container
LoroTrees plus our stitching layer replace the single global tree.** This is a
reopening, not a repudiation:

- **LoroTree stays the intra-container substrate.** It is kept precisely for the
  properties ADR 0003 chose it for — fractional-index ordering, native
  cyclic-move rejection, convergent movable-tree merges (ADR 0028 §5). These are
  the routine device-sync workload (concurrent moves within one container), and
  they remain LoroTree's job.
- **Only the *global* scope is superseded.** Where ADR 0003 had one tree for the
  whole vault, there are now many trees — one per container — knitted by a
  stitching layer. Crossings between containers are **delete-in-source +
  create-in-target pairs bracketed by the H2 crossing log** (ADR 0028 §5); a
  LoroTree `mov` never spans containers, so doc-scoped `TreeID`s stay harmless.

Net: ADR 0003's mechanism is intact *inside* a container; the boundary between
containers is new machinery that ADR 0003 did not contemplate.

## Casualty 1 — unified undo narrows

ADR 0003 sold "unified undo across structure + content" as a headline benefit
(ADR 0003:31, "Unified undo … is more intuitive — validated by spike"; ADR
0003:84, "Unified undo across structure + content"). Loro's UndoManager is
**per-doc**, so once the vault is many docs, a single undo can no longer span
the whole vault (ADR 0028 §7):

- **Within-container undo is unchanged** — still native Loro undo, and it is the
  **90% case** (ADR 0028 §7).
- **Cross-container undo becomes our stitched abstraction.** Undoing a crossing
  is the **inverse crossing routed back through the same H2 log** (see the ADR
  0028 core-machinery plan, Increment 4). It is "best feasible," not free: the
  inverse must be *designed*, because Loro will not give it to us for nothing.

**Concrete demonstration this is real, not theoretical.** The 2026-07-21
sharing-track dogfood found that a per-doc undo of a crossing move reverts a
*concurrent peer's unrelated reparent* on merge — the CRDT stays convergent, yet
a concurrent structural edit is silently lost (BugFunnel.md, 2026-07-21 ORACLE
row; canary-pinned as `move_storm_convergence::undo_across_crossing`, verified on
loro 1.11.1 and inherited by fork 1.13.7). This is the empirical proof that
cross-container inverse-crossings must be an explicit, oracle-guarded design and
cannot be assumed to fall out of Loro's per-doc undo.

## Casualty 2 — base/epoch multiplication

One global doc had one CRDT base and one epoch line. Per-container docs multiply
both: each container carries its own base/epoch under **invariant 10**, applied
per-doc rather than once for the vault (ADR 0028 §7). This multiplication is
**bounded by A7's disjointness cut** — selectors may not nest or overlap (ADR
0028 §4 / §Amendments; A7 forbids nested/overlapping shares in v1), so the number
of distinct container epochs cannot fragment without bound.

## The permanent verification tax

The multi-tree shape is not a one-time migration cost; it installs two standing
obligations that ADR 0028 §7 accepts eyes-open, and this note records so future
work does not treat them as removable:

- **C3 classification forever** — every future structural op must declare its
  boundary behavior (fail-closed classification registry); an unclassified op
  cannot ship.
- **C2 alignment guarded forever** — the directional alignment invariant
  (effective audience never over-approximates policy audience) is a keystone
  oracle that must stay green for the life of the sharing machinery.

## References

- ADR 0003 — All-in-LoroTree Architecture (the decision this note amends)
- ADR 0028 — Sharing as Policy Overlay over Aligned Containers (§5 substrate
  ruling, §7 costs accepted; §Amends reopens ADR 0003)
- `docs/Testing/BugFunnel.md` — 2026-07-21 ORACLE row (F-undo /
  `move_storm_convergence::undo_across_crossing` canary): the landed evidence for
  Casualty 1
