---
id: 2026-07-10-rule-created-journal-block-puts-date
date: 2026-07-10
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Rule-created journal block puts the date in `properties.name` with EMPTY
  `content` → empty row in UI, `* ` headline + `:name:` property on disk (live
  dogfood vs holon-pkm; rule fired correctly otherwise: v5 id, parent,
  at-most-once)
source_line: 839
---

## Bug

Rule-created journal block puts the date in `properties.name` with EMPTY
`content` → empty row in UI, `* ` headline + `:name:` property on disk (live
dogfood vs holon-pkm; rule fired correctly otherwise: v5 id, parent,
at-most-once)

## Missing piece

capstone/PBT oracles assert the field the impl writes (`name` property), not
the org/render truth (`content`); no invariant says "a created journal block
renders its date"

## Remedy

FIXED at the rule boundary (only the journal rule ever passed `name` to
block.create): seeded rule emits `content: col("name")`; ids/FiringKey
unchanged. Oracle FLIPPED: capstone asserts date-in-content, no name
property, org round-trip to `* <date>`. Also fixed the pre-existing-broken
Journals page query (see COVERAGE row below)
