---
id: 2026-07-10-org-writeback-mints-vault-file-document
date: 2026-07-10
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Org writeback RE-MINTS a vault file's document `#+ID:` (Frontends.org
  0776062a→102ea172 during dogfood boot; identity mutation of a real file —
  breaks every reference to the old doc id)
source_line: 838
---

## Bug

Org writeback RE-MINTS a vault file's document `#+ID:` (Frontends.org
0776062a→102ea172 during dogfood boot; identity mutation of a real file —
breaks every reference to the old doc id)

## Missing piece

doc-id round-trip stability at real-vault shape never asserted; the
block-roundtrip PBTs don't pin the document-level `#+ID`

## Remedy

FIXED: root cause = same-named subdir's name-chain placeholder page (random
id) hijacked the file's identity via `LiveDocumentManager::create`
`(parent,title)` dedup (SqlOnly-specific). File `#+ID` now authoritative at
ingest (`create_forcing_id`) and writeback resolves by `#+ID` not name
chain. Pinned by full-boot property
`prop_doc_id_stable_under_same_named_subdir`. Residual wart: duplicate
file-doc/dir-placeholder page pair (safe, no data loss) — unification
deferred
