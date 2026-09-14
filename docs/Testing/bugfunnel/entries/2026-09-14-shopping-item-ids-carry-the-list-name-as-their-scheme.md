---
id: 2026-09-14-shopping-item-ids-carry-the-list-name-as-their-scheme
date: 2026-09-14
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  `shopping_item` rows are keyed `shopping:<cat>:<name>`, so the id's scheme
  names the LIST the row belongs to rather than the entity it is a row of.
---

## Bug

`crates/holon/tests/shopping_authoring_door_e2e.rs`
`deleting_a_shopping_item_is_pushed_to_the_peer_and_does_not_come_back` now
fails at the operation boundary:

```
Error: Operation 'delete' on entity 'shopping-item' failed: operation boundary:
parameter 'id' of 'shopping-item/delete' carries "shopping:Trocken:Spaghetti",
which names the entity 'shopping' — this parameter references a
'shopping-item', so the value must be spelled "shopping-item:<id>".
```

Log: `lane-logs/nextest-core-1789407794.log` (entity-uri-boundary workspace).

## Root cause

The row id is derived in two places that agree with each other and with
nothing else: `crates/holon-kitchen/src/shopping.rs:172` (`row_id`) and the
sidecar mapping `assets/integrations/shopping.yaml:251`, both building
`"shopping:" + cat + ":" + name`. The entity is `shopping_item`, which
normalizes to the scheme `shopping-item`; the leading `shopping` is the LIST
name, and the row already carries it in its own `list` column
(`owner_value: "shopping"`).

That first segment then reads as a URI scheme, which is the exact shape
`holon_rows::parse_local_id` refuses for file-derived ids — "it already reads
as a schemed URI, so it is stored unprefixed and every reference to it joins to
nothing". Connector-derived rows never pass through that parse, so the shape
reached storage unchallenged.

The rows do join today, because every leg builds the same string. What is wrong
is that the id does not name what it is a row of, so nothing downstream can
tell a `shopping_item` reference from a reference to some entity called
`shopping`.

## Missing piece

An ORACLE gap: the shopping door tests assert the peer round-trip and the row
contents, and no assertion anywhere said an entity's stored ids must carry that
entity's scheme. The rule existed only as a convention inside
`EntityUri::from_raw_for`, which the connector path does not call. D125.a made
the operation boundary compare a reference's scheme against the entity its
parameter was declared for, which is what turned the convention into a check
and surfaced this.

COVERAGE secondary: the connector typed-row path has no equivalent of the
file-format adapter's `parse_local_id`, so a mis-schemed id from a sidecar
mapping is not refused where it is produced.

## Exposure

No user-visible breakage today: writes and reads agree on the string, so the
shopping list works. The cost is that `shopping_item` ids are indistinguishable
from references to a non-existent `shopping` entity, which blocks the boundary
check for this entity and would mis-route any future code that resolves a
reference by its scheme.

## Remedy

Open — needs a ruling, and it touches a file another lane owns
(`assets/integrations/shopping.yaml`, lane `lowcode-inc5`).

1. Key the rows `shopping-item:<cat>:<name>` — change `row_id`, the sidecar
   mapping, and the door test's literal. The mirror table is re-derived by
   `replace_typed_rows` on each sync, so stored rows heal on the next round.
2. Route connector-derived typed rows through `holon_rows::parse_local_id` as
   the file-format adapter now does, so a mis-schemed id is refused where it is
   produced rather than at the operation boundary.

Recommendation: both, (1) first. Not done here: it is outside D125.a and the
sidecar file is contended.

**FIXED by lane `lowcode-inc5`** (2026-09-15): `assets/integrations/shopping.yaml`
now keys the rows `shopping-item:<cat>:<name>` with `@uri`-escaped segments, and
the bespoke `crates/holon-kitchen/src/shopping.rs` that carried the second copy
of the derivation is deleted. Remedy (2) — routing connector typed rows through
`holon_rows::parse_local_id` — is NOT done, so a sidecar mapping can still emit
a mis-schemed id; the operation boundary refuses it at dispatch rather than at
the mapping.
