# Phase 1 P1.6 — `BuilderServices` LOC audit (H7)

**Goal**: estimate LOC for a non-Turso `BuilderServices` impl (needed for Phase 9's "in-memory blocks + real GPUI" slice).

**Source**: `crates/holon-frontend/src/reactive.rs:41-340` (trait def, ~30 methods).

## Method classification

### Trivial — ~20 methods (~5-10 LOC each, ~150 LOC total)

Methods with a default impl returning a sensible value (empty Mutable, None, false, error), or trivial reads from an in-memory HashMap:

- `clone_arc`, `profile_signal`, `virtual_child_config`, `set_widget_open`, `ui_state`, `viewport_snapshot`, `key_bindings_snapshot`, `focused_block`, `focused_block_mutable`, `provider_cache`, `set_focus`, `editable_text` (stub), `watch_live`, `unwatch`, `watch_query_signal`, `watch_editor_cursor`, `try_runtime_handle`, `widget_state`, `present_op`, `resolve_profile`.

Most have defaults already; an in-memory impl uses them as-is or overrides with trivial logic.

### SQL-required — ~7 methods (~50-150 LOC each, ~400-800 LOC total)

Methods that need query compilation + execution against the data store:

| Method | Complexity | LOC estimate |
|---|---|---:|
| `interpret(expr, ctx)` | Pure logic, no SQL — actually TRIVIAL on second look | 20 |
| `get_block_data(id)` | Look up block + assemble DataRows | 80 |
| `compile_to_sql(query, lang)` | Reuse `holon`'s compiler | 30 (delegation) |
| `start_query(...)` | Compile + execute + return rows | 150 |
| `dispatch_intent(intent)` | Apply mutation to in-memory store | 200 (per-op handling) |
| `dispatch_intent_sync(...)` | Sync variant of above | 50 (shares impl) |
| `snapshot_resolved(id)` | Combines interpret + get_block_data + traversal | 100 |
| `popup_query(...)` | Popup-specific query | 80 |

Subtotal: **~710 LOC** if we reuse `compile_to_sql` from `holon`. ~1500+ LOC if we drop SQL entirely and reimplement query exec.

### CDC-required — ~3 methods (~150 LOC each, ~450 LOC total)

Methods that emit row-change streams:

| Method | Complexity | LOC estimate |
|---|---|---:|
| `watch_block_signal(...)` | Subscribe to block changes; emit on mutation | 200 |
| `await_ready(...)` | Wait for initial population | 50 |
| `runtime_handle()` | Provide tokio handle for async work | 10 |

Subtotal: **~260 LOC**. An in-memory CDC impl uses tokio channels — simpler than Loro/Turso CDC.

### Matview-required — 0 methods

No `BuilderServices` method directly queries a matview. Matview access flows through `start_query` (which compiles ARB SQL/PRQL/GQL). If the in-memory store implements `compile_to_sql` + a small SQL exec layer over its data, it gets matview-free query support automatically. Phase 9 gate (matview-required count ≤2): **PASS, count = 0**.

## Aggregate estimate

| Bucket | LOC estimate |
|---|---:|
| Trivial | 150 |
| SQL-required (reuse compiler) | 710 |
| CDC-required | 260 |
| Misc glue + tests | 200 |
| **Total** | **~1320 LOC** |

**Verdict — H7 PASS, with margin to the 1500 ceiling.**

Margin notes:
- Reusing `holon`'s SQL compiler is essential for the budget. Without it, `compile_to_sql` + `start_query` together cost 800-1200 LOC instead of 180.
- If we drop the `compile_to_sql` requirement (i.e. the in-memory store doesn't compile queries — it directly traverses the in-memory tree per `BuilderServices` method), the budget drops to ~600 LOC but the slice loses query-bearing block coverage. That's exactly what `SutQueryCompile` (Phase 6g) is for — the in-memory slice's generators don't propose query-bearing blocks, so query exec isn't strictly needed.

**Recommendation**: build the Phase 9 in-memory impl WITHOUT `compile_to_sql` (set it to return `Err("query compilation not supported in this slice")`). Generators that would synthesize query content are gated on `SutQueryCompile` (Phase 6g) and skip blocks with `query_source`. **Revised LOC estimate: 600-800.** Well under budget.

The slice still validates the framework's structural claim (same transitions, same invariants, totally different SUT composition) without the cost overhead.

## What this means for Phase 9 scoping

Phase 9 in the plan reads:
> **Gated on**: Phase 1's H7 audit showing the LOC budget is ≤1500 ... matview-required count ≤2

Both gates pass. The in-memory `BuilderServices` impl is **feasible at ~600-800 LOC** if we adopt the no-query-compilation path. Phase 9 stays in scope.
