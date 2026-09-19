---
id: 2026-09-19-gpui-fixtures-assert-pre-d142a-raw-row-ids
date: 2026-09-19
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  31 holon-gpui windowed tests failed because their fixtures minted bare row
  ids and probed the bounds registry for the bare form, which D142.a now
  normalises to `block:<id>` — no product defect behind any of them.
---

## Bug

Found by lane `fix-gpui-bounds-entity-id` while root-causing the 42
`holon-gpui` reds at integration tip `496a42c7`. 31 of them share one cause
and none of them is a product defect.

The representative panic accuses production outright:

```
item 0 missing from BoundsRegistry after initial paint — entity_id wiring in
production text builder is broken. entity_ids in registry:
["block:test-item-10", "block:test-item-33", …, "block:test-item-0", …]
```

`block:test-item-0` is right there in the list the message prints. Every item
was recorded. The test probes the registry for the bare `test-item-0`, which
matches nothing, and the diagnostic's own dump refutes its accusation.

The message is a false alarm and so are its siblings: `collection_view()
collapsed to 0 height inside the stacking column and painted no rows`,
`the sidebar must render page rows at all`, `first row must be painted
(registry rows: [])`. In each case the "no rows" is a bare-prefix filter over
a registry whose keys are schemed, not an empty registry.

## Root cause

D142.a made the row's `id` column a parsed value. A bare, unschemed id is not
refused — `EntityUri::try_from_raw` normalises it to `block:<id>` — so the
render binding records the canonical URI as the row's `entity_id`. That is the
correct and intended behaviour: the only production reader is
`GpuiUserDriver`, which is handed an `EntityUri` and queries with
`as_str()`, i.e. the canonical form. Production rows already carry their
scheme by the time they reach the frontend (org files store bare ids and the
parser schemes them at the boundary), so the normalisation is a no-op in
production and nothing user-facing changed.

The fixtures were the half that was never production-faithful. They minted
`test-item-0`, `outline-3`, `page-50`, `frow-0`, `band-row-07`, `sib` into the
`id` column and then probed the registry for those same bare strings. Before
D142.a the raw text round-tripped and the probes matched; after it the stored
key is canonical and the bare probe cannot match. One test in the set
(`panel_scroll_spike`) already converted its bare fixture id through
`EntityUri::from_raw` before handing it to a production API — the fixture
half had simply never been brought along.

## Missing piece

Nothing. `gap: FALSE-ALARM` — these tests asserted a representation the
product deliberately stopped promising at D142.a, and the assertions were not
updated with it.

Worth recording rather than fixing silently, for one reason: the failure
messages actively misdirect. Three of them name a specific production
component as broken, and a reader who trusts the prose instead of reading the
dump underneath it starts the investigation at the wrong end. The prose was
written when the bare form was the truth and rotted into misinformation when
the contract moved.

## Remedy

FIXED in lane `fix-gpui-bounds-entity-id`. The fixtures now mint
production-shaped ids (`block:test-item-0`, `block:outline-3`, …) and the
probes and prefix filters match that form, so each test asserts what it always
meant to: the production binding records the row's ENTITY, whatever spelling
the row arrived with. The shared canned row source in
`frontends/gpui/tests/support/mod.rs` was changed at its single mint site, so
`backlink-`, `integration-` and `outline-` rows are schemed for every test
that consumes them.

No production code was touched for these 31. Result: 42 reds → 10, and all 10
remaining are the pre-existing sanctioned entries in the land gate's exclusion
regex (`lane-logs/gpui-after-fix-2.log`).

## Relation to other entries

The 42nd red in the same run WAS a product defect — see
`2026-09-19-condition-row-id-forms-no-uri-loses-row-identity`. The two are
worth reading together: the same classifier change produced one stale-fixture
failure class and one real regression, and the loud accusatory messages all
belonged to the harmless class while the real defect announced itself only as
an empty `Tracked: []`.
