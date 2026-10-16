---
id: 2026-10-16-a-blocks-owner-file-is-quarantined-while-another-file-holds-its-copy
date: 2026-10-16
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A file that owns a block is refused wholesale as declaring another file's id
  while that other file holds a copy of the block, so the owner's own
  write-back is quarantined.
---

## Bug

Two files each hold a copy of a block the other owns: `structural-page.org`
holds a copy of `decision-page.org`'s `block:kd-1`, `decision-page.org` holds a
copy of `structural-page.org`'s `block:c1`. Saving `structural-page.org`
without `c1` cascades an ingest of `decision-page.org`, which is refused:

    [FileSyncController] DUPLICATE BLOCK ID: 'block:kd-1' is declared by BOTH
    structural-page.org and decision-page.org, so decision-page.org is NOT
    ingested — QUARANTINING

The refused file is `kd-1`'s OWNER. Its ingest never registers its copies, so
the `block-in-two-files` disclosure the user is owed for the other copy is
missing too (`inv-conditions-match-ref`).

Found by the composed keystone, not by dogfooding: 2 of 10 full-depth runs
measured at base `b8cc2758` (`HOLON_PBT_FORCE_FULL=1`, 4 cases per run) raise
the refusal; the subject block is seed-dependent (`block:archive-25` there,
`block:archive-89` in another lane's measurement run, where the refused file
also lost a headline from its own render).

## Root cause

`block_home` is replaced wholesale per file on every ingest
(`note_block_homes`), so the ingest of the file holding the COPY takes over the
block's home. The cross-doc-membership guard gives the home back to the owner,
but it runs at the END of that ingest — after the cascaded ingest of the owner
file inside it.

The cascaded ingest therefore finds the slug claimed by another file and asks
whether the write authority routes it to that CLAIMANT's document
(`authority_routes_slug_to_claimant`). It does not: it routes the slug to the
candidate's own document. The check was one-directional, so "the candidate is
the rightful owner" fell into the contested-identity case and refused the whole
file.

## Missing piece

No dedicated or deterministic test covered two files each holding a copy of a
block the other owns. The D229.b hand-authored cases all have ONE copy holder
and one owner, which the one-directional check handles.

## Remedy

`colliding_block_slug` now tolerates the authority routing the slug to either
file — the claimant's document (a copy or half of a move, as before) or the
candidate's own document (the candidate is the owner, the claimant holds the
copy). `authority_routes_slug_to_claimant` became
`authority_routes_slug_to_file`.

Pinned by the hand-authored regression case
`two-files-each-hold-a-copy-of-a-block-the-other-owns`
(`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`,
`just hand-authored`).
