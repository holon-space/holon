---
id: 2026-07-10-silent-block-loss-ingest-writeback-gpui-555ccb
date: 2026-07-10
gap: ENVIRONMENT
secondary: ORACLE
status: MITIGATED
summary: >-
  SILENT BLOCK LOSS in ingest→writeback: the GPUI.org region that previously
  failed ingest (same-file `:ID:` parent + children) now ingests WITHOUT error
  but the blocks never land, and org writeback rewrites the file WITHOUT them
  — ~20 lines deleted from a real vault file, no error, no banner (found by
  post-fix verification dogfood, jj-restored)
source_line: 842
---

## Bug

SILENT BLOCK LOSS in ingest→writeback: the GPUI.org region that previously
failed ingest (same-file `:ID:` parent + children) now ingests WITHOUT error
but the blocks never land, and org writeback rewrites the file WITHOUT them
— ~20 lines deleted from a real vault file, no error, no banner (found by
post-fix verification dogfood, jj-restored)

## Missing piece

real-vault file shape not ingestable headless (same open residue as the
boot-panic row); no invariant "writeback may not drop blocks that exist on
disk and were not user-deleted" — a writeback guard/count check is missing

## Remedy

MITIGATED (2026-07-11): ingest→writeback guard landed —
FileFormatAdapter::check_writeback_lossless at
FileSyncController::ingest_file refuses write-back when any non-empty source
block survives by NEITHER id NOR normalized content; refusal = loud
IngestLoss error + file quarantined from all future write-backs until clean
re-ingest. Root-cause ingest drop still open (guard makes it fail loud
instead of destroying data); keystone repro belongs with the root-cause fix
