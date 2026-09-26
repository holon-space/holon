---
id: 2026-09-28-a-block-pasted-into-or-out-of-a-journal-day-page-is-quarantined-as-a-duplicate-id
date: 2026-09-28
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  While a pasted copy of a heading WITH CHILDREN stands in another file, the
  next ingest of the owner's file is refused with DUPLICATE BLOCK ID for a
  child and the owner file is quarantined. Found through a journal day page,
  but not specific to journals.
---

## Bug
Found by a default-weight keystone run in lane d229-move (round 3,
`lane-logs/d229r3-keystone-final-5.log`). Load average at the time: ~70–200
(not a latency red).

Shape (both storage arms):

1. `BulkExternalAdd` puts a heading with a child under the boot day's journal
   page (`Journals/2026-01-16.org`).
2. `PasteBlockCopy` pastes the heading, with its child, into `Journals.org`.
3. The next write of the day file is refused: `DUPLICATE BLOCK ID:
   'block:bulk-0-6' is declared by BOTH Journals.org and
   Journals/2026-01-16.org` quarantines the day file, and its later edits
   never reach the store.

## Root cause
D229.b. The ingest of a file that holds a copy records the copy's ids as that
file's (`note_block_homes`), then gives the claim back to the owner's file
for the copied HEADING only. The heading's children stay claimed by the copy
file, and the owner's next ingest collides on the first child
(`colliding_block_slug`). A top-level page with a copied subtree fails the
same way; the journal shape found it first.

A/B (round 4, `lane-logs/d229r4-journal-ab-{d229,main}.log`): one probe test,
byte-identical on both trees, pastes a day-page subtree into `Journals.org`
and then edits the day file. Main `d4f426ffc965` (no D229): green on both
arms. D229 tree before the fix: the day file is quarantined on both arms. A
paste into a plain page file (`Notes.org`) of a childless heading stays green
on both trees.

## Missing piece
The keystone's cut & paste excluded journal day pages and `Journals.org`, so
the alphabet no longer reached this shape; the org_suite copy tests never
re-ingested an owner file while a copied subtree with children stood.

## Remedy
FIXED. The owner's file keeps the claim on every member of the copied
subtree. The journal exclusion is removed from `movable_doc`. Red first,
green after: org_suite `the_owner_of_a_copied_subtree_is_still_ingested`,
`a_journal_day_page_whose_heading_is_copied_into_journals_is_still_ingested`,
and keystone records
`journal-day-heading-pasted-into-journals-keeps-the-day-file-ingested-{loro,sqlonly}-arm`
(`lane-logs/d229r4-red-journal*.log`, `lane-logs/d229r4-green-journal*.log`).
