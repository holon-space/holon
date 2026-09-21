---
id: 2026-09-21-loro-path-drops-completed-and-block-type-before-sql
date: 2026-09-21
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  Every write of the `completed` or `block_type` block column is dropped
  between the Loro authority and SQL, so both columns hold their schema
  default forever on the CRDT path — losing a template instance's
  `block_type` and with it every advice rule anchored on that entity.
---

## Bug

Authoring either column through the production facade leaves `block_raw` at
the schema default. Measured by a new probe,
`crates/holon-integration-tests/tests/loro_suite/loro_completed_and_block_type_reach_sql.rs`
(log `lane-logs/fc-red.log:40-42` in lane `f1a-read-model`), which drives two
legs through the same `HolonService` the MCP server builds — one authoring
both columns at `block.create`, one writing them afterwards through
`block.set_field`:

```
the Loro path dropped 4 of 4 authored column writes before SQL:
  create.block_type, create.completed, set_field.block_type, set_field.completed
observed: create: block_type=Some(String("text")) completed=Some(Integer(0))
        · set_field: block_type=Some(String("text")) completed=Some(Integer(0))
```

The probe asserts `content` — a column the projection does emit — arrived on
both legs, so these are drops, not writes that never happened.

Found by agent code-reading during lane `f1a-read-model` (the UI read model
had to state which columns it can carry), then measured.

## Root cause

The write reaches the authority and is discarded on the way out.

1. `set_field("completed"|"block_type")` falls into the `_` scalar arm and
   writes a `LoroMetaCellBacking` cell —
   `crates/holon-loro/src/block_cell_registry.rs:1045-1057`.
2. `read_properties_from_meta` removes both keys again via
   `RESERVED_PROPERTY_KEYS` — `crates/holon-loro/src/loro_backend.rs:458`,
   list at `:476-491`. That list deliberately EXCLUDES
   `collapsed`/`widget_only` *because* those are lifted into typed `Block`
   slots (`:471-475`); these two have no slot to be lifted into
   (`crates/holon-api/src/block.rs:294-384`), so the strip is terminal.
3. `block_to_params` (`crates/holon-loro/src/loro_sync_controller.rs:2222-2283`)
   emits `collapsed` and `widget_only` explicitly and names neither of these
   two, so the INSERT/UPDATE it builds never mentions the columns. They stay
   at `completed INTEGER NOT NULL DEFAULT 0` / `block_type TEXT NOT NULL
   DEFAULT 'text'` (`crates/holon-turso/sql/schema/blocks.sql:21-22`).

Nothing is lost at a reseed — the projection never names the column, so a
reseed cannot clobber it. The loss is at the write, and it is permanent.

Reachable consequence: `plan_instantiation` puts `block_type` into its create
params (`crates/holon-api/src/template_instantiation.rs:383`) and
`OperationEngine` fans those out as ordinary `block.create` ops
(`crates/holon/src/api/operation_engine.rs:1044-1071`) — the exact leg
measured. A template instance therefore lands as `block_type='text'`, and
`AnchorSelector::Entity(name)`, which lowers to
`block_raw.block_type = '<name>'` (`crates/holon-advice/src/lowering.rs:114`),
silently stops matching it.

`completed` is not what marks a task DONE — the widget path writes the
`task_state`/`task_state_category` property pair
(`block_cell_registry.rs:990-1017`), which does travel. The column itself is
unreachable and reads 0 forever, while remaining live in the read direction
(`journal_feed_matview.sql:29`, `journal_day_pages_matview.sql:29`,
`crates/holon-app/src/turso_seams.rs:73`, and the render-DSL checkbox binding
shape `checked: { column: completed }`, `assets/mock_data.yaml:30`).

## Missing piece

COVERAGE. No test asserted that an authored block COLUMN survives the CRDT
path end to end. The sibling pin
`loro_suite/loro_kind_fidelity_through_projection.rs` does exactly this shape
for `properties`/`property_kinds`, and the same shape for these two columns
simply did not exist. `docs/Architecture/BlockEventStorm.md:329` records the
non-round-trip under "H1-residue hardening" — as a tidiness item, not as
data loss on the write path, which is why it never grew a test.

The keystone PBT cannot see it: its oracle compares the store against its own
model of what the store holds, and the model was built from the same
projection, so a column neither side carries agrees vacuously.

## Remedy

Not fixed in lane `f1a-read-model` (out of scope; the lane added only the
probe, which is the red). The fix is a ruling, not a patch:

- `block_type` — give `Block` a typed slot, drop the key from
  `RESERVED_PROPERTY_KEYS`, and emit it in `block_to_params`, following the
  `collapsed`/`widget_only` precedent in the same three files. It has a live
  SQL reader (advice anchors), so this is the recommended direction.
- `completed` — decide whether the column is alive at all. Its only clear
  binding today is a checkbox in mock data; if it is dead, delete the column
  with its readers rather than wiring it.

The probe stays red until one of those lands.
