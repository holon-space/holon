---
id: 2026-07-10-gpui-org-still-fails-full-ingest
date: 2026-07-10
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  GPUI.org STILL fails full ingest post-P0-fix (live verification):
  `update_in_tree` aborts at `flow-mode-shell` with "parent block not found:
  block:ba5ad62d…" although that parent IS in the DB (sibling
  `capture-mode-overlay` landed under it); 37/44 blocks roll back. Headless
  fixture repro (same file pair) is GREEN — only the full real vault
  reproduces
source_line: 837
---

## Bug

GPUI.org STILL fails full ingest post-P0-fix (live verification):
`update_in_tree` aborts at `flow-mode-shell` with "parent block not found:
block:ba5ad62d…" although that parent IS in the DB (sibling
`capture-mode-overlay` landed under it); 37/44 blocks roll back. Headless
fixture repro (same file pair) is GREEN — only the full real vault
reproduces

## Missing piece

incremental `on_file_changed`/`update_in_tree` parent resolution diverges
from the batch path only at real-vault corpus (50 files, cross-file
`:REQUIRES:`, seed interactions); no full-vault-mirror test exists

## Remedy

RESOLVED as artifact + gap closed: rowid forensics of the recovered live DB
proved the refuting run used a STALE BINARY (its schema still carried the
dropped FKs) — the FK-drop fix is the real fix; minimal corpus = ONE file
with a forward same-file `:REQUIRES:` target. `is_fk_violation` now verifies
parent presence before claiming ParentNotFound (2 misdiagnosis rounds
ended). KEYSTONE now detects the class: forward-edge corpus seeded through
the real ingest path + the enhanced
`inv-blocks-match-ref/{block_raw,matview}` arms whose missing-id direction
reports a dropped block as loud INGEST DATA LOSS (parsed==landed) — A/B
verified RED (FK re-added) / GREEN (fix), twice, incl. by orchestrator.
Standalone `forward_edge_ingest_regression`. CONFIRMED live 2026-07-10
(fresh build vs holon-pkm): **44/44** GPUI.org ids ingested, zero
errors/quarantine/panic, vault byte-stable
