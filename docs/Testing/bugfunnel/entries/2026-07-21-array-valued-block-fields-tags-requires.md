---
id: 2026-07-21-array-valued-block-fields-tags-requires
date: 2026-07-21
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Array-valued block fields (tags, requires) carry duplicates in Martin's
  vault (Page tag 5×/4×/1×; requires targets 5×, e.g.
  `block:ac4-readonly-sharing`) — disk is clean (`:REQUIRES:` once, 0 `:Page:`
  tags), so the multiplication is projection/ingest-side; `requires` renders
  duplicated in the UI, tag/edge queries skew, and a writeback of the
  in-memory multiset would corrupt the org file. PHASE-2: current main's
  reingest/reboot is IDEMPOTENT (tags stayed `["Page"]` over 3× file-watch
  reingest + 3× restart; requires stayed 1×) — the 5×/4× is FROZEN LEGACY
  cruft in the persisted DB from older builds, not ongoing accumulation.
source_line: 1082
---

## Bug

Array-valued block fields (tags, requires) carry duplicates in Martin's
vault (Page tag 5×/4×/1×; requires targets 5×, e.g.
`block:ac4-readonly-sharing`) — disk is clean (`:REQUIRES:` once, 0 `:Page:`
tags), so the multiplication is projection/ingest-side; `requires` renders
duplicated in the UI, tag/edge queries skew, and a writeback of the
in-memory multiset would corrupt the org file. PHASE-2: current main's
reingest/reboot is IDEMPOTENT (tags stayed `["Page"]` over 3× file-watch
reingest + 3× restart; requires stayed 1×) — the 5×/4× is FROZEN LEGACY
cruft in the persisted DB from older builds, not ongoing accumulation.

## Missing piece

no array-field-uniqueness invariant (`tags`/`requires` no-duplicates) + no
repeated-reingest rung; parse these fields into a Set at the boundary.
Remedy reframed by phase-2 to a one-time DB dedup/cleanup migration for
existing vaults (the ingest path is already idempotent on current main).

## Remedy

open
