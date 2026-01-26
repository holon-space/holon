---
generated: auto
---

# Research: reactive-vm-refactor

## Executive Summary

Holon's persistent-node + `Mutable<T>` architecture aligns with state-of-the-art Rust reactive UI (futures-signals/haalka, Floem). The 5 devlog next-steps are feasible but have important ordering constraints: step 1 (wire set_template) is safe and high-value; step 2 (resolve_props_only) benefits ~40-50% of builders; step 3 (eliminate shadow builders) is structural and requires step 2; step 4 (remove engine caches) needs node-level state to replace cache-level state; step 5 (direct data+expr in GPUI) should be deferred — props HashMap is the correct intermediary.

## External Research

### Reactive UI Framework Landscape

| Framework | Reactivity | Collection Strategy | Node Persistence |
|-----------|-----------|--------------------|-----------------| 
| futures-signals/haalka | `Mutable<T>` + Signal | `MutableVec` + `VecDiff` (O(1)) | Nodes own Mutables |
| Leptos | `RwSignal<T>` + graph | `<For>` keyed list | Compiled to static templates |
| Floem | leptos-inspired signals | Virtual list | View tree built once |
| Xilem | View diff + retained widgets | Tree diff → mutations | Widget tree persists |
| GPUI | Entity model + `cx.notify()` | Entity-based | Entities persist in AppContext |
| **Holon** | `Mutable<T>` + InterpretFn | MutableVec + flat/tree drivers | `apply_update` preserves identity |

**Holon sits in the futures-signals/haalka camp** — per-node Mutable ownership with driver-mediated updates. Correct for GPUI's entity model.

### Template Switching Prior Art

Most frameworks (SolidJS, Leptos, Floem) destroy+recreate child nodes on template change. Holon's `set_template()` re-interprets existing nodes in place — superior for preserving scroll position and entity caches. **Novel pattern, no exact prior art.**

### Builder Elimination Prior Art

No exact prior art for "macro-generated builder → macro-generated InterpretFn closure". Closest: haalka's ECS self-describing entities. Devlog sequencing (leaves → containers → collections → special) is correct.

### Pitfalls

| Pitfall | Severity | Mitigation |
|---------|----------|-----------|
| Signal diamond (data+expr change simultaneously) | Low | `set_neq()` + generation counter vs HashMap comparison |
| Stale InterpretFn closures on reparent | Low | Reparenting triggers structural rebuild → fresh closures |
| Props HashMap overhead | Low | Keep as intermediary; defer step 5 |
| Template switch ≠ layout variant switch | Medium | `set_template()` for intra-variant only; full rebuild for cross-variant |

## Codebase Analysis

### View Mode Switcher — Current Flow

```
User clicks mode button → GPUI handler:
  1. active_mode_handle.set("table")
  2. services.set_view_mode(&entity_uri, "table")
  3. expr = mode_templates["mode_table"]       // from serialized props
  4. content = services.interpret(expr, ctx)    // FULL re-interpretation
  5. start_reactive_views(&content, ...)        // new drivers
  6. slot.content.set(Arc::new(content))        // swap slot
  7. window.refresh()
```

**Target:** Replace steps 4-6 with `reactive_view.set_template(new_expr)` → template_driver re-interprets props in place. No new tree, no entity cache churn.

**Blocker:** `set_template()` only changes `RenderExpr`, not `CollectionVariant`. Table→tree requires fresh ReactiveView (different driver). Use `set_template()` for intra-variant switches; keep full rebuild for cross-variant.

### Shadow Builder Census (42 total)

| Category | Count | Examples | InterpretFn eligible? |
|----------|-------|---------|---------------------|
| Leaf widgets | 5 | text, badge, icon, checkbox, spacer | Yes — pure args→props |
| Layout containers | 5 | row, column, section, card, collapsible | No — child interpretation |
| Collection widgets | 8 | list, tree, table, columns, query_result | No — ReactiveView creation |
| Special (side effects) | 9 | expand_toggle, view_mode_switcher, live_query, block_ref | No — cache/watch side effects |
| Other (data extraction) | 15 | editable_text, state_toggle, source_block, op_button | Partial — ~10 eligible |

**~15-20 builders (40-50%) eligible for props-only fast path. ~22-27 require full interpretation.**

### InterpretFn Architecture

```rust
pub type InterpretFn =
    Arc<dyn Fn(&RenderExpr, &Arc<DataRow>) -> HashMap<String, Value> + Send + Sync>;
```

- Stored on `ReactiveViewModel::interpret_fn`
- Called by `set_data()` and `set_expr()` to recompute props without full tree rebuild
- **Current limitation:** Internally calls `services.interpret()` — not truly side-effect-free
- Created by collection drivers, captured at node-creation time

### Engine Caches

| Cache | Type | Call Sites | Breaks If Removed |
|-------|------|-----------|-------------------|
| `expand_state_cache` | `HashMap<String, Mutable<bool>>` | expand_toggle.rs only | Toggles reset on structural rebuild |
| `view_mode_cache` | `HashMap<String, Mutable<String>>` | view_mode_switcher.rs only | Mode selection lost; multi-position desync |

**Both caches survive re-interpretation** — same `Mutable<T>` handle reused from cache. With persistent nodes (`apply_update` preserving `expanded` Mutable), the caches become redundant for same-position entities. Cross-position sharing (same entity in two tree locations) still needs them.

### GPUI Builder Prop Access

- 27 of 39 GPUI builders use `prop_str/f64/bool/value` (59 call sites)
- No builder currently accesses `node.data` or `node.expr` directly
- Direct access would lose expression evaluation and couple to row structure
- Props HashMap is the correct intermediary — defer step 5

### Dead Code

- `NEXT_VIEW_ID` in `reactive_view.rs:157` — never used, safe to remove

## Test Coverage

| Test | Exercises | Relevant to |
|------|----------|------------|
| `layout_proptest::layout_invariants_hold_for_random_scenarios` | Mode switches, action replay, end-state equivalence | Steps 1, 4 |
| `layout_proptest::block_ref_inside_tree_item_has_nonzero_height` | Expand state, tree item layout | Step 4 |
| `layout_proptest::streaming_collection_data_arrival` | MutableVec data delivery, subscribe_inner_collections | Steps 2, 3 |
| `general_e2e_pbt` (Full/SqlOnly/CrossExecutor) | Data mutations, CDC, undo/redo, text edit | Overall correctness |

## Quality Commands

| Type | Command |
|------|---------|
| Build | `cargo check --workspace` |
| PBT E2E | `cargo nextest run -p holon-integration-tests --test general_e2e_pbt` |
| Layout proptest | `cargo nextest run -p holon-gpui --test layout_proptest` |
| Frontend tests | `cargo nextest run -p holon-frontend` |

## Feasibility Assessment

| Step | Feasibility | Risk | Effort |
|------|------------|------|--------|
| 1. Wire set_template() | High | Low (intra-variant only) | S |
| 2. resolve_props_only | Medium | Medium (side-effect classification) | M |
| 3. Eliminate shadow builders | Medium | Medium-High (macro refactoring) | L |
| 4. Remove engine caches | Medium | Medium (PBT regression test first) | M |
| 5. GPUI direct data+expr | Low priority | Low (deferred) | L |
| 6. Cleanups | High | Low | S |

## Recommendations

1. **Do step 1 first** — wire `set_template()` into GPUI view_mode_switcher for intra-variant switches. Don't add `set_layout()` yet.
2. **Do step 6 (cleanups) early** — remove NEXT_VIEW_ID, fix subscribe_props_signals granularity, rename update_from. Low risk, clears noise.
3. **Steps 2+3 together** — resolve_props_only is the enabler for shadow builder elimination. Factor `widget_builder!` macro into two paths: `generate_interpret_fn()` (pure args→props) and `generate_builder()` (structural).
4. **Step 4 after PBT verification** — run layout_proptest with caches removed. If tests pass, caches are redundant. If not, the failing test shows which edge case still needs them.
5. **Defer step 5** — props HashMap is correct. Only pursue after shadow builders are eliminated.

## Sources

- futures-signals, haalka, Floem, Leptos, Dioxus, Xilem, GPUI docs
- Internal: reactive_view_model.rs, reactive_view.rs, reactive.rs, shadow_builders/, render/builders/
- devlog/2026-04-22-reactive-vm-refactor.md
