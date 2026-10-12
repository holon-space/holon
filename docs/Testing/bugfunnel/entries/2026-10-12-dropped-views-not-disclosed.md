---
id: 2026-10-12-dropped-views-not-disclosed
date: 2026-10-12
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  When the boot dropped and built again the database's materialized views, the user saw no
  notice, because the session factory skipped `OpenChange::ViewsDropped`.
---

## Bug
Found by the honeymoon2 boot-ladder census (code audit, lane boot-ladder-gaps). Reproduced
red-first by `an_upgrade_builds_the_same_views_as_a_fresh_boot` in
`crates/holon-app/tests/database_open_disclosure.rs`
(`target/boot-ladder-gaps-red-views-dropped.txt`): "views dropped at boot were not
disclosed: []".

## Root cause
`crates/holon-app/src/wiring.rs` (FrontendSession factory) mapped each `OpenChange` to a
condition, but had `OpenChange::ViewsDropped { .. } => continue`.

## Missing piece
No condition kind existed for dropped views, and the disclosure test asserted only that an
upgrade did not move the file aside.

## Remedy
New `ConditionKind::DatabaseViewsDropped { reason, views }` (kind
`database-views-dropped`, Info, Toast, all-clear at restart). The text says why the views
were dropped and that every table row is kept. The factory raises it.
