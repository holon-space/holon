---
generated: auto
---

# Requirements: Reactive ViewModel Persistent-Node Refactor — Downstream Simplifications

## Goal

Complete the persistent-node refactor by wiring template switching into the GPUI view_mode_switcher, adding a props-only fast path for collection drivers, refactoring the macro to separate InterpretFn from structural builders, removing the now-redundant engine caches, and cleaning up dead code. Each step profits from the persistent-node foundation landed in sessions 1-4.

## User Stories

### Epic 1: Wire `set_template()` into View Mode Switcher

#### US-1.1: Intra-variant template switching without rebuild
**As a** user switching between view modes (e.g. table columns A vs table columns B)
**I want to** see the mode change instantly without scroll position reset
**So that** my context is preserved during view mode switches

**Acceptance Criteria:**
- [ ] AC-1.1.1: GPUI `view_mode_switcher.rs` click handler calls `reactive_view.set_template(new_expr)` instead of `services.interpret() → start_reactive_views() → slot.set()`
- [ ] AC-1.1.2: Intra-variant switch (table→table with different columns) preserves GPUI entity cache — no `Arc<ReactiveViewModel>` recreation
- [ ] AC-1.1.3: Cross-variant switch (table→tree) still triggers full rebuild via `start_reactive_views` (different `CollectionVariant` needs different driver)
- [ ] AC-1.1.4: `layout_proptest::layout_invariants_hold_for_random_scenarios` passes (exercises mode switches)
- [ ] AC-1.1.5: `general_e2e_pbt` passes in all variants (Full, SqlOnly, CrossExecutor)

#### US-1.2: View mode switcher accesses ReactiveView for template updates
**As a** developer maintaining the view_mode_switcher builder
**I want to** access the parent's `ReactiveView` from the click handler
**So that** I can call `set_template()` without re-creating the reactive pipeline

**Acceptance Criteria:**
- [ ] AC-1.2.1: The `ReactiveView` reference is accessible from the GPUI click handler (either via node's `collection` field or a new prop/capture)
- [ ] AC-1.2.2: `set_template()` only fires when the target mode uses the same `CollectionVariant` as the current mode
- [ ] AC-1.2.3: If `CollectionVariant` differs, fallback to the existing full-rebuild path — no silent degradation

---

### Epic 2: Lightweight `resolve_props_only`

#### US-2.1: Fast-path props extraction for data-only updates
**As a** user scrolling through a large collection
**I want to** row data updates to apply with minimal overhead
**So that** scroll performance stays smooth even with live CDC updates

**Acceptance Criteria:**
- [ ] AC-2.1.1: New `resolve_props(widget_name, expr, data) -> HashMap<String, Value>` function runs only arg-resolution from the `widget_builder!` macro, no child/slot/collection creation
- [ ] AC-2.1.2: `InterpretFn` closures in flat/tree drivers call `resolve_props` instead of `services.interpret()`
- [ ] AC-2.1.3: Side-effect builders (expand_toggle, block_ref, live_query, view_mode_switcher) are NOT eligible — their `InterpretFn` still calls full interpret
- [ ] AC-2.1.4: `streaming_collection_data_arrival` layout proptest passes
- [ ] AC-2.1.5: `general_e2e_pbt` passes in all variants

#### US-2.2: Classify builders as props-only vs full-interpret
**As a** developer adding new widget builders
**I want to** a clear classification of which builders support props-only fast path
**So that** the `InterpretFn` closure picks the right path automatically

**Acceptance Criteria:**
- [ ] AC-2.2.1: Each shadow builder is annotated (via macro attribute or const) as `props_only: true/false`
- [ ] AC-2.2.2: Builders with children, collections, slots, or cache side effects are `props_only: false`
- [ ] AC-2.2.3: The collection driver's `InterpretFn` creation reads this classification and dispatches accordingly

---

### Epic 3: Eliminate Shadow Builders (Macro Refactor)

#### US-3.1: Separate InterpretFn generation from structural builder
**As a** developer maintaining widget builders
**I want to** the `widget_builder!` macro to produce two distinct outputs: a lightweight `InterpretFn` (pure args→props) and a structural builder (children + slots + collections)
**So that** data/template updates don't pay for structural creation code

**Acceptance Criteria:**
- [ ] AC-3.1.1: `widget_builder!` macro generates a `generate_interpret_fn()` that returns `InterpretFn`-compatible closure (takes `(expr, data)`, returns `HashMap<String, Value>`)
- [ ] AC-3.1.2: `widget_builder!` macro generates a `generate_builder()` that handles structural work (children, slots, collections, side effects) and sets the node's `interpret_fn` to the generated function
- [ ] AC-3.1.3: For leaf widgets (text, badge, icon, checkbox, spacer), the structural builder is a trivial wrapper that creates the node and sets the InterpretFn — no manual builder body needed
- [ ] AC-3.1.4: For complex widgets (expand_toggle, block_ref, columns), the manual builder body remains but the InterpretFn is auto-generated for the props-extraction portion

#### US-3.2: Leaf widget builders eliminated
**As a** developer
**I want to** leaf widgets (text, badge, icon, spacer, checkbox) to have no manual shadow builder
**So that** adding a new leaf widget only requires a `widget_builder!` declaration

**Acceptance Criteria:**
- [ ] AC-3.2.1: All 5 leaf widget shadow builders are removed or reduced to auto-generated code
- [ ] AC-3.2.2: Props computed by the generated InterpretFn match the props computed by the old manual builder (verified by existing tests)
- [ ] AC-3.2.3: GPUI builders continue to read props via `node.prop_str()` / `node.prop_f64()` — no GPUI-side changes
- [ ] AC-3.2.4: All frontend tests pass (36 tests)

#### US-3.3: Layout container builders use generated InterpretFn
**As a** developer
**I want to** layout containers (row, column, section, card, collapsible) to use the generated InterpretFn for their props while keeping structural child interpretation in the builder
**So that** data-only updates on layout containers skip child re-creation

**Acceptance Criteria:**
- [ ] AC-3.3.1: Layout container builders delegate props extraction to the generated InterpretFn
- [ ] AC-3.3.2: Child interpretation remains in the structural builder path
- [ ] AC-3.3.3: `layout_proptest` passes — layout containers render correctly with the split

---

### Epic 4: Remove Engine Caches

#### US-4.1: Remove expand_state_cache
**As a** developer simplifying the codebase
**I want to** remove the `expand_state_cache: HashMap<String, Mutable<bool>>` from `ReferenceState`
**So that** expand state is owned exclusively by the persistent node's `expanded: Option<Mutable<bool>>`

**Acceptance Criteria:**
- [ ] AC-4.1.1: `expand_state_cache` field and its `get_or_create_expand_state()` method removed from `reactive.rs`
- [ ] AC-4.1.2: `expand_toggle` shadow builder reads/creates `expanded` Mutable on the node directly
- [ ] AC-4.1.3: `ui_state()` method no longer reads from expand_state_cache for `is_expanded`
- [ ] AC-4.1.4: `layout_proptest::block_ref_inside_tree_item_has_nonzero_height` passes (exercises expand state)
- [ ] AC-4.1.5: `general_e2e_pbt` passes in all variants

#### US-4.2: Remove view_mode_cache
**As a** developer simplifying the codebase
**I want to** remove the `view_mode_cache: HashMap<String, Mutable<String>>` from `ReferenceState`
**So that** view mode state is owned by the view_mode_switcher node's props

**Acceptance Criteria:**
- [ ] AC-4.2.1: `view_mode_cache` field and `get_or_create_view_mode()` / `set_view_mode()` methods removed from `reactive.rs`
- [ ] AC-4.2.2: View mode state stored on the node (via `active_mode` prop or a dedicated `Mutable<String>`)
- [ ] AC-4.2.3: `ui_state()` method no longer reads from view_mode_cache for `view_mode`
- [ ] AC-4.2.4: `layout_proptest::layout_invariants_hold_for_random_scenarios` passes (exercises mode switches)
- [ ] AC-4.2.5: Cross-position entity edge case (same entity in two tree locations) does NOT silently lose state — either both positions share state or the system explicitly drops it

---

### Epic 6: Cleanups

#### US-6.1: Remove dead NEXT_VIEW_ID
**As a** developer reading `reactive_view.rs`
**I want to** unused statics removed
**So that** the module doesn't mislead readers about identity tracking

**Acceptance Criteria:**
- [ ] AC-6.1.1: `static NEXT_VIEW_ID: AtomicU64` at `reactive_view.rs:157` deleted
- [ ] AC-6.1.2: No compilation errors after removal

#### US-6.2: subscribe_props_signals incremental updates
**As a** developer optimizing collection performance
**I want to** `subscribe_props_signals` to add/remove individual watchers on InsertAt/Push/RemoveAt/Pop
**So that** adding one row doesn't clear and re-subscribe all existing prop watchers

**Acceptance Criteria:**
- [ ] AC-6.2.1: `VecDiff::InsertAt` adds a single props watcher for the new item at the correct index
- [ ] AC-6.2.2: `VecDiff::Push` adds a single props watcher for the new item at the end
- [ ] AC-6.2.3: `VecDiff::RemoveAt`/`Pop` do NOT explicitly cancel the watcher — the stale watcher's `this.update()` returns `Err` and breaks naturally
- [ ] AC-6.2.4: `VecDiff::Replace` still clears and re-subscribes all (current behavior)
- [ ] AC-6.2.5: `streaming_collection_data_arrival` layout proptest passes

#### US-6.3: Rename update_from to patch_mutables
**As a** developer reading `reactive_view_model.rs`
**I want to** `update_from` renamed to `patch_mutables`
**So that** the name reflects its narrower role (patching Mutable fields, not replacing the node)

**Acceptance Criteria:**
- [ ] AC-6.3.1: Method renamed from `update_from` to `patch_mutables` in `reactive_view_model.rs`
- [ ] AC-6.3.2: All call sites updated (push_down_children, set_data, set_expr)
- [ ] AC-6.3.3: No compilation errors

---

## Functional Requirements

| ID | Requirement | Priority | Acceptance Criteria |
|----|-------------|----------|---------------------|
| FR-1 | `set_template()` wired into GPUI view_mode_switcher for intra-variant switches | High | AC-1.1.1 through AC-1.1.5 |
| FR-2 | Cross-variant switches (table↔tree) still use full rebuild | High | AC-1.2.3 |
| FR-3 | `resolve_props()` fast path for ~15-20 eligible builders | High | AC-2.1.1 through AC-2.1.5 |
| FR-4 | Builder classification (props_only vs full-interpret) | Medium | AC-2.2.1 through AC-2.2.3 |
| FR-5 | Macro generates separate InterpretFn and structural builder | Medium | AC-3.1.1 through AC-3.1.4 |
| FR-6 | 5 leaf widget shadow builders eliminated | Medium | AC-3.2.1 through AC-3.2.4 |
| FR-7 | Engine caches removed, node owns state | Medium | AC-4.1.1 through AC-4.2.5 |
| FR-8 | NEXT_VIEW_ID removed | Low | AC-6.1.1, AC-6.1.2 |
| FR-9 | Incremental props watcher subscribe | Low | AC-6.2.1 through AC-6.2.5 |
| FR-10 | update_from → patch_mutables rename | Low | AC-6.3.1 through AC-6.3.3 |

## Non-Functional Requirements

| ID | Requirement | Metric | Target |
|----|-------------|--------|--------|
| NFR-1 | No test regressions | PBT + proptest + frontend tests | All pass after each step |
| NFR-2 | No new unsafe code | Code review | Zero new `unsafe` blocks |
| NFR-3 | Checkpoint after each epic | jj commits | ≥5 checkpoints (one per epic) |

## Glossary

- **InterpretFn**: `Arc<dyn Fn(&RenderExpr, &Arc<DataRow>) -> HashMap<String, Value>>` — stored on each node, recomputes props from (expr, data) without full tree rebuild
- **CollectionVariant**: Layout enum (Tree, Table, List, Columns, Outline) — determines which driver type is spawned
- **Intra-variant switch**: Changing the RenderExpr template within the same CollectionVariant (e.g. table with columns A → table with columns B)
- **Cross-variant switch**: Changing between different CollectionVariants (e.g. table → tree) — requires new ReactiveView with different driver
- **Shadow builder**: Frontend-side function that interprets a RenderExpr into a ReactiveViewModel tree. Generated by `widget_builder!` macro
- **Props-only fast path**: Running only the arg-resolution step of a shadow builder to extract props, skipping child/slot/collection creation
- **Engine cache**: `expand_state_cache` and `view_mode_cache` on ReferenceState — HashMap lookups that preserve UI state across tree rebuilds. Redundant with persistent-node architecture

## Out of Scope

- Step 5 (GPUI builders read from data+expr directly) — deferred, props HashMap is correct intermediary
- Per-node `map_ref!` signal tasks for autonomous data→props reaction — current driver-mediated `set_data()` is sufficient
- Signal diamond batching (simultaneous data+expr changes fire InterpretFn twice) — benign, not worth the complexity
- Collection widgets (list, tree, table, columns, query_result) shadow builder elimination — they need ReactiveView creation in the structural path
- Cross-position state sharing for expand/view_mode — accept state loss on reparent as acceptable tradeoff

## Dependencies

- Persistent-node architecture (sessions 1-4) — DONE
- `InterpretFn` + `set_data()` / `set_expr()` — DONE
- `template_mutable` + `set_template()` on ReactiveView — DONE
- `apply_update` / `push_down_children` preserving `expanded` and `slot` — DONE
- `props_watchers` in ReactiveShell — DONE

## Assumptions

- The `set_template()` template_driver is correctly wired in the flat collection driver (validated by PoC tests)
- `apply_update` / `push_down_children` preserves `expanded: Option<Mutable<bool>>` for same-position entities (validated by devlog session 4)
- Cross-position state sharing is rare enough that losing expand/view_mode state on reparent is acceptable
- The ~15-20 props-only-eligible builders have no hidden side effects in their arg-resolution code

## Unresolved Questions

- For US-1.2: How does the GPUI click handler get a reference to the parent's `ReactiveView`? Options: (a) store `Arc<ReactiveView>` on the node, (b) pass it through the render context, (c) capture it in the click closure. Decision: implementation should pick the simplest option that doesn't add fields to `ReactiveViewModel` for all nodes.
- For US-4.1: Does `ui_state()` need to read `is_expanded` at all after removing the cache? If the node owns its Mutable, the shadow builder reads it directly. But if `ui_state()` feeds into RenderContext for initial interpretation, some bridge is still needed.
- For US-3.1: Should the generated InterpretFn live as a registered function in RenderInterpreter or as a static function on the shadow builder module? Former is more decoupled; latter is simpler.

## Success Criteria

- View mode switching within the same variant type preserves scroll position and entity caches
- Collection data updates use `resolve_props` fast path for eligible builders (measurable via tracing spans)
- At least 5 leaf widget shadow builders eliminated — only macro declaration remains
- `expand_state_cache` and `view_mode_cache` removed from `reactive.rs` — grep returns zero hits
- All PBTs and proptests pass at every checkpoint

## Next Steps

1. Implement Epic 6 (cleanups) first — lowest risk, clears noise
2. Implement Epic 1 (wire set_template) — highest immediate payoff
3. Implement Epics 2+3 together (resolve_props_only + macro refactor) — tightly coupled
4. Implement Epic 4 (remove engine caches) — needs PBT verification
5. Run full test suite at each checkpoint boundary
