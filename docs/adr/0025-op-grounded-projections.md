# ADR 0025: Op-Grounded Projections and the Two Intent-less Boundaries

**Status:** RATIFIED (Martin, 2026-07-12) — ROOT ITEM IMPLEMENTED (2026-07-12): per-block
`Remove` deltas are threaded feed→writeback end-to-end. Since 2026-07-24 the routing is
`LiveData::group_by` (stateful: element→last-owning-doc accumulator), so `di.rs` delivers
both feed removals AND cross-doc departures as `OrgRerender::Block { doc, Remove(id) }`
directly to the owning doc, with the id sanctioned (`on_block_changed`); the former
`on_block_removed` reverse lookup is deleted. Since 2026-08-01 the block-driven guard is
ZERO-TOLERANCE (`FileSyncController::veto_ungrounded_removals`): the former
mass-truncation threshold (`max(3, 25%)`) let an unsanctioned removal of a few blocks
through silently, which destroyed the blocks a folder companion authored under an inlined
foreign page root (BugFunnel 2026-08-01). A removal grounded by neither a delivered op nor
a sibling file now refuses the write and quarantines the file whatever its size. Since
2026-08-09 the ingest re-project shares that one grounding assembly and recognises RELOCATION
on store-authority evidence — see the amendment at the end.
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
  reseed). It carries the same veto, grounded by the removal ids the feed accumulated for it
  plus the sibling union; a file whose render drops anything else is quarantined and the batch
  continues past it.

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

## Amendment 2026-08-09 — the ingest boundary grounds on store authority

The external-file boundary above is still intent-less, and its guard is still permanent. What
changed is the EVIDENCE that guard is allowed to use.

Grounding an absence against the ingested file's own re-projection cannot tell a block that
MOVED from one that was DESTROYED. `get_blocks` stops at `Page` boundaries, so a `:Page:`
child of a hand-authored file is legitimately absent from its host's render the moment it
becomes a page doc-root — and was reported as data loss, quarantining the file (BugFunnel
2026-08-09).

The ingest re-project therefore uses the same verdict as the block-driven paths
(`FileSyncController::writeback_drops`): for each absent block, the same authority that
produced the render names the file that owns it now. A block the authority homes elsewhere is
RELOCATED, not lost. This is not op-grounding — no op exists here — it is authority-grounding,
and it is available at this boundary precisely because the render was taken from that
authority. Ingest holds no op, and must still infer nothing from the diff alone.

Unchanged: a block the authority no longer HOLDS (`AbsentOwner::AuthorityLost`) is ungrounded
and refuses; a sibling's bytes cannot rescue it, because that file may have been written
before the loss — only the projection about to be written can, since those are the bytes that
will land on disk. An absent block whose own-file path cannot be derived is UNRESOLVABLE and
refuses under its own disclosure, ahead of the loss verdict. One grounding assembly serves
every boundary, so grounding against a file's own projection alone is unrepresentable.
