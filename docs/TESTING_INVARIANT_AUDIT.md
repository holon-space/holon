# Invariant dependency audit (Phase 0/C)

Source: `crates/holon-integration-tests/src/pbt/sut.rs` — 25 distinct invariants identified by `[inv-*]` labels.

**Subsystems** (the dimensions a narrow PBT's SUT either supplies or doesn't):
- `block-tree` — block + tree CRDT state in memory
- `loro` — LoroSyncController + Loro doc (writes + observers)
- `turso-projection` — Turso storage + matviews (CDC fan-out)
- `cdc` — CDC stream observation specifically (sub-aspect of `turso-projection` but called out when an invariant depends on stream ordering, not just final state)
- `viewmodel` — ReactiveEngine ViewModel tree resolution
- `renderer` — render-expr → ViewModel pipeline (a sub-step of `viewmodel` called out when the renderer specifically is the SUT)
- `editor-state` — `InputState` + active editor mirror
- `frontend-bounds` — real GPUI/TUI render window with `BoundsRegistry`
- `driver` — `UserDriver` impl (synthetic interaction)

## Invariants by min-SUT set

### 1-subsystem (11 invariants — each trivially included by any T1 SUT containing the subsystem)

| Invariant | Min-SUT |
|---|---|
| `inv-frontend-bounds-rendered` | `{frontend-bounds}` |
| `inv-loro-no-errors` | `{loro}` |
| `inv-matview-consistent-with-ref` | `{turso-projection}` |
| `inv-sql-budget` | `{turso-projection}` |
| `inv-value-fn-provider-arg-variance-13` | `{viewmodel}` |
| `inv-value-fn-provider-identity` | `{viewmodel}` |
| `inv-viewmodel-editable-text-triggers` | `{viewmodel}` |
| `inv-viewmodel-no-error-widgets` | `{viewmodel}` |
| `inv-viewmodel-snapshot` | `{viewmodel}` |
| `inv-viewmodel-tree-virtual-slots` | `{viewmodel}` |
| `inv-frontend-root-not-error` | `{viewmodel}` |

### 2-subsystem (12 invariants)

| Invariant | Min-SUT |
|---|---|
| `inv-backend-blocks-match-ref` | `{loro, turso-projection}` |
| `inv-editable-text-has-draggable` | `{viewmodel, frontend-bounds}` |
| `inv-focus-matches-ref` | `{driver, editor-state}` |
| `inv-focus-roots` | `{turso-projection, cdc}` |
| `inv-frontend-engine` | `{viewmodel, frontend-bounds}` |
| `inv-frontend-no-error-widgets` | `{viewmodel, frontend-bounds}` |
| `inv-live-children-match-ref` | `{block-tree, loro}` |
| `inv-viewmodel-decompiled-rows-match-query` | `{viewmodel, turso-projection}` |
| `inv-viewmodel-entity-ids-subset-of-data` | `{viewmodel, turso-projection}` |
| `inv-viewmodel-root-matches-render-expr` | `{viewmodel, renderer}` |
| `inv-viewmodel-state-toggle-correct` | `{viewmodel, block-tree}` |
| `inv-watch-rows-match-ref` | `{turso-projection, cdc}` |

### 3+-subsystem (2 invariants)

| Invariant | Min-SUT |
|---|---|
| `inv-displayed-text` | `{editor-state, viewmodel, frontend-bounds}` |
| `inv-org-render-fixed-point` | `{block-tree, renderer, loro \| turso-projection}` |

## Gate verdict

The plan's Phase 0 gate: **if ≥30% of invariants need ≥3 subsystems, switch from "narrow PBT excludes invariants by subsystem" to "each invariant declares an explicit min-SUT set and the registry checks set inclusion."**

- 3+-subsystem invariants: **2 / 25 = 8%.**
- **PASSES** the 30% threshold by a wide margin.

The subset-by-subsystem architecture proposed in Phase 3 is viable as-is — no redesign needed. The registry should still record the min-SUT set explicitly (it's cheap and prevents drift), but Phase 3 doesn't need to change shape.

## Phase-by-phase invariant coverage (which T1 PBT picks up which invariants)

Predicting what each narrow T1 PBT's SUT supplies, and which invariants the registry will include:

### Phase 4 — block-tree org round-trip
SUT: `{block-tree, renderer}` (pure functions; no Loro, no Turso, no ViewModel).
Invariants picked up: `inv-org-render-fixed-point` (partial — needs to land an in-memory shadow of `block-tree → render-expr → re-parse`).
Coverage: 1/25 = 4%. Low but expected — round-trip PBT exists to pin one specific property.

### Phase 5 (T1 component) — editor + Loro
SUT: `{block-tree, loro, editor-state, viewmodel}` (no Turso, no frontend-bounds, no driver).
Invariants picked up: `inv-loro-no-errors`, `inv-live-children-match-ref`, `inv-viewmodel-state-toggle-correct`, `inv-viewmodel-editable-text-triggers`, `inv-viewmodel-no-error-widgets`, `inv-viewmodel-snapshot`, `inv-viewmodel-root-matches-render-expr`, `inv-viewmodel-tree-virtual-slots`, `inv-frontend-root-not-error`, `inv-value-fn-provider-identity`, `inv-value-fn-provider-arg-variance-13`.
Coverage: 11/25 = 44%. **Phase 5 is doing real load-bearing work.**

### Phase 5 (T0 component) — pure MutableTree + cursor
SUT: `{block-tree, editor-state}`.
Invariants picked up: none of the labelled inv-* set — it asserts pure-logic invariants (cursor monotonicity, trim, structural integrity) that aren't in the wide-PBT registry today. **Will need to register new T0-only invariants.**

### Phase 6 — BlockCellRegistry routing
SUT: `{block-tree, loro, turso-projection, cdc}` (no ViewModel, no frontend, no driver).
Invariants picked up: `inv-loro-no-errors`, `inv-backend-blocks-match-ref`, `inv-live-children-match-ref`, `inv-matview-consistent-with-ref`, `inv-focus-roots`, `inv-watch-rows-match-ref`, `inv-sql-budget`.
Coverage: 7/25 = 28%. **Highest absolute Turso-side coverage of any T1 PBT.** Confirms Phase 6 is the right home for the borderline cases flagged in `TESTING_BACKTEST.md`. The matview invariants (`inv-matview-consistent-with-ref`, `inv-focus-roots`) are exactly the ones gated on the upstream Turso fix.

### Phase 7 — SqlOperationProvider + event-bus
SUT: `{turso-projection}` (no Loro, no block-tree).
Invariants picked up: `inv-matview-consistent-with-ref`, `inv-sql-budget`, possibly `inv-focus-roots`/`inv-watch-rows-match-ref` if the SUT exercises matviews end-to-end.
Coverage: 2-4/25 ≈ 8-16%. Confirms `TESTING_BACKTEST.md`'s finding that Phase 7 is the weakest of the proposed T1 PBTs — limited invariant reach *and* zero historical bug demand.

### Phase 9a — render-DSL / view-model resolution
SUT: `{viewmodel, renderer, block-tree}` (no Loro, no Turso, no editor).
Invariants picked up: `inv-viewmodel-root-matches-render-expr`, `inv-viewmodel-no-error-widgets`, `inv-viewmodel-snapshot`, `inv-viewmodel-tree-virtual-slots`, `inv-viewmodel-entity-ids-subset-of-data`, `inv-viewmodel-decompiled-rows-match-query` (the data-subset assertions need a stub data layer), `inv-frontend-root-not-error`.
Coverage: 6-7/25 = 24-28%.

### Phase 9b — reactive-engine-only
SUT: `{viewmodel}` (engine + in-memory store; no Loro, no Turso, no renderer).
Invariants picked up: `inv-viewmodel-no-error-widgets`, `inv-viewmodel-snapshot`, `inv-viewmodel-tree-virtual-slots`, `inv-value-fn-provider-identity`, `inv-value-fn-provider-arg-variance-13`.
Coverage: 5/25 = 20%.

### Wide PBTs (reference)
- `general_e2e_pbt` (no frontend-bounds): all except `inv-displayed-text`, `inv-editable-text-has-draggable`, `inv-frontend-engine`, `inv-frontend-no-error-widgets`, `inv-frontend-bounds-rendered`. **20/25 = 80%.**
- `gpui_ui_pbt` (with frontend-bounds): **25/25 = 100%.**

## Findings

1. **Subset architecture is sound** — 92% of invariants need ≤2 subsystems, so a registry that filters by min-SUT-set inclusion is a clean fit.
2. **Phase 5 T1 alone covers 44% of invariants** — single largest narrow-PBT contributor, justifies prioritising it.
3. **Phase 7 covers 8-16%** — confirms the `TESTING_BACKTEST.md` recommendation to demote.
4. **`inv-displayed-text` and `inv-org-render-fixed-point` straddle three subsystems each** — these will stay in the wide PBTs and possibly the T1 PBTs whose SUT happens to span the set. Acceptable: they're invariants whose *value proposition* is precisely the cross-subsystem check.
5. **T0 MutableTree proptest needs new invariant labels** — none of today's `inv-*` set is small enough. Pre-register a `inv-tree-cursor-*` family in Phase 3's registry to slot them.
6. **Three invariants today are warn-only** (`[inv-backend-blocks-match-ref WARN]`, `[inv-watch-rows-match-ref WARN]`, `[inv-focus-roots WARN]`) — they downgrade to a log line under CDC-lag conditions. The registry must preserve this warn/error distinction; a strict-only enforcement would re-introduce the flakes those WARN paths were added to handle. Document explicitly in Phase 3.
