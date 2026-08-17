---
id: 2026-07-10-silent-block-loss-ingest-writeback-gpui-00c8c6
date: 2026-07-10
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  SILENT BLOCK LOSS in ingest→writeback: the GPUI.org region that previously
  failed ingest (same-file `:ID:` parent + children) now ingests WITHOUT error
  but its blocks never land, and org writeback rewrites the file WITHOUT them
  — ~20 lines deleted from a real vault file, no error, no banner (post-fix
  verification dogfood; jj-restored)
source_line: 836
---

## Bug

SILENT BLOCK LOSS in ingest→writeback: the GPUI.org region that previously
failed ingest (same-file `:ID:` parent + children) now ingests WITHOUT error
but its blocks never land, and org writeback rewrites the file WITHOUT them
— ~20 lines deleted from a real vault file, no error, no banner (post-fix
verification dogfood; jj-restored)

## Missing piece

real-vault file shape not ingestable headless (same open residue as the
boot-panic row); no invariant "writeback may not drop blocks that exist on
disk and were not user-deleted"

## Remedy

FIXED. Root cause: hard FK `block_requires.required_id → block_raw(id)`
(same for `advice_suppressed.lesson_id`) — but `:REQUIRES:`/`:BLOCKED-BY:`
legitimately dangle forward-in-file/cross-file; the create-txn FK rollback
was misattributed as ParentNotFound, aborting the whole file's ingest, and
the CDC re-render wrote the truncated prefix to disk. Remedy: soft-target
FKs dropped (dangling = representable state; consumers join/anti-join
tolerate) + **writeback quarantine** (partially-ingested file is never
rewritten until clean re-ingest; loud ERROR + banner). Pinned by full-boot
`region_writeback_loss` (scrubbed GPUI.org fixture, both create + update
paths) + guard test. Latent flagged: `is_parent_fk_violation` guesses the
failing constraint — inspect constraint name in a follow-up. **LIVE
VERIFICATION 2026-07-10: data-loss vector confirmed closed (disk
byte-identical, failure loud) but ingest still RED on the real vault — see
follow-up row**
