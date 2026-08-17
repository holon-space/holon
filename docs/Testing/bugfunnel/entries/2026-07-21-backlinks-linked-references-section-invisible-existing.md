---
id: 2026-07-21-backlinks-linked-references-section-invisible-existing
date: 2026-07-21
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Backlinks "Linked references" section invisible on existing vaults — the
  section was added only to `assets/default/index.org`
  (`default-main-panel::render::0`, the fresh-vault seed) and never migrated
  into an existing `__default__.org` whose `default-main-panel` has only a
  `src::0` GQL query and NO `render::0` block; the live backlinks query
  returns rows (Denis → "Schwester von Denis") but nothing renders. PHASE-2
  VALIDATED: the section DOES render on a fresh vault seeded from the current
  default, confirming the invisibility is stale-existing-layout wiring, not a
  broken feature.
source_line: 1084
---

## Bug

Backlinks "Linked references" section invisible on existing vaults — the
section was added only to `assets/default/index.org`
(`default-main-panel::render::0`, the fresh-vault seed) and never migrated
into an existing `__default__.org` whose `default-main-panel` has only a
`src::0` GQL query and NO `render::0` block; the live backlinks query
returns rows (Denis → "Schwester von Denis") but nothing renders. PHASE-2
VALIDATED: the section DOES render on a fresh vault seeded from the current
default, confirming the invisibility is stale-existing-layout wiring, not a
broken feature.

## Missing piece

no layout-migration path that adds/updates `render::0` in existing vaults
(or render backlinks unconditionally, not via a seed-only render block);
keystone always seeds fresh `index.org` so it cannot see the stale-layout
wiring

## Remedy

open
