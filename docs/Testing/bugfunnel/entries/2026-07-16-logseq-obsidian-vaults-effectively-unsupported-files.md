---
id: 2026-07-16-logseq-obsidian-vaults-effectively-unsupported-files
date: 2026-07-16
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  LogSeq + Obsidian vaults effectively unsupported: `.md` files are never
  scanned/ingested by the app (LogSeq vault: only its stray `.org` file
  ingested — LATER/NOW, lowercase `id::`, `((uuid))` refs unreachable;
  Obsidian vault: NOTHING ingested, 18 blocks = pure seed);
  holon-markdown/logseq.rs parser exists but is unwired in the orgmode sync
source_line: 834
---

## Bug

LogSeq + Obsidian vaults effectively unsupported: `.md` files are never
scanned/ingested by the app (LogSeq vault: only its stray `.org` file
ingested — LATER/NOW, lowercase `id::`, `((uuid))` refs unreachable;
Obsidian vault: NOTHING ingested, 18 blocks = pure seed);
holon-markdown/logseq.rs parser exists but is unwired in the orgmode sync

## Missing piece

markdown ingest wiring absent from the app; no cross-format vault fixture in
any harness

## Remedy

OPEN — write-back to `.org` inside a LogSeq vault verified working
(small-scale)
