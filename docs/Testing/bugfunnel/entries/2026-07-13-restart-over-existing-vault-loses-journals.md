---
id: 2026-07-13-restart-over-existing-vault-loses-journals
date: 2026-07-13
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Restart-over-existing-vault LOSES the Journals page view definition:
  `journals::src::0` + `journals::render::0` are seeded with
  `_routing_doc_uri=block:__default__` but `parent_id=block:journals` (seed
  log line 103), so org writeback emits them into NEITHER `__default__.org`
  NOR `Journals.org` (verified on disk; sibling src blocks under headings
  round-trip fine, `journals::action::0` under its heading survives); next
  boot's file-authority ingest then deletes both from the DB (22 vs 24 blocks)
  — journal list rendering silently gone after one restart. Also observed:
  `sentinel:no_parent` materializes as a block_raw ROW on second boot (absent
  on first boot)
source_line: 974
---

## Bug

Restart-over-existing-vault LOSES the Journals page view definition:
`journals::src::0` + `journals::render::0` are seeded with
`_routing_doc_uri=block:__default__` but `parent_id=block:journals` (seed
log line 103), so org writeback emits them into NEITHER `__default__.org`
NOR `Journals.org` (verified on disk; sibling src blocks under headings
round-trip fine, `journals::action::0` under its heading survives); next
boot's file-authority ingest then deletes both from the DB (22 vs 24 blocks)
— journal list rendering silently gone after one restart. Also observed:
`sentinel:no_parent` materializes as a block_raw ROW on second boot (absent
on first boot)

## Missing piece

keystone never restarts over a written-back vault (writeback→re-ingest
lifecycle); no invariant "every non-sentinel DB block renders into exactly
one org file" (would catch routing/parent doc mismatch at seed time)

## Remedy

OPEN — dogfood #5; fix = route journals src/render blocks to the journals
doc (or parent them per routing); the renderable-to-one-file invariant
closes the class
