---
id: 2026-07-07-ios-add-block-creation-slot-parents
date: 2026-07-07
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  iOS add-block: creation slot parents to panel id, engine rejects create
source_line: 849
---

## Bug

iOS add-block: creation slot parents to panel id, engine rejects create

## Missing piece

no text-sync-on-virtual transition; creation-slot code unreachable

## Remedy

FIXED (landed main e831f0bd): `resolve_creation_parent` resolves the slot
parent to the query focus root + keystone `create_block_under_focus`
transition
