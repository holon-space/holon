---
id: 2026-10-10-an-upgrade-deletes-the-database
date: 2026-10-10
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A binary with another scalar-function set deletes the whole database at
  open, so an ordinary upgrade loses every row only the database holds (all
  of them in SqlOnly) and says the org files restore them.
---

## Bug
Found by a verifier code audit in lane block-l1 (finding F2,
`.claude/worktrees/block-l1/lane-logs/bl1-verify4.md`), tracked as
holon-space/holon-work#37 (lane sqlonly-keep).

## Root cause
`crates/holon-turso/src/db_open.rs` `open` calls `rebuild`, which deletes
`holon.db`, `-wal` and `-shm`, whenever `unusable_reason` returns a reason:
a changed scalar-function signature, an absent or unreadable signature
record, or a materialized view the engine cannot load. None of these makes a
table row invalid: the functions are used only in materialized views and the
`block_derived` cache (`crates/holon-api/src/computation.rs`). The delete
takes the navigation history, redirects, identities and user-type rows in
every mode, and in SqlOnly also the block data the org files do not hold.
`block_raw` is `Class::Rebuilt` (`table_classes.rs`), so that loss is not
even disclosed, and the log says the org files and the Loro store rebuild it.

## Missing piece
No keystone transition changes the function set between two boots (every
reboot reopens the database with the same binary), and the unit test
`db_open::tests::a_database_opened_by_a_larger_set_is_rebuilt_and_records_it`
asserted the deletion as the wanted outcome.

## Remedy
Keystone transition `RebootAfterUpgrade` (hand-authored cases
`an-upgrade-keeps-the-navigation-history{,-sqlonly-arm}`), the app test
`crates/holon-app/tests/sqlonly_database_survives_function_set_change.rs`,
and rewritten `db_open` unit tests pin that rows survive. Fix: `db_open`
drops only the materialized views and clears `block_derived`, in one
transaction that writes the signature last, and never deletes the file. A
file the engine cannot open is moved aside to `<db>.unusable-<timestamp>`
and disclosed as condition `database-moved-aside`.
