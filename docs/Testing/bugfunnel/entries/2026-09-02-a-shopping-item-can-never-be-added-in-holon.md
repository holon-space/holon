---
id: 2026-09-02-a-shopping-item-can-never-be-added-in-holon
date: 2026-09-02
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  `shopping_item` declares no `properties` overflow column, so the engine's
  `_provenance` stamp has nowhere to land and every authoring `create` is
  refused — the user cannot add anything to the shopping list.
---

## Bug

Found dogfooding the kitchen feature against a local stand-in for the
shopping-list peer (lane `kitchen-dogfood`). After a successful pull, adding an
ingredient from a recipe to the list through the declared type's own generic
authority is refused outright:

```
execute_operation shopping-item/create
  {"id":"shopping:Fleisch:Guanciale","name":"Guanciale","cat":"Fleisch",
   "count":1,"checked":0}

Operation 'create' on entity 'shopping-item' failed: shopping-item: field
'_provenance' is not a column of `shopping_item_raw` and `shopping-item`
declares no `properties` overflow column, so this write has nowhere to land.
Declared columns: ["cat","checked","count","deleted_at","id",
"last_seen_remote","name","product_id"].
```

The pull leg is unaffected — three remote items landed correctly, with
categories and the checked flag from `pickedItems`. It is the AUTHORING leg
that is dead: nothing a person does in Holon can put an item on the list. The
commit leg works when a row is inserted below the dispatcher, so the sync
engine itself is sound (verified: a locally added `Guanciale` produced
`{"cmd":"add","good":{"name":"Guanciale","cat":"Fleisch","new":true},"id":…}`,
the peer's version advanced 7→8, and the verifying re-pull reconciled).

## Root cause

`crates/holon-kitchen/assets/types/shopping_item.yaml` declares no `properties`
/ `property_kinds` fields. Every other kitchen type declares both, and each
carries the same comment explaining why: "The overflow bag every authoring
write needs: the engine stamps `_provenance` onto each `create`, and a type
with nowhere to put it is refused at the write boundary rather than written
without its origin." `recipe.yaml`, `ingredient_use.yaml` and
`pantry_item.yaml` all have it; `shopping_item.yaml` was written without it.

The refusal is correct behaviour — the boundary is doing precisely what the
comment promises. The defect is the missing pair of columns.

## Missing piece

Nothing exercises `shopping_item` through the AUTHORING door.
`crates/holon-app/tests/shopping_pull_mock.rs` and
`crates/holon-kitchen/tests/shopping_sync_pbt.rs` both drive the reconciler and
the peer, and both supply local rows directly rather than through
`execute_operation`. The keystone PBT has no shopping transition. So every test
reaches `shopping_item` by a path that never stamps `_provenance`.

## Remedy

OPEN. Fix is to add `properties` (TEXT, nullable, jsonb) and `property_kinds`
(TEXT, nullable) to `shopping_item.yaml`, matching the three sibling types
verbatim. The closing test is a `shopping-item/create` through the dispatcher —
red today with the message quoted above. An architecture-test sweep asserting
that EVERY declared type carrying authoring operations has the overflow pair
would stop the next one, and is the better investment: this is a
one-type-forgot-it defect, and nothing but a person's memory currently prevents
it.
