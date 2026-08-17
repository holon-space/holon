---
id: 2026-07-21-only-org-files-only-reingest-empty
date: 2026-07-21
gap: PERCEPTION
secondary: COVERAGE
status: UNCLASSIFIED
summary: >-
  ID-only org files (only `#+ID:`, no `#+TITLE:`) reingest as EMPTY-content,
  ORPHANED `Page`s — blank sidebar rows rooted at depth 0, inverting
  parent/child indentation (live: `Resources.org` = `block:a9163ed8`
  content="" parent=no_parent; `Projects/DBG/Agentic DPL.org` =
  `block:9464fbf0` content="" parent=no_parent). Discriminator: the on-disk
  file is NOT the signal — `Resources.org` and `Projects.org` are
  byte-identical (`#+ID: <uuid>\n`, 43 bytes), yet one titles and one blanks.
  On boot the Loro tree projects to SQL BEFORE org ingest, so a `Page` a
  `convert_block_to_page`/delete had persisted with empty content is already
  present; `FileSyncController` resolves the doc-root by `#+ID` (`get_by_id` →
  `Some(empty page)`) and REUSES it, bypassing the filename-title default the
  create arm gives a genuinely-new page — so `content=""` survives (and
  `parent=no_parent` orphaning survives for nested files). Suspected fallout
  adjacency to the Directory-entity purge (spaced-folder cluster).
source_line: 1072
---

## Bug

ID-only org files (only `#+ID:`, no `#+TITLE:`) reingest as EMPTY-content,
ORPHANED `Page`s — blank sidebar rows rooted at depth 0, inverting
parent/child indentation (live: `Resources.org` = `block:a9163ed8`
content="" parent=no_parent; `Projects/DBG/Agentic DPL.org` =
`block:9464fbf0` content="" parent=no_parent). Discriminator: the on-disk
file is NOT the signal — `Resources.org` and `Projects.org` are
byte-identical (`#+ID: <uuid>\n`, 43 bytes), yet one titles and one blanks.
On boot the Loro tree projects to SQL BEFORE org ingest, so a `Page` a
`convert_block_to_page`/delete had persisted with empty content is already
present; `FileSyncController` resolves the doc-root by `#+ID` (`get_by_id` →
`Some(empty page)`) and REUSES it, bypassing the filename-title default the
create arm gives a genuinely-new page — so `content=""` survives (and
`parent=no_parent` orphaning survives for nested files). Suspected fallout
adjacency to the Directory-entity purge (spaced-folder cluster).

## Missing piece

keystone STRUCTURALLY cannot represent this class: (1)
`SutFixtureFs::CreateDirectory` (spaced subdir) is `!app_started`-gated and
unreachable in the boot-pre-started composed config; (2)
`convert_block_to_page` (BlockToPage) leaves NON-empty content in the store,
and no transition zeroes a page's content or
deletes-a-page-but-leaves-the-file, so no reingest ever hits `get_by_id →
Some(empty)`; (3) no cross-boot Loro-projects-then-org-ingests ordering
rung. Secondary ENVIRONMENT: the empty content is produced by the writeback
lane (only `#+ID:` written, title lives in Loro/DB) — separate lane owns
that.

## Remedy

INGEST-HEAL FIXED 2026-07-21 (this lane) —
`FileSyncController::process_external_change` now heals a title-less `Page`
doc-root at reingest: empty content re-derived from the filename, and an
orphaned nested page reparented under its folder chain, through the SAME
store-mutation seam a normal block update uses (disclosed via
`tracing::warn!`, never a silent empty-content page). Belt-and-suspenders
for already-broken vaults; composes with the writeback-side `#+TITLE:`
persistence fix (separate lane). Red-first at the `on_file_changed` boundary
(`idonly_title_heal.rs`, 2/2; RED = `recorded updates = []`). Sidebar render
fallback (blank row, no disclosure) NOT touched here — separate follow-up.
