---
id: 2026-10-09-warm-boot-rebuilds-commented-matviews
date: 2026-10-09
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Every warm boot dropped and rebuilt block_with_path and four other matviews
  over an unchanged schema; the block_with_path rebuild alone took 2.5-2.8 s
  at 10k blocks and 40 s at 50k.
---

## Bug

A release reboot measurement on the lazy-boot lane (`diag_harness`,
`HOLON_SOAK_REBOOT=1`, real `holon_app` Full boot) showed a
`matview_ddl view="block_with_path"` DROP and a 2 457 ms CREATE on the second
boot over the same `test.db`. At 50k blocks the CREATE took 40 s and the SQL
actor watchdog fired. Android boot speed and memory are the reason it matters.

## Root cause

`reconcile_named_view` (`crates/holon-turso/src/matview_manager.rs`) decided
"unchanged" by comparing whitespace-collapsed TEXT of `sqlite_master.sql`
against the desired CREATE. Turso stores a matview definition re-rendered from
its AST: `--` comments are gone and the spacing is its own (`AS (SELECT`,
`p.id)`, `COUNT (*)`). Every definition loaded from a commented `.sql` file
therefore compared unequal on every boot and was dropped and rebuilt, with its
dependents: `automations_journal`, `block_with_path`, `focus_roots`,
`journal_day_pages`, `journal_feed` (red log of
`crates/holon-turso/tests/warm_reboot_keeps_matviews.rs` prints each stored
definition).

## Missing piece

No test rebooted over an unchanged schema and asserted that no matview DDL
ran. At test scale the rebuild costs milliseconds, so no timing showed it;
only a real-size reboot did.

## Remedy

The compare is now over SQL tokens (`view_sql_tokens`: comments and whitespace
dropped, unquoted words case-insensitive, literals exact). The new test boots
every holon-turso schema module, reboots, and asserts no `matview_ddl` event
names an existing view; it also asserts that a changed `block` definition
still rebuilds `block` and `block_with_path` once, with correct rows, and
nothing on the boot after.

Release build, 10k blocks (435 files x 22), `diag_harness` reboot, loaded
host: before `start_app` 17 080 / 5 225 / 7 311 ms with `block_with_path`
CREATE 13 700 / 2 875 / 4 621 ms; after `start_app` 3 137 / 3 684 / 3 161 ms
with 0 matview DDL against existing views (sum of all reboot `matview_ddl`
<= 1 ms).
