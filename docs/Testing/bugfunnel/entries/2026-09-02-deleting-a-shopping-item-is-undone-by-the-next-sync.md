---
id: 2026-09-02-deleting-a-shopping-item-is-undone-by-the-next-sync
date: 2026-09-02
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  The generic `delete` hard-deletes a `shopping_item` row instead of setting
  its `deleted_at` tombstone, so the deletion is never pushed to the peer and
  the next sync resurrects the item.
---

## Bug

Found dogfooding the kitchen feature against a local stand-in for the
shopping-list peer (lane `kitchen-dogfood`). Deleting a pulled item, then
syncing, silently undoes the delete:

```
execute_operation shopping-item/delete {"id":"shopping:Trocken:Spaghetti"}
  -> executed successfully
SELECT id,name,deleted_at FROM shopping_item;
  -> Milch (null), Zucker (null)          # Spaghetti gone, no tombstone

execute_operation shopping-item/shopping_sync {}
  -> executed successfully
SELECT id,name,deleted_at FROM shopping_item;
  -> Milch, Spaghetti, Zucker             # back
```

The peer was never told. Its request log for that round shows only the GET; no
`/commit` was ever posted and its version stayed at 7. So a person who removes
an item in Holon watches it reappear within one sync round, and the item is
still on the shared list every other device sees.

## Root cause

`crates/holon-kitchen/assets/types/shopping_item.yaml` declares `deleted_at`
with an explicit contract: "Local tombstone: set on a local delete so the next
pull cannot resurrect the item before the deletion has been pushed." The
generic `delete` operation removes the row instead, so the tombstone is never
written.

Both halves of the design then fail together, because both read the tombstone.
The reconciler in `crates/holon-kitchen/src/shopping.rs` derives a `del` push
intent from a tombstoned row, and derives resurrection protection from the same
column; with the row simply gone there is nothing to push and nothing to
protect, and absence-as-deletion runs the other way — the peer's complete
snapshot re-creates it.

## Missing piece

No test deletes a `shopping_item` through the dispatcher.
`crates/holon-kitchen/tests/shopping_sync_pbt.rs` generates local delete
INTENTS directly against the reconciler, so it proves the reconciler handles a
tombstoned row and never learns that nothing produces one.
`crates/holon-app/tests/shopping_pull_mock.rs` has the same shape. The escape
sits precisely in the hop the PBT starts after.

## Remedy

OPEN. Fix is that `shopping-item/delete` writes `deleted_at` rather than
removing the row — either a type-level soft-delete declaration, or a bespoke
delete on the shopping provider beside `shopping_sync`
(`crates/holon-app/src/shopping_operations.rs`). Note the ordering constraint:
the row may only be reaped after a commit the peer acknowledged, or the
deletion is lost again.

The closing test drives the full round — dispatcher delete, then `sync_once`
against a mock peer, asserting a `del` command was sent and the item did not
come back. It goes red today at the first assertion. This is a sibling of
[[2026-09-02-a-shopping-item-can-never-be-added-in-holon]]: both are the
authoring door, which no test opens.
