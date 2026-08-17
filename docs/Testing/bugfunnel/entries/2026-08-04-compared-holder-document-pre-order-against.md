---
id: 2026-08-04-compared-holder-document-pre-order-against
date: 2026-08-04
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  `WritebackShadow::compare` compared the `home_by` holder's document
  pre-order against `doc_blocks`' sequence as flat lists. `doc_blocks` comes
  from `get_blocks`' `ORDER BY sort_key, id` — a GLOBAL sort over fractional
  indices minted PER SIBLING GROUP, so every group restarts at "80", 1861/1889
  live blocks share a sort_key with another row, and the prod CTE returns 14
  consecutive `sort_key="80"` rows from 14 different parents at depths 1-4.
  Its Vec is a set with WITHIN-PARENT relative order only, never a document
  sequence. Reported as `doc=block:journals` diverging at index 0
  (`journals::action::0` first vs last): a depth-2 grandchild, sole child of
  `auto-create`, whose group restarts at "80", ties with depth-1 keys and wins
  the `id` tiebreak.
source_line: 1158
---

## Bug

(Option C Inc 1 differential shadow, first armed run — a defect in the NEW
ORACLE, caught by the oracle itself firing) `WritebackShadow::compare`
compared the `home_by` holder's document pre-order against `doc_blocks`'
sequence as flat lists. `doc_blocks` comes from `get_blocks`' `ORDER BY
sort_key, id` — a GLOBAL sort over fractional indices minted PER SIBLING
GROUP, so every group restarts at "80", 1861/1889 live blocks share a
sort_key with another row, and the prod CTE returns 14 consecutive
`sort_key="80"` rows from 14 different parents at depths 1-4. Its Vec is a
set with WITHIN-PARENT relative order only, never a document sequence.
Reported as `doc=block:journals` diverging at index 0 (`journals::action::0`
first vs last): a depth-2 grandchild, sole child of `auto-create`, whose
group restarts at "80", ties with depth-1 keys and wins the `id` tiebreak.

## Root cause

Option C Inc 1 differential shadow, first armed run — the shadow's own
ORACLE was wrong, not either prod authority. `WritebackShadow::compare`
normalized the `home_by` holder to a document PRE-ORDER and compared it as a
flat sequence against `doc_blocks`, which is seeded from `get_blocks`'
`ORDER BY sort_key, id`. That is a GLOBAL sort over fractional indices
minted PER SIBLING GROUP: every group restarts at the same low key ("80"),
1861/1889 live blocks share a sort_key with another row, and the exact prod
CTE returns 14 consecutive `sort_key="80"` rows from 14 different parents at
depths 1-4, so cross-parent ties fall to the `id` tiebreak and interleave
the sequence. `get_blocks`' Vec is a SET WITH WITHIN-PARENT RELATIVE ORDER
ONLY — never a document sequence — so comparing it against a pre-order walk
was invalid by construction. The reported divergence (`doc=block:journals`:
`journals::action::0` first vs last) was a depth-2 grandchild, sole child of
`auto-create` whose own group restarts at "80", tying with depth-1 keys and
UUID-sorting to position 0. No move involved; H2 (mov_after projection
staleness) REFUTED — the Loro projector tie-suffixes within groups, dirties
both scopes on Move, and diffs sort_key first. PROD OUTPUT UNAFFECTED:
`render_entitys` re-nests by `parent_id` and reads only within-parent order,
and org bytes are identical from either sequence. Canonical order authority
= `BlockOrdering::children` per ADR-0005 (`sort_key` is a storage encoding
chosen by one adapter). Fixed by normalizing BOTH sides to `parent -> [child
ids]` and comparing per-parent; repro pinned in
`crates/holon-org-format/tests/get_blocks_flat_order_is_not_document_order.rs`;
`BlockReader::get_blocks` now documents the within-parent-only guarantee and
names both impls (`LoroBlockReader::collect_subtree` = pre-order DFS,
`CacheBlockReader` = flat global sort))

## Missing piece

The shadow asserted an ordering guarantee that NEITHER authority makes and
that no consumer reads. Missing piece = the `get_blocks` contract was
undocumented, so a new consumer could (and did) treat the result as a
sequence; and the two impls legitimately differ
(`LoroBlockReader::collect_subtree` is a pre-order DFS, `CacheBlockReader`
is the flat global sort), which nothing stated.

## Remedy

FIXED 2026-08-04. Adjudicated: ORACLE defect, NOT a prod ordering bug — H2
(mov_after projection staleness) REFUTED (the Loro projector tie-suffixes
within groups, dirties both scopes on Move, diffs sort_key first), and org
output is byte-identical from either order because `render_entitys` re-nests
by `parent_id`. Canonical order authority = `BlockOrdering::children` per
ADR-0005. Fix: both sides normalized to `parent -> [child ids]`, compared
per-parent (`writeback_shadow.rs`); `BlockReader::get_blocks`
(`sync_ports.rs`) now documents the within-parent-only guarantee and names
both impls; repro pinned in
`crates/holon-org-format/tests/get_blocks_flat_order_is_not_document_order.rs`.
Turso CTE deliberately NOT changed to pre-order — noted as an Inc-2
consideration in the design doc. Second, independent oracle gap found while
fixing this: quiescence ignored controller-side activity, because
`doc_blocks` is also mutated by file-ingest / boot-scan / poll /
full-rerender paths that emit no feed event; the check now requires BOTH
sides unchanged between consecutive checks.
