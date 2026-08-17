---
id: 2026-07-20-share-subtree-doc-says-retention-full
date: 2026-07-20
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  share_subtree doc says retention "full or none" but "full" is
  runtime-disabled
source_line: 1051
---

## Bug

share_subtree doc says retention "full or none" but "full" is
runtime-disabled

## Missing piece

doc/impl parity; describe op accurately

## Remedy

OPEN (minor) — 2026-07-21 three-way confirmed (W2+W3+review): the 'full'
disable is OWNER-EXPORT-SCOPE (full forked oplog = whole-vault history
leak), INDEPENDENT of the W2 fork_at/retention=none fix — do NOT re-enable
on W2's basis. Doc/impl parity fix tracked under ADR 0028 H4
