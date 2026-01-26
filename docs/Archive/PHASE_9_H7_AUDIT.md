# Phase 9 H7 audit — DEFER

Phase 9 of `~/.claude/plans/stage-a-ship-with-dynamic-pudding.md` is gated on
H7: a non-Turso `BuilderServices` impl ≤1500 LOC, with matview-required method
count ≤2. Audit performed 2026-05-18 on the `pbt-slicing-doc` worktree.

## Matview-required count (render path)

Production matviews the render path queries:

| Matview | File | Shape |
|---|---|---|
| `block` | `crates/holon/sql/schema/block_matview.sql` | LEFT JOIN `block_raw` × `block_tags` × `block_requires` + `json_group_array` + `GROUP BY` (17 cols) |
| `block_requirement_edges` | `crates/holon/sql/schema/block_requirement_edges_matview.sql` | edge derivation |
| `focus_roots` | `crates/holon/sql/schema/matview_focus_roots.sql` | UNION ALL focus subtree |

**Count: 3** — exceeds the ≤2 gate.

Also-present matviews (not on the critical render path but used by ancillary
flows): `matview_current_focus`, `matview_current_editor_focus`,
`mv_events_global_watermark`, `mv_event_acks_watermark`. Total prod matviews: 7.

## LOC budget estimate

Baseline reference impls:
- `StubBuilderServices` — 67 LOC (defaults only, `start_query` always `Err`)
- `HeadlessBuilderServices` — 73 LOC (delegates to a real Turso `BackendEngine`)

A Phase-9-faithful impl needs to drive a real GPUI render path against
in-memory storage. Methods that cannot be defaulted/stubbed:

| Surface | Why nontrivial | Est LOC |
|---|---|---|
| `start_query` + `watch_live` | emit `Change<DataRow>` from in-memory store with CDC-equivalent reactivity | 400-700 |
| `dispatch_intent` | in-memory op executor for create/set_field/move/delete | 300-500 |
| In-memory matview equivalents | hydrate tags+requires (block matview), compute focus_roots, propagate under mutation | 400-800 |
| `get_block_data` | render expression resolution against in-memory rows | 100-150 |
| `editable_text` | `Cell<String>` over in-memory content with collab semantics | 50-100 |
| `focused_block` + UI state | `Mutable<Option<EntityUri>>` plumbing | 50-100 |
| `resolve_profile` + signal | profile resolver minimal impl | 50-100 |
| Scaffolding + tests | constructor wiring, generator-side setup | 200-400 |
| **Total** | | **1550-2850** |

**Estimate: 1600-2950 LOC** — exceeds the ≤1500 gate.

The matview re-implementation cost is the dominant uncertainty. Reproducing
the `block` matview's `json_group_array + LEFT JOIN + GROUP BY` semantics
*including correct CDC under junction-table mutation* is the same surface
that has produced multiple historical bugs in Turso itself (see MEMORY:
multiset-negative, MatchCounter Uninitialized, focus_roots IVM drop). A
correct in-memory clone is its own project.

## Decision

**Defer Phase 9 to a separate plan**, per the explicit policy in the parent
plan: "If H7 fails (LOC budget too high), defer to a separate plan; the
framework still stands without this slice."

The framework's structural claim is *already validated* by the three existing
slice consumers running today:

1. `editor_pure_pbt` — no storage, no UI, no async runtime — ~2 s/1024 cases
2. `storage_consistency_pbt` — real Turso + Loro, no UI, no driver — ~124 s/16 cases × 1-10 steps
3. `general_e2e_pbt` (wide PBT) — full stack — minutes

These cover the cross-axis claim (storage axis swap, UI axis off). The
missing slice — "real renderer + in-memory storage" — does not test a new
*axis*; it tests a specific *composition* whose value is bounded.

## What a follow-up Phase 9 plan would look like

If Phase 9 is revisited, viable paths:

1. **Real GPUI + real Turso, no router/CDC isolation**: ~100 LOC delta over
   the wide PBT but doesn't validate the in-memory claim.
2. **In-memory blocks + headless reactive renderer**: reuse `StubBuilderServices`
   shape + add live row-change streams; skips real GPUI window. ~600-900 LOC.
   Most pragmatic. Would catch render-pipeline structural bugs but not Turso
   IVM bugs (which the storage slice already covers).
3. **Full in-memory matview re-implementation**: 1500-2500 LOC, only worth
   doing if cross-frontend (Flutter, web) consumers materialize.

Option 2 likely wins. The "real GPUI" portion of the original plan was
aspirational — what we actually need is "renderer agnostic of storage backend",
which a headless `BuilderServices` + WidgetSnapshot already provides.

## Phase 10 (non-optional) proceeds independently

Phase 10 (cleanup + archlint rules + inline invariant body deletion) is not
gated on Phase 9 and is non-optional per the parent plan ("If the cleanup
phase doesn't ship, the framework will silently revert via new code paths").
Proceeding to Phase 10 with this memo as the Phase 9 deferral record.
