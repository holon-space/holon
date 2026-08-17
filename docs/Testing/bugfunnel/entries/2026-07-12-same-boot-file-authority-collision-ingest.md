---
id: 2026-07-12-same-boot-file-authority-collision-ingest
date: 2026-07-12
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Same boot, file-authority collision on `block:journals`: ingest of the
  populated `Journals.org` logged "Re-parenting 14 blocks from other documents
  to block:journals (… e.g. from seed_default_layout)". VERIFIED what
  re-parenting actually did: the 14 were NOT seed blocks — they were the
  page-files' CHILD blocks (Journals/2026-07-10.org etc.) whose roots the
  foreign-PAGE-doc-root skip protects but whose subtrees it did not: the
  companion emitted conflict-"update" ops that steal page-owned user blocks
  into itself (row-136 folder-companion class, general form: ANY user file
  inlining another doc's subtree, incl. a seeded doc id)
source_line: 970
---

## Bug

Same boot, file-authority collision on `block:journals`: ingest of the
populated `Journals.org` logged "Re-parenting 14 blocks from other documents
to block:journals (… e.g. from seed_default_layout)". VERIFIED what
re-parenting actually did: the 14 were NOT seed blocks — they were the
page-files' CHILD blocks (Journals/2026-07-10.org etc.) whose roots the
foreign-PAGE-doc-root skip protects but whose subtrees it did not: the
companion emitted conflict-"update" ops that steal page-owned user blocks
into itself (row-136 folder-companion class, general form: ANY user file
inlining another doc's subtree, incl. a seeded doc id)

## Missing piece

keystone never generates two on-disk representations of the same block ids
(companion + page-file); seed-vs-disk authority was implicit; rows 60/72
fixed only the DUPLICATE-page case, not a populated `#+ID: journals` file
colliding with the seed

## Remedy

FIXED (2026-07-12): file authority made explicit in
`FileSyncController::ingest_file` — a foreign PAGE-owned subtree (root +
transitive parsed descendants, `foreign_subtree_ids`) is never
created/updated/re-parented/placed/anchored from another file; the owning
page-file stays sole authority for the whole subtree; a populated disk file
with a seeded doc id still owns its document and ADOPTS seed gap-fill blocks
(conflict-adopt path unchanged for non-page-owned blocks — that's the
superset-merge), but can no longer steal page-owned user blocks; companion
write-back is DEFERRED (disclosed INFO, disk left byte-identical) so ingest
never de-inlines the user's file — de-inline stays the row-136 Fork-B
workstream's decision. `journals_auto_create_blocks` untouched (parallel
stream). NOTE for that stream: block-driven write-back of a companion doc
still renders WITHOUT the inlined page subtrees, so the mass-truncation
tripwire will veto+quarantine it until Fork B de-inlines — loud, no data
loss
