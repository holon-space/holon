---
id: 2026-09-26-typed-drawer-key-under-a-plain-property-makes-the-file-unparseable
date: 2026-09-26
gap: COVERAGE
secondary: null
status: PARTIAL
summary: >-
  A plain property whose key names a field the org parser lifts into a typed
  field (`widget_only`, `TASK_STATE`, `Priority`, `REQUIRES`, ...) is written
  to the drawer, and a value that field cannot parse makes the whole file
  fail to parse.
---

## Bug
Found by agent exploration during the group A fix round (org-faithful,
probe kept as `lane-logs/groupA-r2-probe-lifted.rs.txt`, output
`lane-logs/groupA-r2-probe-lifted.log`). A block with property
`widget_only = "x"` renders `:widget_only: x`; the re-parse fails with
`:WIDGET_ONLY: must be t or true`. The same holds for `Widget_Only`,
`TASK_STATE` (any value), `Priority` (not one uppercase letter), and
`REQUIRES`/`BLOCKED-BY`/`ADVICE_SUPPRESSED`/`contributes-to` (a value that is
not bare ids, e.g. `[[x]]`). A `TAGS` property with a line break also cost the
block its place in the re-parse (not traced).

## Root cause
`drawer_properties` (`crates/holon-org-format/src/models.rs`) excludes only
the exact-case internal keys, so another spelling passes as a plain property.
The parser (`crates/holon-org-format/src/parser.rs`, the drawer loop and
`lift_edge_properties`) lifts these keys case-insensitively into typed fields
and refuses the file on a bad value.

## Missing piece
No generator writes a plain property under a lifted key, and the engine key
guard does not know the lifted keys.

## Remedy
The engine refuses a property write under every spelling the ingest lifts,
through a properties bag or as a flat field, and names the key. Both ask one
predicate, `TypedDrawerKey` (`crates/holon-org-format/src/drawer.rs`), which
the parser's drawer loop also dispatches on. A flat edge column
(`requires`, `contributes_to`) and `priority` stay their own field's write.
Pinned by `editing_suite::drawer_key_write_boundary::every_spelling_the_ingest_lifts_is_refused_as_a_property`,
whose spellings come from `TypedDrawerKey::spellings()`. Open: a block that
already holds such a property still renders it, and the file then fails to
parse on a bad value; `drawer_properties` excludes only the exact-case keys.
