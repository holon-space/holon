---
id: 2026-07-10-org-ingest-strips-plain-text-marks
date: 2026-07-10
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  Org ingest strips `[[...]]` to plain text (marks NULL) and writeback
  persists the stripped form — user's link syntax permanently destroyed on
  disk (seeded `[[Linked Page]]` → bare "Linked Page" in DB and in the
  rewritten .org). Extends the open org-drops-marks row from "render loses
  marks" to on-disk data loss
source_line: 887
---

## Bug

Org ingest strips `[[...]]` to plain text (marks NULL) and writeback
persists the stripped form — user's link syntax permanently destroyed on
disk (seeded `[[Linked Page]]` → bare "Linked Page" in DB and in the
rewritten .org). Extends the open org-drops-marks row from "render loses
marks" to on-disk data loss

## Missing piece

CORRECTED after fix: escape was pure ORACLE — generators DID emit `[[…]]`;
the keystone reference model was TUNED TO THE LOSSY BEHAVIOR
(`normalize_content_for_org_roundtrip` extracted marks then kept only the
stripped label → both sides `marks=None`), and the SUT snapshot readers
never even SELECTed `marks` (third blind leg); the Block↔file round-trip
PBTs structurally never touch the store

## Remedy

FIXED (red-first, 2026-07-10/11): prod = `build_block_params` now emits
`marks` (sole store writer that dropped them; parser/renderer were always
correct). Oracle = reference computes the writeback→re-ingest fixed point
over (content, marks), `BlockFacet::Marks` compare, marks-aware SUT readers;
A/B proven (fix reverted → RED on Marks facet; restored → GREEN; 259-test
sweep). INCREMENT 2 LANDED: `block_links` junction (soft targets,
`EntityRef::Name` for dangling), write-txn resolution + re-resolve on page
creation, `backlinks` IVM matview (entity-shaped, incrementally maintained),
`[[id][label]]`/`[[label]]` canonicalization byte-stable. Open: backlinks UI
(incr 3), rename semantics (unruled), keystone junction correspondence
