# ADR 0025: Op-Grounded Projections and the Two Intent-less Boundaries

**Status:** RATIFIED (Martin, 2026-07-12)
**Context:** first-principles session on block loss, 2026-07-12; empirical basis: the seven
block-loss classes found and fixed 2026-07-10..12.

## Problem

Every block loss found in the 2026-07 bug funnel was a projection defect between
representations (org disk ⇄ Loro ⇄ Turso ⇄ render), never a storage failure. The recurring
root: a boundary received a STATE DELTA and had to re-derive the INTENT behind it —
ambiguously. "Block absent" cannot distinguish *deleted on purpose* from *lost in transit*
once the originating operation has been discarded. Point guards (e.g. the row-28 ingest-loss
writeback guard) bound the damage but do not remove the ambiguity.

Empirical taxonomy (all fixed, all in this family):
1. Intent reconstruction from state diffs (delete-wins DiffEvent routing; writeback-guard dilemma).
2. Private projection bases drifting from sink truth (reseed drift; sentinel wedge; non-transitive orphan gate).
3. Types narrower than the payload (BlockContent lacked Image).
4. Round-trip through a lossier format with an *implicit* middle between "round-trips" and
   "declared internal" (marks, task keywords, Page-ness).
5. Two authorities for one fact resolved by arrival order (companion vs page-file, scan-order LWW).

## Decision

**Ops are the only propagation currency inside the system.** Every mutation is an operation
with provenance (ADR 0024 + C2a stamping). Projections and writebacks consume ops (or deltas
derived 1:1 from ops); a projection-derived DELETE that cannot be grounded in a known op is
loss by definition and MUST fail loud.

**Exactly two boundaries are irreducibly intent-less**, and they are the permanent home of
conformance tripwires (not patches):
- **Remote CRDT merges.** Peer edits arrive as Loro state. Intent is *recoverable* only through
  the consolidator's CRDT-delta → domain-op mapping; the intent-divergence check
  (`agrees_with_ops`) is that mapping's conformance gate and stays forever.
- **External file edits.** Ingest can only diff old-parse vs new-parse; intent inference is
  irreducible. The ingest-loss guard (row 28) stays forever and is the honest bound on this
  boundary's damage.

**Writeback becomes op-grounded:**
- `on_block_changed` receives per-block deltas; a block absent from the render whose id is in
  the delta's `Remove` set is a *sanctioned* removal; absent WITHOUT a grounding delta = veto +
  quarantine (SurvivingProjection union admits blocks that legitimately moved to sibling files,
  e.g. companion de-inline).
- `re_render_all_tracked` is reclassified as a RECOVERY path (state-driven by nature, like
  reseed). Recovery paths carry the guard as tripwire. Follow-up (not this increment): feed it
  from the C2b history relation so even recovery can ground removals in recorded ops.

**Standing disciplines** (established by this cycle's fixes, now doctrine):
- Diff bases are never private: projections diff against SINK TRUTH; in-memory bases are
  caches that must re-converge on every reseed.
- Single ownership for every fact; a second describer may never demote/rewrite (is_page() authority).
- Every block field either round-trips through org or is DECLARED internal
  (INTERNAL_PROPS / `_`-prefix); the implicit middle is forbidden.
- Representation types must carry the full payload (parse, don't validate; no
  narrower-than-domain enums).

## Consequences

- The block-driven writeback guard (B1') is implemented as op-grounding, not survival-checking:
  no false quarantines of user deletions, structural loss impossible on the op path.
- Guards at the two intent-less boundaries are permanent architecture, exempt from
  "delete defensive code" sweeps.
- Future subsystems (connectors/twins, rules) inherit the rule: emit ops, never mutate state
  behind the projection's back.
