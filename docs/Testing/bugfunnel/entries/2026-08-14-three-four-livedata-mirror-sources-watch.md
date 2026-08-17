---
id: 2026-08-14-three-four-livedata-mirror-sources-watch
date: 2026-08-14
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Three of the four LiveData mirror sources watch projections whose SELECT
  list omits `_change_origin`, so their CDC batches structurally cannot name a
  writer
source_line: 708
---

## Bug

(task-#27 org-origin lane; found by the task-#15 verification, which
measured every `live_data.apply_batch` as an unparented root with zero links
in 7/7 runs) **Three of the four LiveData mirror sources watch projections
whose SELECT list omits `_change_origin`, so their CDC batches structurally
cannot name a writer** — `focus_roots` (`turso_block_query_source.rs:134`
over `matview_focus_roots.sql`), `entity_keyed` (`registration.rs:341-347`)
and `entity_profile` (`sql/profiles/get_profiles.sql`); only `block`
(`SELECT *`) carries it. `extract_change_origin_from_data`
(`holon-turso/src/turso.rs:978-987`) then collapses "the projection never
selected the column" into the same silent `Remote { None, None }` as a
genuinely remote change. Not a writer defect: the loss is in the projection,
by design but undisclosed.

## Root cause

task-#27 org-origin lane, found by the task-#15 verification — 7/7 runs
showed every `live_data.apply_batch` as an unparented root with zero links,
and the source-blind ROOT assert flaked 1/6 → 3/4 before being deleted:
**three of the four LiveData mirror sources watch projections whose SELECT
list omits `_change_origin`, so their CDC batches structurally cannot name a
writer**, and `extract_change_origin_from_data`
(`crates/holon-turso/src/turso.rs:978-987`) collapses "the projection never
selected the column" into the same silent `Remote { operation_id: None,
trace_id: None }` it uses for a genuinely remote change. Measured by reading
all four registration sites: `focus_roots` watches `SELECT region, root_id
FROM focus_roots` (`crates/holon/src/sync/turso_block_query_source.rs:134`)
over a matview projecting only `(region, root_id, added_ts, history_id)`
(`crates/holon-turso/sql/schema/matview_focus_roots.sql`); `entity_keyed`
watches `SELECT id, parent_id, source_language FROM <block>`
(`crates/holon/src/di/registration.rs:341-347`); `entity_profile` watches
`SELECT id, content FROM block WHERE …`
(`crates/holon/sql/profiles/get_profiles.sql`). Only the `block` source
carries it, via `SELECT *`
(`crates/holon/src/sync/event_infra_module.rs:95`,
`turso_block_query_source.rs:109`). NOT a writer defect — no writer emits
origin-less rows on `block`; the loss is in the PROJECTION and is by design
but UNDISCLOSED. ORACLE because the interaction is generatable and the path
runs in the keystone's own wiring: what was missing is an invariant that
separates "this mirror CANNOT be attributed" from "this mirror's attribution
BROKE", which the source-blind assert could not do, so it flaked and was
deleted rather than fixed. CLOSED at the oracle:
`interaction_trace_connectivity.rs` now asserts parenthood for the `block`
source only, and `crates/holon-turso/tests/cdc_batch_trace_links.rs` pins
both directions at the projection level. Widening the other three SELECT
lists is NOT done — adding a column to a matview for tracing alone is a
separate decision.)

## Missing piece

No invariant separated "this mirror CANNOT be attributed" from "this
mirror's attribution BROKE"; the source-blind ROOT assert could not tell
them apart, so it flaked 3/4 and was deleted instead of fixed.

## Remedy

CLOSED at the oracle — `interaction_trace_connectivity.rs` asserts
parenthood for the `block` source only, and
`holon-turso/tests/cdc_batch_trace_links.rs` pins both directions at the
projection level. Widening the other three SELECT lists is deliberately NOT
done.
