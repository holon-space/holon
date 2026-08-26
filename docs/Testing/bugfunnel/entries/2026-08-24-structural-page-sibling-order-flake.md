---
id: 2026-08-24-structural-page-sibling-order-flake
date: 2026-08-24
gap: ORACLE
secondary: ENVIRONMENT
status: UNCLASSIFIED
summary: >-
  A structural-page keystone case showed sibling-order divergence from the
  reference. COVERAGE is excluded — the interaction WAS generated, that is
  how the divergence was observed. The KnownReds registry's one matching
  family (`bulk-add-sibling-order-under-journals`) anchors its Match pattern
  to `block:journals`, so a same-mechanism recurrence on a non-journals
  structural page would not classify as known — an ORACLE (registry
  classification) gap, not a generation gap. A/B rerun (both arms green)
  clears the landing lane; which family, if any, this sighting belongs to is
  undiagnosed.
---

## Bug

A keystone case over a structural page showed the rendered sibling order of
some block's children diverging from the reference model's expected sibling
order on 2026-08-24. The raw log did not survive a session restart; what is
recorded here is the orchestrator's adjudication note only (signature
description, method, verdict, date) — no page/block identity, no before/after
ordering.

**Correction of the original filing.** The first draft framed this as a
plausible settle-timing race and found no prior entry. Two established
sibling-order families exist and were not checked:

1. `docs/Testing/bugfunnel/entries/2026-07-10-org-writeback-writes-siblings-non-sort.md`
   — FIXED 2026-07-10. Org writeback wrote siblings in non-`sort_key` order
   because the incremental writeback cache's cheap content-only-edit path
   compared only `parent_id`/tags, so a same-parent reorder was invisible and
   the file kept stale pre-reorder order forever. This was a real,
   deterministic cache-staleness bug, not a timing race, and the fix
   (`reorder_within_parent_takes_full_reseed`) is locked by a test.
2. `docs/Testing/KeystoneKnownReds.md:189`, `bulk-add-sibling-order-under-journals`
   — `known-red`, described as **DETERMINISTIC — 3 transitions**
   (`SplitBlock` → `BulkExternalAdd` → `UndoLastMutation`), reproduced
   byte-identically both from the sweep and from a hand-authored replay. Its
   documented mechanism: `bulk_add_blocks` canonically re-sequences the whole
   tree, but `rematerialize_file_ingested`'s content-block loop restores
   blocks verbatim from `pre_restore`, carrying post-canonicalisation
   sequences into a snapshot whose other children hold the older ones — a
   real reconciliation gap, root-caused as part of the `org-blocks-ref-diverge`
   family, not a race. It was independently confirmed pre-existing at
   `843c5ce0` by a base-rev replay.

Neither of these is a nondeterministic timing artifact — both are real,
reproducible ordering defects at the org-writeback/ref-reconcile seam. That
directly contradicts the "settle-timing race" framing this entry originally
asserted for an unrelated-looking signature with no case detail to check it
against.

## Classification

**COVERAGE is excluded by its own litmus**: "is there a transition sequence
in the current catalog+wiring that reaches this state?" — yes, that is
literally how the divergence was observed; the keystone generated the
interaction and an invariant flagged it, so generation is not the gap.

**ORACLE (primary).** Both established sibling-order families are real,
already-diagnosed defects, not generation or detection failures — the
open question is whether the classifier RECOGNIZES a recurrence as known. The
one registered pattern, `bulk-add-sibling-order-under-journals`
(`KeystoneKnownReds.md:189`), anchors its `Match pattern` to `block:journals`
specifically; a same-mechanism recurrence of that `rematerialize_file_ingested`
reconciliation gap on a structural (non-journals) page would not match that
pattern and would misreport as a fresh regression on every sighting, exactly
the registry-narrowness gap `2026-08-24-write-org-file-sql-reads-ceiling-flake`
documents for a different family. That is an ORACLE gap — the classifier's
registered oracle doesn't cover the shape — not a COVERAGE or ENVIRONMENT one.

**ENVIRONMENT (secondary).** Kept only because a genuine one-off
settle-timing artifact unrelated to either registered family remains a live
alternative that the missing case detail cannot rule out; it is not the
primary conclusion.

## Root cause

Not diagnosed, and the tension above is not resolved: this entry does not
know whether the 2026-08-24 sighting is (a) the known-red
`bulk-add-sibling-order-under-journals` family reached by a different
transition sequence over a structural page rather than journals, (b) a
regression of the FIXED 2026-07-10 writeback-cache defect, (c) a third,
distinct sibling-order defect at the same seam, or (d) a genuine one-off
timing artifact unrelated to either. Nothing in what survived — no page
identity, no transition sequence, no before/after ordering — supports picking
among these.

Adjudicated DRAWN by A/B: rerun at the landing lane's tip and at the base arm
both came back green, so this occurrence was not caused by the lane under
test. That clears the lane; it does not clear the possibility that this is a
recurrence of a documented, real (non-timing) defect that the A/B's small
rerun count (4/4 each arm) simply did not redraw — the known-red family's own
history shows it requires a specific 3-transition sequence to reproduce
reliably, so a handful of reruns with a different random draw would not be
expected to hit it even if it is present.

## Missing piece

The page identity, the transition sequence, and the actual vs. expected
sibling orderings are gone, so there is no way to check this sighting against
either established family's documented shape. Making this attributable would
need the sibling-order invariant to log both orderings (rendered and
reference) plus the transition sequence that produced them, captured before
log rotation — that is exactly what let the `bulk-add-sibling-order-under-journals`
row get root-caused instead of staying a guess.

## Remedy

OPEN in the sense that the mechanism is undiagnosed and the family match is
unresolved. UNCLASSIFIED rather than NOTED: given two real, non-timing
sibling-order families already exist at this seam, treating a fresh sighting
as settled with no follow-up would risk hiding a live recurrence the registry
cannot yet recognize. If a structural-page sibling-order
mismatch recurs, capture the case's log, the transition sequence, and both
orderings before they rotate out, and check the sequence against
`bulk-add-sibling-order-under-journals` first.
