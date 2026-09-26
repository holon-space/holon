---
id: 2026-09-26-loro-projection-writes-null-priority-into-every-properties-bag
date: 2026-09-26
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The Loro→SQL projection and the org-ingest params wrote `"priority": null`
  into the `properties` bag of every block that has no priority, so the SQL row disagreed with the Loro
  authority and every `TaskEntity::priority()` read of such a row logged a
  warning.
---

## Bug
Found by a verifier auditing the Loro UI row parity work
(`2026-09-25-loro-ui-row-drops-edge-fields-so-tag-conditions-never-match`),
by reading the code behind an exemption in
`crates/holon-app/tests/loro_ui_row_matches_sql_row.rs`. The exemption ignored
a Null `priority` in the SQL bag when the Loro bag had none.

## Root cause
`block_to_params` (`crates/holon-loro/src/loro_sync_controller.rs`) inserted
`priority: Value::Null` for a block with no priority, and `block_diff_params`
emitted Null for a cleared one. Their comments called `priority` a SQL column.
It is not: `holon_pattern::schema::BLOCK` (`crates/holon-pattern/src/schema.rs`)
declares no such field, so `SqlOperationProvider::partition_params`
(`crates/holon/src/core/sql_operation_provider.rs`) routes it to the overflow
bag, where a Null is stored as a JSON null. `Value::Null` is not
`Value::REMOVED`, so nothing removed it. `TaskEntity::priority`
(`crates/holon-core/src/traits.rs`) then finds `Some(Null)`, fails
`Priority::try_from`, and warns once per block read.

Red: the parity test without the exemption,
`lane-logs/tags4-c-parity-red.log` — `block:doc: properties Loro Object({})
!= SQL Object({"priority": Null})`, and the same for every block without a
priority.

## Missing piece
No invariant compares the SQL `properties` bag with the Loro bag key for key.
The keystone generates blocks without a priority in every case; the only
Loro-vs-SQL row comparison is the harness test above, and it carried the
exemption.

## Remedy
The projection emits no `priority` for a block without one, and a cleared
priority becomes `Value::REMOVED`, like any other removed property. The parity
test compares the bags exactly (`lane-logs/tags4-c-parity-green.log`).

The org-ingest leg (`crates/holon-orgmode/src/block_params.rs`) had the same
shape: it always inserted `priority`, `Value::Null` when the headline has
none. Red through the production write path
(`lane-logs/h1-red-org-store.log`,
`holon-app::org_store_org_round_trip::a_block_that_never_had_a_priority_stores_no_priority_key`:
`"priority": Null` in the stored bag after a create and after an update; and
`a_file_that_lost_its_priority_clears_the_stored_rank_on_re_ingest`, which now
passes the previous parse the way `FileSyncController` does: `"priority":
Null` instead of no key). Clearing did work, and `content_differs` compares
`priority()`, so no update was dispatched because of it; the defect is the
stored null key. The keystone cannot see it: `normalize_block`
(`crates/holon-pbt-core/src/block_compare.rs`) strips Null-valued properties
before comparing, so a hand-authored "edit a block without priority" case
passed on both arms before the fix.

The org leg now emits `priority` only when the headline carries one, and
`Value::REMOVED` when `previous` carried one and the file no longer does
(`lane-logs/h1-green-org-store.log`, and the hand-authored cases
`ingest-clears-a-removed-priority-{loro,sqlonly}-arm`). One behavior change:
an ingest with no `previous` (a create, or a block re-parented from another
document's file) no longer clears a stored priority, the same contract as
drawer keys and the task keyword.
