---
id: 2026-07-21-boot-panic-vault-folder-containing-space
date: 2026-07-21
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Boot PANIC on any vault folder containing a space — `Directory` entity
  minted path-shaped ids (`generate_directory_id` returned a raw relative
  path) through `EntityUri::from_raw`, whose unschemed-string fallback maps to
  `block:<s>`; `block:Projects/DBG/Agentic DPL` is invalid RFC 3986 →
  `entity_uri.rs:58` panic, fatal under `[profile.release] panic="abort"`.
  Directory was write-only (produced by the boot scan into a `directory` table
  + QueryableCache, read by NOTHING; only non-block `BlockEntity` impl).
source_line: 1069
---

## Bug

Boot PANIC on any vault folder containing a space — `Directory` entity
minted path-shaped ids (`generate_directory_id` returned a raw relative
path) through `EntityUri::from_raw`, whose unschemed-string fallback maps to
`block:<s>`; `block:Projects/DBG/Agentic DPL` is invalid RFC 3986 →
`entity_uri.rs:58` panic, fatal under `[profile.release] panic="abort"`.
Directory was write-only (produced by the boot scan into a `directory` table
+ QueryableCache, read by NOTHING; only non-block `BlockEntity` impl).

## Missing piece

keystone/PBT wiring never constructs `OrgModeSyncProvider` and never builds
a `Directory` — `sync_controller_mutation_pbt` drives `FileSyncController`
against `MockDocumentManager`, so the failing path is absent from the test
environment entirely; secondary: `path_segment()` alphabet
`[a-zA-Z][a-zA-Z0-9]{0,15}` excluded spaces

## Remedy

FIXED (Directory entity + table + DDL + cache feed + `known_dirs` deleted;
red-first repro `test_directory_name_with_space_syncs` at the
`OrgModeSyncProvider` layer survives as regression lock; PBT alphabet
widened to spaces + non-ASCII as adjacent coverage)
