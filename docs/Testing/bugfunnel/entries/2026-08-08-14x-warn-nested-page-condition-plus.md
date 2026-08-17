---
id: 2026-08-08-14x-warn-nested-page-condition-plus
date: 2026-08-08
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  14x WARN `profile condition failed to evaluate on PRESENT columns (type
  mismatch / non-bool result) — treated as non-match, this variant is
  DEGRADED` for the nested-page condition `is_page_row && (!is_def_var("role")
  | | role != "page_title")`, plus both `has_query_source && …` variants on
  that row shape — 7 of the 13 shipped conditions are reachable by the class
  (1/3/5 degrade depending on which columns the row carries).
source_line: 751
---

## Bug

(task #44 lane, live GPUI run) **14x WARN `profile condition failed to
evaluate on PRESENT columns (type mismatch / non-bool result) — treated as
non-match, this variant is DEGRADED` for the nested-page condition
`is_page_row && (!is_def_var("role") | | role != "page_title")`, plus both
`has_query_source && …` variants on that row shape — 7 of the 13 shipped
conditions are reachable by the class (1/3/5 degrade depending on which
columns the row carries).** Type-aware binding encodes "unbound computed
field" as ABSENCE from the Rhai scope, but the enrichment boundary writes
`Value::Null` for such fields back into the row
(`crates/holon-api/src/computed.rs:141`) and the render seat re-seeded scope
from that row (`crates/holon-api/src/entity_profile.rs:269`), so the absence
came back as `()` and rhai failed `() && …` with `Data type incorrect: ()
(expecting bool)`. A lossy enrich→render round trip, not a wrong condition
and not SQLite integer-as-bool.

## Root cause

task #44 lane, found by a dogfood/live GPUI run: **14x WARN `profile
condition failed to evaluate on PRESENT columns (type mismatch / non-bool
result) — treated as non-match, this variant is DEGRADED` for the
nested-page condition `is_page_row && (!is_def_var("role") || role !=
"page_title")`.** The mismatch is NOT in the condition and NOT in SQLite
integer-as-bool: type-aware binding represents an UNBOUND computed field as
ABSENCE from the Rhai scope, but the enrichment boundary writes
`Value::Null` for those fields back INTO the row
(`resolve_computed_fields_with_scope`,
`crates/holon-api/src/computed.rs:141`, keeping the row shape by contract),
and the render seat then seeded scope from that row
(`EntityProfile::build_scope`,
`crates/holon-api/src/entity_profile.rs:269`), turning the typed absence
into a `()` binding — rhai then fails `() && …` with `Data type incorrect:
() (expecting bool)`. So the escape is a LOSSY enrich→render round trip, and
it is a CLASS defect: 7 of the 13 shipped conditions are reachable by it,
and how many degrade depends on which columns the row carries — measured
with a faithful engine, 1 on a plain row (`is_page_row && …`), 3 on the
reported sighting's shape (adds both `has_query_source && …`), 5 when
`content_type` is genuinely absent (adds `is_program`, `is_holon_source`,
`is_image`, `is_source && !is_program`). Rows whose columns are
present-but-NULL — the real SQL shape — resolve cleanly either way.
ENVIRONMENT: `just keystone-smoke` at base emits ZERO of these warns — the
headless keystone never feeds an already-enriched row back through profile
resolution, so the failing code path does not exist in its wiring; secondary
ORACLE, no invariant reads a DEGRADED-condition WARN as a red
(`inv-no-observed-errors` matches ERROR only). FIXED in-lane: `build_scope`
skips row keys that name one of the profile's own computed fields — the
computed pass is their sole authority in scope and recomputing is
idempotent. Fail-loud preserved: a computed field that genuinely errors on
PRESENT columns still lands as `()` and still degrades LOUDLY. Red-first
`crates/holon-profiles/tests/computed_null_round_trip.rs` (5 tests over 11
row shapes incl. adversarial ones — extra unknown keys, and a row key
shadowing a computed field with a WRONG stale value, which must lose to
render-time recomputation; mutation-proven: reverting the skip reds exactly
2). Evidence `lane-logs/44-red-faithful-engine.log` / `44-amend-green.log`.)

## Missing piece

ENVIRONMENT: `just keystone-smoke` at base emits ZERO of these warns — the
headless keystone never re-resolves an already-enriched row, so the failing
path is absent from its wiring. Missing piece = a rung (or a keystone
assertion) that resolves profiles over the rows `ui_watcher::enrich_row`
actually produces. ORACLE residual: no invariant treats a DEGRADED
profile-condition WARN as a red — `inv-no-observed-errors` matches ERROR
only, which is why 14 sightings were invisible to every gate.

## Remedy

**FIXED in-lane 2026-08-08 (task #44).** `EntityProfile::build_scope` skips
row keys naming one of the profile's own computed fields: the computed pass
is their sole authority in scope, and recomputing is idempotent, so dropping
the row's copy loses nothing. Fail-loud is untouched — a computed field that
errors on columns that ARE present still lands in scope as `()` and still
degrades LOUDLY (pinned by `a_genuine_type_error_still_degrades_loudly`).
Red-first `crates/holon-profiles/tests/computed_null_round_trip.rs`: 5 tests
incl. a sweep over every shipped block condition x 11 row shapes (collecting
across all shapes, so a regression prints the whole class inventory) and a
stale-shadowing-value test pinning that render-time recomputation wins;
mutation-proven (removing the skip reds exactly the 2 round-trip tests).
Gates: fmt clean; holon-api+holon-profiles 527/527; keystone-smoke 4/0 with
0 DEGRADED warns; hand-authored 9/9 (40 cases); `cargo check -p holon-gpui`
clean. Residual, disclosed: the ORACLE half is NOT closed — a future
DEGRADED condition still passes every gate silently.
